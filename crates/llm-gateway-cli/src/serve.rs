//! Composing the gateway from a loaded deployment, serving it, and stopping on a signal
//! (rows C1 and D2), and composing it with the relay to Runpod pods (row B8).

use crate::{
    config::Deployment,
    refusal::{Refusal, StartupRefusal},
    relaying::{CLEANUP_INTERVAL, PodConnector, PoolTargets},
    trusted,
};
use llm_gateway::{
    AuthKind, BillingKind, Gateway, GatewayHandle, Label, OwnerToken, Relay, RelayModel,
    RouteInventory, RouteSummary, SharedSecretVerifier, ShutdownReport, TargetLimits,
    TargetProvenance, TargetSummary, TokenError,
};
use llm_runpod::{Clock, RunpodTransport};
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    iterator::Signals,
};
use std::{
    fmt,
    net::SocketAddr,
    sync::{Arc, Condvar, Mutex, PoisonError},
    thread::{self, JoinHandle},
};

/// The signal that ended a served process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopSignal {
    Sigint,
    Sigterm,
}

impl fmt::Display for StopSignal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Sigint => "SIGINT",
            Self::Sigterm => "SIGTERM",
        })
    }
}

/// How a served process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stopped {
    pub signal: StopSignal,
    pub report: ShutdownReport,
}

/// Reads the owner secret through the trusted-file reader and builds the owner verifier.
///
/// # Errors
/// An `owner-secret:*` [`Refusal`].
pub fn owner_verifier(deployment: &Deployment) -> Result<SharedSecretVerifier, Refusal> {
    let text = trusted::read(&deployment.owner_secret_file, &trusted::OWNER_SECRET)?;
    // ASCII whitespace only: a trailing newline or CRLF, never a non-ASCII character.
    let material = text.trim_end_matches(|character: char| character.is_ascii_whitespace());
    let token = OwnerToken::new(material.as_bytes().to_vec()).map_err(|error| {
        Refusal::new(
            // The token bound is applied here, after trimming; the file bound is the reader's.
            if error == TokenError::TooLarge {
                StartupRefusal::OwnerSecretTooLarge
            } else {
                StartupRefusal::OwnerSecretNotAToken
            },
            format!("{}: {error}", deployment.owner_secret_file.display()),
        )
    })?;
    SharedSecretVerifier::new(token).map_err(|error| {
        Refusal::new(
            StartupRefusal::OwnerSecretTooShort,
            format!("{}: {error}", deployment.owner_secret_file.display()),
        )
    })
}

fn label(value: &str) -> Result<Label, Refusal> {
    Label::new(value).map_err(|error| {
        Refusal::new(
            StartupRefusal::ConfigValue,
            format!("{value:?} cannot be served as an inventory identifier: {error}"),
        )
    })
}

fn inventory_refusal(error: &impl fmt::Display) -> Refusal {
    Refusal::new(StartupRefusal::ConfigValue, format!("inventory: {error}"))
}

/// The inspection inventory: one route per model alias, one target per declared wire, under
/// the digest of the exact document bytes.
///
/// # Errors
/// A `config:value` [`Refusal`] when the document cannot be represented as an inventory.
pub fn inventory(deployment: &Deployment) -> Result<RouteInventory, Refusal> {
    let mut routes = Vec::with_capacity(deployment.models.len());
    for (alias, model) in &deployment.models {
        let mut targets = Vec::with_capacity(model.wires.len());
        for (position, wire) in model.wires.iter().enumerate() {
            targets.push(TargetSummary {
                target_id: label(&format!("{alias}.{}", wire.label()))?,
                position,
                provenance: TargetProvenance {
                    protocol: label(wire.label())?,
                    provider: label(&model.provider)?,
                    account: label(&model.provider)?,
                    endpoint: label(alias)?,
                    model: label(alias)?,
                    binding_revision: label(&deployment.digest)?,
                },
                auth_kind: AuthKind::Bearer,
                billing_kind: BillingKind::SelfHosted,
                limits: TargetLimits {
                    context_window: Some(u64::from(model.context_window)),
                    max_output_tokens: None,
                },
            });
        }
        let route = RouteSummary::new(label(alias)?, label(alias)?, false, targets)
            .map_err(|error| inventory_refusal(&error))?;
        routes.push(route);
    }
    RouteInventory::new(routes)
        .and_then(|inventory| inventory.with_config_digest(&deployment.digest))
        .map_err(|error| inventory_refusal(&error))
}

/// The pool's cleanup pass, run every [`CLEANUP_INTERVAL`] on its own thread until stopped.
struct Cleanup {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: JoinHandle<()>,
}

impl Cleanup {
    fn start(pass: impl Fn() + Send + 'static) -> Self {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let stopped = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let (lock, wake) = &*stopped;
            let mut stopping = lock.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                let (next, _) = wake
                    .wait_timeout_while(stopping, CLEANUP_INTERVAL, |stop| !*stop)
                    .unwrap_or_else(PoisonError::into_inner);
                stopping = next;
                if *stopping {
                    return;
                }
                drop(stopping);
                pass();
                stopping = lock.lock().unwrap_or_else(PoisonError::into_inner);
            }
        });
        Self { stop, thread }
    }

    fn stop(self) {
        let (lock, wake) = &*self.stop;
        *lock.lock().unwrap_or_else(PoisonError::into_inner) = true;
        wake.notify_all();
        drop(self.thread.join());
    }
}

/// A gateway that is bound, ready, and has its stop signals installed.
pub struct Running {
    handle: GatewayHandle,
    signals: Signals,
    vllm_keys: crate::keys::VllmKeys,
    cleanup: Option<Cleanup>,
    /// Whether the gateway relays model calls.
    relaying: bool,
}

impl Running {
    pub fn local_addr(&self) -> SocketAddr {
        self.handle.local_addr()
    }

    /// Each model's vLLM key, read once at startup.
    pub fn vllm_keys(&self) -> &crate::keys::VllmKeys {
        &self.vllm_keys
    }

    /// Whether the gateway relays model calls: only one composed by [`start_relaying`] does.
    pub fn relaying(&self) -> bool {
        self.relaying
    }

    /// Drains and stops the gateway gracefully now, without waiting for a signal, then stops
    /// the pool's cleanup pass.
    pub fn shutdown(self) -> ShutdownReport {
        self.handle.begin_drain();
        let report = self.handle.shutdown();
        if let Some(cleanup) = self.cleanup {
            cleanup.stop();
        }
        report
    }

    /// Blocks until SIGINT or SIGTERM, then drains and stops the gateway gracefully: every
    /// connection already accepted is answered before this returns.
    pub fn wait_for_stop(mut self) -> Stopped {
        // Only the two registered signals are delivered. The iterator ends only when its handle
        // is closed, which nothing here does; should it end anyway, the stop is still graceful.
        let signal = self
            .signals
            .forever()
            .find_map(|signal| match signal {
                SIGINT => Some(StopSignal::Sigint),
                SIGTERM => Some(StopSignal::Sigterm),
                _ => None,
            })
            .unwrap_or(StopSignal::Sigterm);
        self.handle.begin_drain();
        let report = self.handle.shutdown();
        if let Some(cleanup) = self.cleanup.take() {
            cleanup.stop();
        }
        Stopped { signal, report }
    }
}

/// Composes and starts the gateway the deployment describes. The stop signals are installed
/// before the listener is bound, so a signal that arrives once the gateway is reachable always
/// takes the graceful path.
///
/// # Errors
/// An `owner-secret:*`, `vllm-api-key:*`, `config:value`, `signal:install` or `listen:bind`
/// [`Refusal`].
pub fn start(deployment: &Deployment) -> Result<Running, Refusal> {
    let verifier = owner_verifier(deployment)?;
    let vllm_keys = crate::keys::vllm_keys(deployment)?;
    let inventory = inventory(deployment)?;
    let signals = install_signals()?;
    let bind = deployment.gateway.bind;
    let handle = Gateway::bind(deployment.gateway.clone(), Arc::new(verifier), inventory)
        .map_err(|error| Refusal::new(StartupRefusal::ListenBind, format!("{bind}: {error}")))?;
    handle.mark_ready();
    Ok(Running {
        handle,
        signals,
        vllm_keys,
        cleanup: None,
        relaying: false,
    })
}

fn install_signals() -> Result<Signals, Refusal> {
    Signals::new([SIGINT, SIGTERM])
        .map_err(|error| Refusal::new(StartupRefusal::SignalInstall, error.to_string()))
}

fn relay_label(value: &str) -> Result<Label, Refusal> {
    label(value)
}

fn relay_wire(wire: crate::config::Wire) -> llm_gateway::Wire {
    match wire {
        crate::config::Wire::Chat => llm_gateway::Wire::Chat,
        crate::config::Wire::Responses => llm_gateway::Wire::Responses,
        crate::config::Wire::Messages => llm_gateway::Wire::Messages,
    }
}

/// Composes the gateway like [`start`], and also relays each model on its wires to the pod a
/// `RunpodPool` over `transport` hands out, sending the model's vLLM key as the bearer (row
/// B8). Every model must name a `vllm_api_key_file`. The pool runs one cleanup pass before the
/// gateway is marked ready and then one every [`CLEANUP_INTERVAL`] until the stop.
///
/// The shipped binary has no Runpod transport and never calls this (story:live-runpod-wiring);
/// it is the seam a test composes with `EmulatedRunpod` and loopback pods.
///
/// # Errors
/// As [`start`], plus `config:value` for a model the pool cannot run or that names no
/// `vllm_api_key_file`.
pub fn start_relaying<T: RunpodTransport + Send + 'static>(
    deployment: &Deployment,
    transport: T,
    pods: Arc<dyn PodConnector>,
    clock: Arc<dyn Clock>,
) -> Result<Running, Refusal> {
    let verifier = owner_verifier(deployment)?;
    let vllm_keys = crate::keys::vllm_keys(deployment)?;
    let inventory = inventory(deployment)?;
    let pool = Arc::new(crate::relaying::runpod_pool(deployment, transport, clock)?);
    let targets = PoolTargets::new(deployment, &vllm_keys, Arc::clone(&pool), pods)?;
    let mut models = Vec::with_capacity(deployment.models.len());
    for (alias, model) in &deployment.models {
        let wires = model.wires.iter().copied().map(relay_wire).collect();
        models.push(
            // The pod serves the model under its alias (`--served-model-name`), so the
            // upstream name is the alias too.
            RelayModel::new(relay_label(alias)?, relay_label(alias)?, wires).map_err(|error| {
                Refusal::new(
                    StartupRefusal::ConfigValue,
                    format!("models.{alias}: {error}"),
                )
            })?,
        );
    }
    let relay = Relay::new(models, Arc::new(targets))
        .map_err(|error| Refusal::new(StartupRefusal::ConfigValue, format!("models: {error}")))?;
    let signals = install_signals()?;
    let bind = deployment.gateway.bind;
    let handle = Gateway::bind_with_relay(
        deployment.gateway.clone(),
        Arc::new(verifier),
        inventory,
        relay,
    )
    .map_err(|error| Refusal::new(StartupRefusal::ListenBind, format!("{bind}: {error}")))?;
    // One pass before serving: it sweeps the pods a previous run of this controller left.
    let _swept = pool.reap();
    let cleanup = Cleanup::start(move || {
        // A refused pass changes nothing; the next one runs on time.
        let _report = pool.reap();
    });
    handle.mark_ready();
    Ok(Running {
        handle,
        signals,
        vllm_keys,
        cleanup: Some(cleanup),
        relaying: true,
    })
}
