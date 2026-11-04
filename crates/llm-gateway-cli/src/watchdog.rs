//! `b10x-llm-gateway watchdog`: a separate process that terminates the pods the gateway owns
//! when the gateway cannot (story:pod-watchdog).
//!
//! The gateway can die with a pod running, so stopping a pod must not depend on it. The
//! watchdog reads the same deployment document, polls the Runpod account every
//! `poll_seconds`, and terminates a pod the gateway owns when any of these holds, checked in
//! this order:
//!
//! 1. `min-balance`: the account balance is under `min_balance_usd`.
//! 2. `stale-heartbeat`: the gateway's heartbeat is missing or older than `stale_minutes`.
//! 3. `max-runtime`: the pod has run longer than `max_runtime_hours`.
//! 4. `not-ready`: the pod has not been ready for `not_ready_minutes`.
//! 5. `idle`: the heartbeat lists the pod's model with no call in flight, and its last call
//!    ended longer ago than the model's `idle_timeout_minutes`, never less than
//!    `stale_minutes`, so the watchdog does not race the gateway's own idle stop.
//!
//! A pod is the gateway's when its name starts with `llm_runpod::POD_NAME_PREFIX` and its
//! environment's owner tag is the gateway's controller identity, [`crate::CONTROLLER`]. Every
//! other pod of the account is somebody else's and is never terminated, whatever holds. Each
//! termination, and each account read that failed, is one JSON line on standard error.

use crate::{
    cli::WatchdogCli,
    config::{Deployment, WatchdogConfig},
    heartbeat::{self, Heartbeat},
    refusal::{Refusal, StartupRefusal},
    relaying::{CONTROLLER, ProbeOver, idle_timeout_ms},
};
use llm_runpod::{
    POD_NAME_PREFIX, PodSummary, Probe, ProbeTarget, RestBinding, RestRunpod, RunpodAccount,
    RunpodTransport, TerminateAnswer, in_namespace,
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// The format every watchdog record names.
pub const RECORD_FORMAT: &str = "llm-gateway.watchdog/1";

/// How often the waiting loop looks for SIGINT or SIGTERM.
const SIGNAL_CHECK: Duration = Duration::from_millis(200);

/// Why the watchdog terminated a pod.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reason {
    MinBalance,
    StaleHeartbeat,
    MaxRuntime,
    NotReady,
    Idle,
}

impl Reason {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::MinBalance => "min-balance",
            Self::StaleHeartbeat => "stale-heartbeat",
            Self::MaxRuntime => "max-runtime",
            Self::NotReady => "not-ready",
            Self::Idle => "idle",
        }
    }
}

/// One terminate the watchdog asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Termination {
    pub(crate) pod: String,
    pub(crate) name: String,
    pub(crate) reason: Reason,
    pub(crate) answer: TerminateAnswer,
}

/// What one poll did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Polled {
    /// The account read failed: nothing was terminated.
    AccountUnreadable,
    /// The account was read; these pods were asked to terminate.
    Read(Vec<Termination>),
}

/// The watchdog's limits and what it remembers between polls.
pub(crate) struct Watch {
    stale_ms: u64,
    not_ready_ms: u64,
    max_runtime_ms: u64,
    min_balance_usd: f64,
    /// Each declared model's idle window, by alias.
    idle_ms: BTreeMap<String, u64>,
    /// When this watchdog first saw each pod, by pod id.
    first_seen: BTreeMap<String, u64>,
    /// When this watchdog last saw each pod ready, by pod id.
    last_ready: BTreeMap<String, u64>,
}

impl Watch {
    pub(crate) fn new(deployment: &Deployment, config: &WatchdogConfig) -> Self {
        Self {
            stale_ms: config.stale_minutes.saturating_mul(60_000),
            not_ready_ms: config.not_ready_minutes.saturating_mul(60_000),
            max_runtime_ms: config.max_runtime_hours.saturating_mul(3_600_000),
            min_balance_usd: config.min_balance_usd,
            idle_ms: deployment
                .models
                .iter()
                .map(|(alias, model)| (alias.clone(), idle_timeout_ms(model.idle_timeout_minutes)))
                .collect(),
            first_seen: BTreeMap::new(),
            last_ready: BTreeMap::new(),
        }
    }

    /// One poll at `now_ms`: reads the account, then terminates each owned pod a limit names.
    pub(crate) fn poll<T: RunpodTransport + RunpodAccount>(
        &mut self,
        transport: &mut T,
        heartbeat: Option<&Heartbeat>,
        now_ms: u64,
    ) -> Polled {
        let Some(account) = transport.account() else {
            return Polled::AccountUnreadable;
        };
        let owned: Vec<&PodSummary> = account
            .pods
            .iter()
            .filter(|pod| owned(pod) && pod.desired_status != "TERMINATED")
            .collect();
        let live: Vec<&str> = owned.iter().map(|pod| pod.id.as_str()).collect();
        self.first_seen.retain(|id, _| live.contains(&id.as_str()));
        self.last_ready.retain(|id, _| live.contains(&id.as_str()));
        let low_balance = account
            .balance_usd
            .is_some_and(|balance| balance < self.min_balance_usd);
        let stale =
            heartbeat.is_none_or(|beat| now_ms.saturating_sub(beat.time_ms) > self.stale_ms);
        let mut terminations = Vec::new();
        for pod in owned {
            let reason = if low_balance {
                Some(Reason::MinBalance)
            } else if stale {
                Some(Reason::StaleHeartbeat)
            } else {
                self.limit(transport, pod, heartbeat, now_ms)
            };
            if let Some(reason) = reason {
                let answer = transport.terminate_pod(&pod.id);
                if answer == TerminateAnswer::Terminated {
                    self.first_seen.remove(&pod.id);
                    self.last_ready.remove(&pod.id);
                }
                terminations.push(Termination {
                    pod: pod.id.clone(),
                    name: pod.name.clone(),
                    reason,
                    answer,
                });
            }
        }
        Polled::Read(terminations)
    }

    /// The pod's own limits, past a fresh heartbeat: runtime, readiness, idleness.
    fn limit<T: RunpodTransport>(
        &mut self,
        transport: &mut T,
        pod: &PodSummary,
        heartbeat: Option<&Heartbeat>,
        now_ms: u64,
    ) -> Option<Reason> {
        let born = self.born(pod, now_ms);
        if now_ms.saturating_sub(born) > self.max_runtime_ms {
            return Some(Reason::MaxRuntime);
        }
        let alias = pod.name.strip_prefix(POD_NAME_PREFIX).unwrap_or_default();
        let ready_since = self
            .last_ready
            .get(&pod.id)
            .copied()
            .unwrap_or(born)
            .max(born);
        if now_ms.saturating_sub(ready_since) > self.not_ready_ms {
            // Probed only once it would be terminated otherwise: a ready pod resets the window.
            if ready(transport, pod, alias) {
                self.last_ready.insert(pod.id.clone(), now_ms);
            } else {
                return Some(Reason::NotReady);
            }
        }
        let idle_ms = self.idle_ms.get(alias)?.max(&self.stale_ms);
        let route = heartbeat?
            .routes
            .iter()
            .find(|route| route.route == alias)?;
        let last = route.last_call_end_ms.unwrap_or(0).max(born);
        (route.in_flight == 0 && now_ms.saturating_sub(last) > *idle_ms).then_some(Reason::Idle)
    }

    /// The earliest instant the pod is known to have existed: its container start, its uptime
    /// counted back, or when this watchdog first saw it.
    fn born(&mut self, pod: &PodSummary, now_ms: u64) -> u64 {
        let seen = *self.first_seen.entry(pod.id.clone()).or_insert(now_ms);
        let uptime = pod
            .uptime_seconds
            .map(|seconds| now_ms.saturating_sub(seconds.saturating_mul(1_000)));
        [Some(seen), pod.last_started_at_ms, uptime]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(now_ms)
    }
}

/// The gateway's pod: in its name space and tagged with its controller identity.
pub(crate) fn owned(pod: &PodSummary) -> bool {
    in_namespace(&pod.name) && pod.env_owner.as_deref() == Some(CONTROLLER)
}

/// One probe of the pod's vLLM server at llm's Runpod endpoint for it.
fn ready<T: RunpodTransport>(transport: &mut T, pod: &PodSummary, alias: &str) -> bool {
    let endpoint = llm_providers::descriptions::runpod()
        .inference_base_url(&pod.id)
        .ok()
        .map(|url| url.as_str().to_owned());
    matches!(
        transport.probe_ready(&ProbeTarget {
            pod_id: &pod.id,
            model: (!alias.is_empty()).then_some(alias),
            endpoint: endpoint.as_deref(),
        }),
        Probe::Ready { .. }
    )
}

const fn answer_label(answer: TerminateAnswer) -> &'static str {
    match answer {
        TerminateAnswer::Terminated => "terminated",
        TerminateAnswer::Refused => "refused",
        TerminateAnswer::Lost => "lost",
    }
}

/// The JSON lines one poll writes: one per termination, or one for an unreadable account.
pub(crate) fn records(polled: &Polled, now_ms: u64) -> Vec<String> {
    match polled {
        Polled::AccountUnreadable => vec![
            serde_json::json!({
                "format": RECORD_FORMAT,
                "time_ms": now_ms,
                "event": "account-unreadable",
            })
            .to_string(),
        ],
        Polled::Read(terminations) => terminations
            .iter()
            .map(|termination| {
                serde_json::json!({
                    "format": RECORD_FORMAT,
                    "time_ms": now_ms,
                    "event": "terminate",
                    "pod": termination.pod,
                    "name": termination.name,
                    "reason": termination.reason.label(),
                    "answer": answer_label(termination.answer),
                })
                .to_string()
            })
            .collect(),
    }
}

/// The Runpod API key the watchdog reads the account with. No provider declares one yet: the
/// keychain reference (`runpod_api_key`) is read here once the document carries it.
fn runpod_key(_deployment: &Deployment) -> Result<Vec<u8>, Refusal> {
    Err(Refusal::new(
        StartupRefusal::ConfigValue,
        "watchdog: no provider declares runpod_api_key, which the account read needs".to_owned(),
    ))
}

/// The REST transport over `pods`' verified TLS, with every model's vLLM key for the probe.
fn rest_transport(
    deployment: &Deployment,
    key: Vec<u8>,
    pods: &crate::ProxyConnector,
) -> Result<RestRunpod, Refusal> {
    let mut transport = RestRunpod::new(RestBinding::runpod(), key);
    transport.set_tls(Arc::new(ProbeOver(Arc::new(pods.clone()))));
    let keys = crate::vllm_keys(deployment)?;
    for alias in deployment.models.keys() {
        if let Some(key) = keys.get(alias) {
            transport.set_vllm_key(alias, key.expose().to_vec());
        }
    }
    Ok(transport)
}

/// Polls through `transport` until `stop` is set, or once.
pub(crate) fn run<T: RunpodTransport + RunpodAccount>(
    watch: &mut Watch,
    transport: &mut T,
    heartbeat_file: &std::path::Path,
    poll: Duration,
    once: bool,
    stop: &AtomicBool,
) {
    loop {
        let now_ms = heartbeat::unix_ms();
        let beat = heartbeat::read(heartbeat_file);
        let polled = watch.poll(transport, beat.as_ref(), now_ms);
        for line in records(&polled, now_ms) {
            use std::io::Write as _;
            drop(writeln!(std::io::stderr(), "{line}"));
        }
        if once {
            return;
        }
        let deadline = Instant::now() + poll;
        while Instant::now() < deadline {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(
                SIGNAL_CHECK.min(deadline.saturating_duration_since(Instant::now())),
            );
        }
        if stop.load(Ordering::SeqCst) {
            return;
        }
    }
}

fn watch_document(cli: &WatchdogCli, pods: &crate::ProxyConnector) -> Result<(), Refusal> {
    let deployment =
        crate::serve::document_path(cli.config.clone()).and_then(|path| crate::load(&path))?;
    let config = deployment.watchdog.clone().ok_or_else(|| {
        Refusal::new(
            StartupRefusal::ConfigValue,
            "watchdog: the document has no [watchdog] table".to_owned(),
        )
    })?;
    let heartbeat_file = heartbeat::path_of(&config).ok_or_else(|| {
        Refusal::new(
            StartupRefusal::ConfigValue,
            "watchdog.heartbeat_file: none declared, and neither XDG_STATE_HOME nor HOME is set"
                .to_owned(),
        )
    })?;
    let mut transport = rest_transport(&deployment, runpod_key(&deployment)?, pods)?;
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(&stop))
            .map_err(|error| Refusal::new(StartupRefusal::SignalInstall, error.to_string()))?;
    }
    let mut watch = Watch::new(&deployment, &config);
    crate::serve::report(format_args!(
        "watchdog polling every {} s",
        config.poll_seconds
    ));
    run(
        &mut watch,
        &mut transport,
        &heartbeat_file,
        Duration::from_secs(config.poll_seconds),
        cli.once,
        &stop,
    );
    Ok(())
}

/// The `watchdog` command: load the document, then watch until SIGINT or SIGTERM, or once.
pub(crate) fn main(cli: &WatchdogCli, pods: &crate::ProxyConnector) -> std::process::ExitCode {
    match watch_document(cli, pods) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(refusal) => {
            crate::serve::report(format_args!("refused {refusal}"));
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "watchdog_tests.rs"]
mod tests;
