//! Structured logs (row O3): `tracing` events on standard error, at the level `RUST_LOG` sets,
//! `info` by default, and one `info` event per usage record the gateway hands out.
//!
//! The process's own lines (`listening on`, `refused`, `stopped by`, `spec/domains/deployment.yaml`)
//! are written as before and are not events, so they stay byte-identical at every level.

use llm_gateway::{Disposition, UsageRecord, UsageRecords};
use std::io;
use tracing_subscriber::{
    EnvFilter,
    filter::{Directive, LevelFilter},
};

/// The filter for a `RUST_LOG` value: its directives, with `info` for every target it does not
/// name. Unset, empty or unparsable, it is `info`. A directive it cannot read is dropped
/// silently rather than refusing to start: each is read on its own first, so only readable ones
/// reach the filter, which therefore has nothing to warn about on standard error, where only
/// the process's published lines and its events belong.
pub fn log_filter(rust_log: Option<&str>) -> EnvFilter {
    let readable: Vec<&str> = rust_log
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|directive| !directive.is_empty() && directive.parse::<Directive>().is_ok())
        .collect();
    EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .parse_lossy(readable.join(","))
}

/// Installs the process's subscriber: events on standard error without colour, filtered by
/// [`log_filter`] of `RUST_LOG`. A subscriber already installed is kept.
pub fn install_logging() {
    let rust_log = std::env::var("RUST_LOG").ok();
    let _installed = tracing_subscriber::fmt()
        .with_env_filter(log_filter(rust_log.as_deref()))
        .with_ansi(false)
        .with_writer(io::stderr)
        .try_init();
}

/// Writes each usage record as one structured `info` event, target `usage`. A field the record
/// does not carry is left out of the event.
#[derive(Debug, Clone, Copy, Default)]
pub struct TracingRecords;

const fn disposition(disposition: Disposition) -> &'static str {
    match disposition {
        Disposition::Relayed => "Relayed",
        Disposition::Refused => "Refused",
        Disposition::UpstreamFailed => "UpstreamFailed",
    }
}

impl UsageRecords for TracingRecords {
    fn record(&self, record: &UsageRecord) {
        tracing::info!(
            target: "usage",
            model = record.model.as_deref(),
            wire = %record.wire.label(),
            disposition = %disposition(record.disposition),
            refusal = record.refusal.map(|code| tracing::field::display(code.wire())),
            status = record.status,
            target_status = record.target_status,
            response_bytes = record.response_bytes,
            duration_ms = record.duration_ms,
            "usage record"
        );
    }
}
