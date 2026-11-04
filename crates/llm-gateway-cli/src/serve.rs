//! Composing the gateway from a loaded deployment, serving it, and stopping on a signal
//! (rows C1 and D2).

use crate::{
    config::Deployment,
    refusal::{Refusal, StartupRefusal},
    trusted,
};
use llm_gateway::{
    AuthKind, BillingKind, GatewayHandle, IngressAuth, Label, NoOwnerVerifier, OwnerToken,
    OwnerVerifier, RouteInventory, RouteSummary, SharedSecretVerifier, ShutdownReport,
    TargetLimits, TargetProvenance, TargetSummary, TokenError,
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

/// Reads the owner secret through the trusted-file reader and builds the owner verifier; with no
/// `owner_secret_file` (`ingress_auth = "none"`) it is [`NoOwnerVerifier`], accepting nothing.
///
/// # Errors
/// An `owner-secret:*` [`Refusal`], its message starting `owner_secret_file:`, never the path.
pub fn owner_verifier(deployment: &Deployment) -> Result<Arc<dyn OwnerVerifier>, Refusal> {
    let Some(path) = &deployment.owner_secret_file else {
        return Ok(Arc::new(NoOwnerVerifier));
    };
    let text = trusted::read_named(path, OWNER_SECRET_FILE, &trusted::OWNER_SECRET)?;
    let text = trusted::Wiped::adopt(text.into_bytes());
    // ASCII whitespace only: a trailing newline or CRLF, never a non-ASCII character.
    let token = OwnerToken::new(text.as_slice().trim_ascii_end().to_vec()).map_err(|error| {
        Refusal::new(
            // The token bound is applied here, after trimming; the file bound is the reader's.
            if error == TokenError::TooLarge {
                StartupRefusal::OwnerSecretTooLarge
            } else {
                StartupRefusal::OwnerSecretNotAToken
            },
            format!("{OWNER_SECRET_FILE}: {error}"),
        )
    })?;
    let verifier = SharedSecretVerifier::new(token).map_err(|error| {
        Refusal::new(
            StartupRefusal::OwnerSecretTooShort,
            format!("{OWNER_SECRET_FILE}: {error}"),
        )
    })?;
    Ok(Arc::new(verifier))
}

/// Reads the metrics listener's scrape token through the trusted-file reader, under the
/// endpoint credential's rules; `None` without `metrics_bearer_file`.
///
/// # Errors
/// A `metrics-bearer:*` [`Refusal`], its message starting `metrics_bearer_file:`, never the path.
pub fn metrics_bearer(deployment: &Deployment) -> Result<Option<OwnerToken>, Refusal> {
    let Some(path) = &deployment.metrics_bearer_file else {
        return Ok(None);
    };
    let material = crate::keys::read_token(
        path,
        "metrics_bearer_file",
        &trusted::METRICS_BEARER,
        (
            StartupRefusal::MetricsBearerTooLarge,
            StartupRefusal::MetricsBearerNotAToken,
        ),
    )?;
    // Held to the owner secret's floor: on a cluster that enforces no network policy this token
    // is the metrics listener's only guard. `scrape_token` holds the check and sits at the end
    // of this file, so that every line below keeps its number:
    // docs/llmgw-capability-matrix.md and spec/domains/upstream.yaml cite them by line
    // (`adv_b9_citations_into_serve_rs_still_land`).
    scrape_token(material).map(Some)
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

/// The inspection inventory: one route per model alias, Runpod or hosted, one target per wire,
/// under the digest of the exact document bytes.
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
    RouteInventory::new(crate::hosted::routes(deployment, routes)?)
        .and_then(|inventory| inventory.with_config_digest(&deployment.digest))
        .map_err(|error| inventory_refusal(&error))
}

/// A gateway that is bound, ready, and has its stop signals installed.
pub struct Running {
    handle: GatewayHandle,
    signals: Signals,
    vllm_keys: crate::keys::VllmKeys,
}

impl Running {
    pub fn local_addr(&self) -> SocketAddr {
        self.handle.local_addr()
    }

    /// Each model's vLLM key, read once at startup.
    pub fn vllm_keys(&self) -> &crate::keys::VllmKeys {
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

/// Composes and starts the gateway as the shipped binary does: [`start_with`] the shipped pod
/// connector, [`crate::ProxyConnector::shipped`], which reaches each pod over TLS verified
/// against the roots compiled into the binary.
///
/// # Errors
/// An `owner-secret:*`, `vllm-api-key:*`, `config:value`, `signal:install` or `listen:bind`
/// [`Refusal`].
pub fn start(deployment: &Deployment) -> Result<crate::Relaying, Refusal> {
    start_with(deployment, crate::ProxyConnector::shipped())
}

/// [`crate::start_connected`] over `ConnectorsRunpod` itself and `pods`. Signals are installed
/// before binding. [`start`] passes the shipped connector; only tests pass one built by
/// [`crate::ProxyConnector::with_test_seam`].
///
/// # Errors
/// As [`start`].
pub fn start_with(
    deployment: &Deployment,
    pods: crate::ProxyConnector,
) -> Result<crate::Relaying, Refusal> {
    crate::start_connected(deployment, |transport| transport, std::sync::Arc::new(pods))
}

/// The line written, after the `b10x-llm-gateway: ` prefix and before `listening on`, when the
/// document turns ingress authentication off (story:ingress-auth-optional).
pub const INGRESS_AUTH_OFF: &str = "warning ingress auth is off (ingress_auth = \"none\"): model calls are served without a credential";

/// Writes one line to standard error. A standard error that is gone (a log reader that exited)
/// changes nothing: the line is lost, and the exit status still says what happened.
pub(crate) fn report(line: fmt::Arguments<'_>) {
    use std::io::Write as _;
    drop(writeln!(std::io::stderr(), "b10x-llm-gateway: {line}"));
}

/// A usage error of a subcommand: its kind and its help line, never an argument (clap quotes
/// the offending one, which may be a key). Help and version exit as clap prints them.
pub(crate) fn usage_error(error: &clap::Error, command: &str) -> std::process::ExitCode {
    use clap::error::ErrorKind;
    if matches!(
        error.kind(),
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
    ) {
        error.exit();
    }
    report(format_args!(
        "usage error ({:?}); see `b10x-llm-gateway {command} --help`",
        error.kind()
    ));
    std::process::ExitCode::from(2)
}

/// The document `--config` names, else the default document ([`crate::cli::default_config`]).
///
/// # Errors
/// `config:unreadable` when there is no `--config` and neither `XDG_CONFIG_HOME` nor `HOME` is
/// set.
pub(crate) fn document_path(
    config: Option<std::path::PathBuf>,
) -> Result<std::path::PathBuf, Refusal> {
    config.or_else(crate::cli::default_config).ok_or_else(|| {
        Refusal::new(
            StartupRefusal::ConfigUnreadable,
            "no --config and no default document: neither XDG_CONFIG_HOME nor HOME is set"
                .to_owned(),
        )
    })
}

/// The `b10x-llm-gateway` process over `pods`: with `credential` as the first argument, the
/// `credential set` command, which reaches `https` endpoints over `pods` too; with `watchdog`,
/// the pod watchdog; with `serve`, the serving command line reading the default document unless
/// `--config` names one; otherwise the closed serving command line. Serving installs logging,
/// loads the document, [`start_with`] `pods`, serves until SIGINT or SIGTERM, and stops
/// gracefully, writing the lines `spec/domains/deployment.yaml` names. The shipped binary's
/// `main` passes [`crate::ProxyConnector::shipped`]; only the `gateway-tls-seam` test program
/// passes another.
pub fn main_with(pods: crate::ProxyConnector) -> std::process::ExitCode {
    use crate::cli::Invocation;
    use clap::Parser as _;
    match Invocation::from_args(std::env::args_os().collect()) {
        Invocation::Credential(args) => crate::credential::main(args, &pods),
        Invocation::Watchdog(args) => match crate::cli::WatchdogCli::try_parse_from(args) {
            Ok(cli) => crate::watchdog::main(&cli, &pods),
            Err(error) => usage_error(&error, "watchdog"),
        },
        Invocation::ServeCommand(args) => match crate::cli::ServeCli::try_parse_from(args) {
            Ok(cli) => match document_path(cli.config) {
                Ok(config) => serve_document(&config, pods),
                Err(refusal) => {
                    report(format_args!("refused {refusal}"));
                    std::process::ExitCode::FAILURE
                }
            },
            Err(error) => usage_error(&error, "serve"),
        },
        Invocation::Serve => serve_closed_line(pods),
    }
}

/// The closed serving command line, parsed exactly as in every earlier release.
fn serve_closed_line(pods: crate::ProxyConnector) -> std::process::ExitCode {
    use clap::{CommandFactory as _, Parser as _};
    let args = match crate::cli::Cli::try_parse() {
        Ok(args) => args,
        Err(error) => {
            use clap::error::ErrorKind;
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                error.exit();
            }
            // clap quotes the offending argument, which may be a key: name only the kind and
            // the usage line.
            report(format_args!(
                "usage error ({:?}); {}; see `b10x-llm-gateway --help`",
                error.kind(),
                crate::cli::Cli::command().render_usage()
            ));
            return std::process::ExitCode::from(2);
        }
    };
    serve_document(&args.config, pods)
}

/// The heartbeat file of a document with `[watchdog]`; `None` without one.
///
/// # Errors
/// `config:value` naming `watchdog.heartbeat_file` when none is declared and neither
/// `XDG_STATE_HOME` nor `HOME` is set.
pub(crate) fn heartbeat_file(
    deployment: &Deployment,
) -> Result<Option<std::path::PathBuf>, Refusal> {
    deployment
        .watchdog
        .as_ref()
        .map(|config| {
            crate::heartbeat::path_of(config).ok_or_else(|| {
                Refusal::new(
                    StartupRefusal::ConfigValue,
                    "watchdog.heartbeat_file: none declared, and neither XDG_STATE_HOME nor HOME is set"
                        .to_owned(),
                )
            })
        })
        .transpose()
}

/// Starts the heartbeat at `file` with `activity`, every `heartbeat_seconds` of the document's
/// `[watchdog]`; `None` without a file.
pub(crate) fn heartbeat_of(
    deployment: &Deployment,
    file: Option<std::path::PathBuf>,
    activity: crate::heartbeat::ActivitySource,
) -> Option<crate::heartbeat::Beating> {
    let config = deployment.watchdog.as_ref()?;
    Some(crate::heartbeat::start(
        file?,
        std::time::Duration::from_secs(config.heartbeat_seconds),
        crate::CONTROLLER,
        activity,
    ))
}

/// Serves the document at `config` until SIGINT or SIGTERM.
fn serve_document(config: &std::path::Path, pods: crate::ProxyConnector) -> std::process::ExitCode {
    // Events (row O3) go to standard error beside the lines `report` writes, which they leave
    // unchanged; at the default level they are one `usage` event per authenticated model call.
    crate::install_logging();
    let (running, beating) = match crate::load(config).and_then(|deployment| {
        tracing::debug!(
            models = deployment.models.len(),
            digest = %deployment.digest,
            "deployment loaded"
        );
        // Before the listener is bound, so no model call is served before the warning is out.
        if deployment.gateway.ingress_auth == IngressAuth::None {
            report(format_args!("{INGRESS_AUTH_OFF}"));
        }
        let heartbeat = heartbeat_file(&deployment)?;
        let running = start_with(&deployment, pods)?;
        Ok((
            running,
            heartbeat_of(&deployment, heartbeat, crate::heartbeat::no_activity()),
        ))
    }) {
        Ok(started) => started,
        Err(refusal) => {
            report(format_args!("refused {refusal}"));
            return std::process::ExitCode::FAILURE;
        }
    };
    report(format_args!("listening on {}", running.local_addr()));
    let stopped = running.wait_for_stop();
    // The heartbeat ends with the gateway; its last beat stays and goes stale, so the watchdog
    // stops the pods unless a gateway beats again within `stale_minutes`.
    drop(beating);
    report(format_args!(
        "stopped by {} accepted={} completed={}",
        stopped.signal, stopped.report.accepted, stopped.report.completed
    ));
    std::process::ExitCode::SUCCESS
}

impl Running {
    /// Drains and stops the gateway gracefully now, without waiting for a signal.
    pub fn shutdown(self) -> ShutdownReport {
        self.handle.begin_drain();
        self.handle.shutdown()
    }

    /// A running gateway from its parts, for the relay composition in `relaying.rs`.
    pub(crate) fn from_parts(
        handle: GatewayHandle,
        signals: Signals,
        vllm_keys: crate::keys::VllmKeys,
    ) -> Self {
        Self {
            handle,
            signals,
            vllm_keys,
        }
    }

    /// Blocks until SIGINT or SIGTERM and names it, as [`Self::wait_for_stop`] does, without
    /// stopping anything.
    pub(crate) fn wait_for_signal(&mut self) -> StopSignal {
        self.signals
            .forever()
            .find_map(|signal| match signal {
                SIGINT => Some(StopSignal::Sigint),
                SIGTERM => Some(StopSignal::Sigterm),
                _ => None,
            })
            .unwrap_or(StopSignal::Sigterm)
    }

    /// Installs the stop signals, as [`start`] does.
    pub(crate) fn install_signals() -> Result<Signals, Refusal> {
        Signals::new([SIGINT, SIGTERM])
            .map_err(|error| Refusal::new(StartupRefusal::SignalInstall, error.to_string()))
    }
}

/// The shortest scrape token `metrics_bearer_file` may hold, the owner secret's floor.
const MIN_METRICS_BEARER_BYTES: usize = 32;

/// The setting naming the owner secret, which its refusals name in place of the path.
const OWNER_SECRET_FILE: &str = "owner_secret_file";

/// `material`, read from `metrics_bearer_file`, as the metrics listener's scrape token: refused
/// `metrics-bearer:not-a-token` under [`MIN_METRICS_BEARER_BYTES`] or when it is not a token.
fn scrape_token(mut material: Vec<u8>) -> Result<OwnerToken, Refusal> {
    let refuse = |reason: &dyn fmt::Display| {
        Refusal::new(
            StartupRefusal::MetricsBearerNotAToken,
            format!("metrics_bearer_file: {reason}"),
        )
    };
    if material.len() < MIN_METRICS_BEARER_BYTES {
        // As the reader does with what it read: the refused bytes are overwritten.
        material.fill(0);
        std::hint::black_box(&material);
        return Err(refuse(&format_args!(
            "the token must be at least {MIN_METRICS_BEARER_BYTES} bytes"
        )));
    }
    OwnerToken::new(material).map_err(|error| refuse(&error))
}

#[cfg(test)]
mod tests {
    use super::owner_verifier;
    use crate::{config, trusted::ledger};
    use std::{fs, os::unix::fs::PermissionsExt, path::Path};

    /// What this proves: the owner secret file's text was held only in the allocation the
    /// trusted-file reader took for it, which read zero in full, spare capacity included, as it
    /// was let go, whether the secret was taken or refused, and none was freed by a reallocation
    /// behind its back. That freed memory stays zero is not observable from safe Rust.
    #[test]
    fn the_owner_secret_files_text_is_overwritten_whichever_way_the_rules_go() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!(
                "gateway-cli-unit-serve-owner-{}",
                std::process::id()
            ));
        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).unwrap();
        let secret = dir.join("owner");
        let deployment = config::parse(&format!(
            "listen = \"127.0.0.1:0\"\nowner_secret_file = \"{}\"\n\n[providers.runpod]\n\
             kind = \"runpod-vllm\"\n\n[models.small]\nprovider = \"runpod\"\n\
             wires = [\"chat\"]\ncontext_window = 65536\nhf_model = \"example/small-model\"\n\
             image = \"vllm/vllm-openai:v0.27.1\"\ngpu_types = [\"NVIDIA L40S\"]\n\
             max_model_len = 1024\n",
            secret.display()
        ))
        .unwrap();
        for (name, bytes, taken) in [
            (
                "taken",
                &b"owner-secret-0123456789abcdef-0123456789\r\n"[..],
                true,
            ),
            ("short", b"owner-secret\n", false),
            (
                "spaced",
                b"owner secret 0123456789abcdef 0123456789\n",
                false,
            ),
        ] {
            fs::write(&secret, bytes).unwrap();
            fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
            ledger::drain();
            assert_eq!(owner_verifier(&deployment).is_ok(), taken, "{name}");
            ledger::assert_every_allocation_overwritten(name);
        }
    }
}
