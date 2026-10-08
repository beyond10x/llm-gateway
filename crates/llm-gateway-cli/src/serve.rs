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
    TokenError,
};
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    iterator::Signals,
};
use std::{collections::BTreeMap, fmt, net::SocketAddr, sync::Arc};

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

/// The largest vLLM key, after trailing ASCII whitespace is trimmed: the owner token's bound.
const MAX_VLLM_KEY_BYTES: usize = 4096;

/// One model's vLLM key (row B9). It is redacted in `Debug`, has no `Display` and no `Clone`,
/// and its bytes are overwritten when it is dropped.
pub struct VllmKey(Vec<u8>);

impl VllmKey {
    /// The key as the pod's vLLM server expects it: one printable ASCII token.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for VllmKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VllmKey([REDACTED])")
    }
}

impl Drop for VllmKey {
    fn drop(&mut self) {
        // Best effort without a `zeroize` dependency, as `OwnerToken` does.
        self.0.fill(0);
        std::hint::black_box(&self.0);
    }
}

/// Every model's vLLM key, by alias. A model whose declaration names no `vllm_api_key_file` has
/// none. `Debug` prints the aliases and no value.
#[derive(Default)]
pub struct VllmKeys(BTreeMap<String, VllmKey>);

impl VllmKeys {
    pub fn get(&self, alias: &str) -> Option<&VllmKey> {
        self.0.get(alias)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for VllmKeys {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_map().entries(self.0.iter()).finish()
    }
}

/// Reads each model's `vllm_api_key_file` once through the trusted-file reader.
///
/// # Errors
/// A `vllm-api-key:*` [`Refusal`] for the first file, in alias order, that breaks a rule. The
/// message names the model and the file, never the bytes.
pub fn vllm_keys(deployment: &Deployment) -> Result<VllmKeys, Refusal> {
    let mut keys = BTreeMap::new();
    for (alias, model) in &deployment.models {
        let Some(path) = &model.vllm_api_key_file else {
            continue;
        };
        let refuse = |kind, rule: &str| {
            Refusal::new(
                kind,
                format!(
                    "models.{alias}.vllm_api_key_file {}: {rule}",
                    path.display()
                ),
            )
        };
        let mut read = trusted::read(path, &trusted::VLLM_API_KEY)?.into_bytes();
        // ASCII whitespace only: a trailing newline or CRLF, never a non-ASCII character.
        let end = read
            .iter()
            .rposition(|byte| !byte.is_ascii_whitespace())
            .map_or(0, |last| last + 1);
        let material = &read[..end];
        let key = if material.len() > MAX_VLLM_KEY_BYTES {
            Err(refuse(
                StartupRefusal::VllmApiKeyTooLarge,
                &format!("the key exceeds {MAX_VLLM_KEY_BYTES} bytes"),
            ))
        } else if material.is_empty() || !material.iter().all(u8::is_ascii_graphic) {
            Err(refuse(
                StartupRefusal::VllmApiKeyNotAToken,
                "the key must be one non-empty printable ASCII token",
            ))
        } else {
            Ok(VllmKey(material.to_vec()))
        };
        // The bytes read are overwritten whichever way the rules went.
        read.fill(0);
        std::hint::black_box(&read);
        keys.insert(alias.clone(), key?);
    }
    Ok(VllmKeys(keys))
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
    vllm_keys: VllmKeys,
}

impl Running {
    pub fn local_addr(&self) -> SocketAddr {
        self.handle.local_addr()
    }

    /// Each model's vLLM key, read once at startup.
    pub fn vllm_keys(&self) -> &VllmKeys {
        &self.vllm_keys
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
/// An `owner-secret:*`, `vllm-api-key:*`, `config:value`, `signal:install` or `listen:bind`
/// [`Refusal`].
pub fn start(deployment: &Deployment) -> Result<Running, Refusal> {
    let verifier = owner_verifier(deployment)?;
    let vllm_keys = vllm_keys(deployment)?;
    let inventory = inventory(deployment)?;
    let signals = Signals::new([SIGINT, SIGTERM])
        .map_err(|error| Refusal::new(StartupRefusal::SignalInstall, error.to_string()))?;
    let bind = deployment.gateway.bind;
    let handle = Gateway::bind(deployment.gateway.clone(), Arc::new(verifier), inventory)
        .map_err(|error| Refusal::new(StartupRefusal::ListenBind, format!("{bind}: {error}")))?;
    handle.mark_ready();
    Ok(Running {
        handle,
        signals,
        vllm_keys,
    })
}
