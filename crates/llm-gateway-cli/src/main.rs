#![forbid(unsafe_code)]

//! `b10x-llm-gateway`: one closed TOML deployment document, one gateway, a clean stop.

use clap::Parser;
use llm_gateway_cli::cli::Cli;
use std::{
    fmt,
    io::{self, Write},
    process::ExitCode,
};

/// Writes one line to standard error. A standard error that is gone (a log reader that exited)
/// changes nothing: the line is lost, and the exit status still says what happened.
fn report(line: fmt::Arguments<'_>) {
    drop(writeln!(io::stderr(), "b10x-llm-gateway: {line}"));
}

fn main() -> ExitCode {
    let args = Cli::parse();
    // Events (row O3) go to standard error beside the lines `report` writes, which they leave
    // unchanged; at the default level they are one `usage` event per authenticated model call.
    llm_gateway_cli::install_logging();
    let running = match llm_gateway_cli::load(&args.config).and_then(|deployment| {
        tracing::debug!(
            models = deployment.models.len(),
            digest = %deployment.digest,
            "deployment loaded"
        );
        llm_gateway_cli::start(&deployment)
    }) {
        Ok(running) => running,
        Err(refusal) => {
            report(format_args!("refused {refusal}"));
            return ExitCode::FAILURE;
        }
    };
    report(format_args!("listening on {}", running.local_addr()));
    let stopped = running.wait_for_stop();
    report(format_args!(
        "stopped by {} accepted={} completed={}",
        stopped.signal, stopped.report.accepted, stopped.report.completed
    ));
    ExitCode::SUCCESS
}
