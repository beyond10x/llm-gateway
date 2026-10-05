//! Composing the gateway from a loaded deployment, serving it, and stopping on a signal
//! (rows C1 and D2).

use crate::{
    config::Deployment,
    refusal::{Refusal, StartupRefusal},
    trusted,
};
use llm_gateway::{
    AuthKind, BillingKind, Gateway, GatewayHandle, Label, OwnerToken, RouteInventory, RouteSummary,
    SharedSecretVerifier, ShutdownReport, TargetLimits, TargetProvenance, TargetSummary,
};
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    iterator::Signals,
};
use std::{fmt, net::SocketAddr, sync::Arc};

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
    let token = OwnerToken::new(text.trim_end().as_bytes().to_vec()).map_err(|error| {
        Refusal::new(
            StartupRefusal::OwnerSecretNotAToken,
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

/// A gateway that is bound, ready, and has its stop signals installed.
pub struct Running {
    handle: GatewayHandle,
    signals: Signals,
}

impl Running {
    pub fn local_addr(&self) -> SocketAddr {
        self.handle.local_addr()
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
        Stopped {
            signal,
            report: self.handle.shutdown(),
        }
    }
}

/// Composes and starts the gateway the deployment describes. The stop signals are installed
/// before the listener is bound, so a signal that arrives once the gateway is reachable always
/// takes the graceful path.
///
/// # Errors
/// An `owner-secret:*`, `config:value`, `signal:install` or `listen:bind` [`Refusal`].
pub fn start(deployment: &Deployment) -> Result<Running, Refusal> {
    let verifier = owner_verifier(deployment)?;
    let inventory = inventory(deployment)?;
    let signals = Signals::new([SIGINT, SIGTERM])
        .map_err(|error| Refusal::new(StartupRefusal::SignalInstall, error.to_string()))?;
    let bind = deployment.gateway.bind;
    let handle = Gateway::bind(deployment.gateway.clone(), Arc::new(verifier), inventory)
        .map_err(|error| Refusal::new(StartupRefusal::ListenBind, format!("{bind}: {error}")))?;
    handle.mark_ready();
    Ok(Running { handle, signals })
}
