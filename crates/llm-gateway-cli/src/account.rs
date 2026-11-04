//! The Runpod account panel `serve` shows on its dashboard (story:live-dashboard).
//!
//! When a provider names its Runpod API key by a keychain reference (`providers.<name>.api_key`),
//! `serve` reads the account every [`POLL`]: its balance, its spend rate and its pods. The pods
//! the gateway owns (by name prefix and owner tag, as the watchdog decides) become the panel's
//! rows; every other pod is only counted. The read is the watchdog's own read
//! (`RunpodAccount::account`) and changes nothing on the account.
//!
//! Each pod's shutdown countdown needs the limits that stop it: the model's
//! `idle_timeout_minutes`, after which the gateway's own pool stops an idle pod, and the
//! `[watchdog]` table's maximum runtime and balance floor when the document has one.

use crate::{
    config::Deployment,
    keychain::{KeychainRef, read_keychain_token},
    refusal::{Refusal, StartupRefusal},
    relaying::{ProbeOver, idle_timeout_ms},
    watchdog::owned,
};
use llm_gateway::{Dashboard, PodPanel, ProviderPanel, WatchdogLimits};
use llm_runpod::{AccountSnapshot, POD_NAME_PREFIX, RestBinding, RestRunpod, RunpodAccount};
use std::{
    sync::{Arc, Condvar, Mutex, PoisonError},
    thread::{self, JoinHandle},
    time::Duration,
};

/// How often `serve` reads the account.
pub const POLL: Duration = Duration::from_secs(60);

/// The first provider that names its Runpod API key by a keychain reference, and that reference.
pub(crate) fn keyed_provider(deployment: &Deployment) -> Option<(&str, &KeychainRef)> {
    deployment
        .providers
        .iter()
        .find_map(|(name, provider)| Some((name.as_str(), provider.api_key.as_ref()?)))
}

/// The Runpod API key of `provider`, read from the keychain; the refusal names
/// `providers.<name>.api_key` and quotes nothing read.
pub(crate) fn read_key(provider: &str, reference: &KeychainRef) -> Result<Vec<u8>, Refusal> {
    read_keychain_token(
        reference,
        &format!("providers.{provider}.api_key"),
        (
            StartupRefusal::EndpointCredentialUnreadable,
            StartupRefusal::EndpointCredentialTooLarge,
            StartupRefusal::EndpointCredentialNotAToken,
        ),
    )
}

/// The panel for one account read at `read_ms`.
pub(crate) fn panel(
    provider: &str,
    snapshot: &AccountSnapshot,
    read_ms: u64,
    deployment: &Deployment,
) -> ProviderPanel {
    let live = |pod: &&llm_runpod::PodSummary| pod.desired_status != "TERMINATED";
    let pods: Vec<PodPanel> = snapshot
        .pods
        .iter()
        .filter(live)
        .filter(|pod| owned(pod))
        .map(|pod| {
            let model = pod
                .name
                .strip_prefix(POD_NAME_PREFIX)
                .filter(|alias| !alias.is_empty())
                .map(str::to_owned);
            let idle_ms = model
                .as_deref()
                .and_then(|alias| deployment.models.get(alias))
                .map(|model| idle_timeout_ms(model.idle_timeout_minutes));
            PodPanel {
                id: pod.id.clone(),
                name: pod.name.clone(),
                model,
                state: pod.desired_status.clone(),
                gpu: pod.gpu.clone(),
                cost_per_hour_usd: pod.cost_per_hour_usd,
                uptime_s: pod.uptime_seconds,
                vllm_ready: None,
                idle_ms,
            }
        })
        .collect();
    let others = snapshot
        .pods
        .iter()
        .filter(live)
        .filter(|pod| !owned(pod))
        .count();
    ProviderPanel {
        name: provider.to_owned(),
        read_ms,
        balance_usd: snapshot.balance_usd,
        spend_per_hour_usd: snapshot.spend_per_hour_usd,
        other_pods: u64::try_from(others).unwrap_or(u64::MAX),
        pods,
        watchdog_heartbeat_ms: None,
    }
}

/// The limits each pod's countdown is computed from: the idle stop comes per pod from its
/// model; the maximum runtime and the balance floor from `[watchdog]`, when declared.
pub(crate) fn limits(deployment: &Deployment) -> WatchdogLimits {
    deployment
        .watchdog
        .as_ref()
        .map_or_else(WatchdogLimits::default, |config| WatchdogLimits {
            idle_ms: None,
            max_runtime_ms: Some(config.max_runtime_hours.saturating_mul(3_600_000)),
            min_balance_usd: Some(config.min_balance_usd),
        })
}

/// The account read running beside `serve`; dropping it stops the thread.
pub(crate) struct Polling {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Polling {
    fn drop(&mut self) {
        let (stopped, wake) = &*self.stop;
        *stopped.lock().unwrap_or_else(PoisonError::into_inner) = true;
        wake.notify_all();
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

/// Starts reading the account every `every` into `dashboard`, through `transport`.
pub(crate) fn start<T: RunpodAccount + Send + 'static>(
    mut transport: T,
    provider: String,
    deployment: Deployment,
    dashboard: Arc<Dashboard>,
    every: Duration,
) -> Polling {
    dashboard.set_watchdog(Some(limits(&deployment)));
    let stop = Arc::new((Mutex::new(false), Condvar::new()));
    let signal = Arc::clone(&stop);
    let thread = thread::Builder::new()
        .name("runpod-account".to_owned())
        .spawn(move || {
            let (stopped, wake) = &*signal;
            loop {
                if let Some(snapshot) = transport.account() {
                    dashboard.set_provider(Some(panel(
                        &provider,
                        &snapshot,
                        crate::heartbeat::unix_ms(),
                        &deployment,
                    )));
                } else {
                    tracing::warn!(
                        provider = provider.as_str(),
                        "runpod account read failed; the dashboard keeps the last read"
                    );
                }
                let guard = stopped.lock().unwrap_or_else(PoisonError::into_inner);
                let (guard, _) = wake
                    .wait_timeout_while(guard, every, |stopped| !*stopped)
                    .unwrap_or_else(PoisonError::into_inner);
                if *guard {
                    return;
                }
            }
        })
        .ok();
    Polling { stop, thread }
}

/// The account panel for `deployment`, when a provider names its Runpod API key by a keychain
/// reference; `None` otherwise. The key is read once, here.
pub(crate) fn start_for(
    deployment: &Deployment,
    pods: &crate::ProxyConnector,
    dashboard: Arc<Dashboard>,
) -> Result<Option<Polling>, Refusal> {
    let Some((provider, reference)) = keyed_provider(deployment) else {
        return Ok(None);
    };
    let key = read_key(provider, reference)?;
    let mut transport = RestRunpod::new(RestBinding::runpod(), key);
    transport.set_tls(Arc::new(ProbeOver(Arc::new(pods.clone()))));
    Ok(Some(start(
        transport,
        provider.to_owned(),
        deployment.clone(),
        dashboard,
        POLL,
    )))
}

#[cfg(test)]
mod tests {
    use super::{Polling, panel, start};
    use llm_gateway::Dashboard;
    use llm_runpod::{AccountSnapshot, POD_NAME_PREFIX, PodSummary, RunpodAccount};
    use std::{sync::Arc, time::Duration};

    const DOCUMENT: &str = "listen = \"127.0.0.1:0\"\ningress_auth = \"none\"\n\n[providers.runpod]\nkind = \"runpod-vllm\"\n\n\
         [models.qwen]\nprovider = \"runpod\"\nwires = [\"chat\"]\ncontext_window = 65536\n\
         hf_model = \"example/small-model\"\nimage = \"vllm/vllm-openai:v0.27.1\"\n\
         gpu_types = [\"NVIDIA L40S\"]\nmax_model_len = 1024\nidle_timeout_minutes = 15\n\n\
         [watchdog]\nmax_runtime_hours = 8\nmin_balance_usd = 5.0\n";

    fn pod(id: &str, name: &str, owner: Option<&str>, status: &str) -> PodSummary {
        PodSummary {
            id: id.to_owned(),
            name: name.to_owned(),
            desired_status: status.to_owned(),
            cost_per_hour_usd: Some(0.5),
            gpu: Some("A".to_owned()),
            uptime_seconds: Some(120),
            last_started_at_ms: None,
            env_owner: owner.map(str::to_owned),
        }
    }

    fn snapshot() -> AccountSnapshot {
        let ours = format!("{POD_NAME_PREFIX}qwen");
        AccountSnapshot {
            balance_usd: Some(42.0),
            spend_per_hour_usd: Some(0.5),
            pods: vec![
                pod("p1", &ours, Some(crate::CONTROLLER), "RUNNING"),
                pod("p2", &ours, Some("someone-else"), "RUNNING"),
                pod("p3", "theirs", None, "RUNNING"),
                pod("p4", &ours, Some(crate::CONTROLLER), "TERMINATED"),
            ],
        }
    }

    fn deployment() -> crate::config::Deployment {
        crate::parse(DOCUMENT).expect("the test document parses")
    }

    #[test]
    fn owned_pods_are_rows_with_their_model_idle_and_other_live_pods_are_counted() {
        let panel = panel("runpod", &snapshot(), 1_000, &deployment());
        assert_eq!(panel.balance_usd, Some(42.0));
        assert_eq!(panel.other_pods, 2);
        assert_eq!(panel.pods.len(), 1);
        let row = &panel.pods[0];
        assert_eq!(row.id, "p1");
        assert_eq!(row.model.as_deref(), Some("qwen"));
        assert_eq!(row.idle_ms, Some(15 * 60_000));
        assert_eq!(row.state, "RUNNING");
    }

    struct Account(AccountSnapshot);

    impl RunpodAccount for Account {
        fn account(&mut self) -> Option<AccountSnapshot> {
            Some(self.0.clone())
        }
    }

    #[test]
    fn the_poller_feeds_the_dashboard_a_balance_and_a_shutdown_countdown() {
        let dashboard = Arc::new(Dashboard::default());
        let polling: Polling = start(
            Account(snapshot()),
            "runpod".to_owned(),
            deployment(),
            Arc::clone(&dashboard),
            Duration::from_secs(3600),
        );
        let mut snapshot_text = String::new();
        for _ in 0..200 {
            snapshot_text = dashboard.snapshot();
            if snapshot_text.contains("\"balance_usd\":42") {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(polling);
        assert!(
            snapshot_text.contains("\"balance_usd\":42"),
            "{snapshot_text}"
        );
        assert!(
            snapshot_text.contains("\"limit\":\"idle\""),
            "{snapshot_text}"
        );
        let metrics = dashboard.render_prometheus();
        assert!(
            metrics.contains("llmgw_pod_shutdown_seconds{pod=\"p1\",limit=\"idle\"}"),
            "{metrics}"
        );
    }
}
