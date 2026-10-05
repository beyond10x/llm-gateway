#![forbid(unsafe_code)]

//! `b10x-llm-gateway`: one closed TOML deployment document, one gateway, a clean stop.

use clap::Parser;
use std::{
    fmt,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

/// The authenticated single-owner llm-gateway, configured by one closed TOML document.
#[derive(Debug, Parser)]
#[command(name = "b10x-llm-gateway", version)]
struct Args {
    /// Path to the closed TOML deployment document.
    #[arg(long)]
    config: PathBuf,
}

/// Writes one line to standard error. A standard error that is gone (a log reader that exited)
/// changes nothing: the line is lost, and the exit status still says what happened.
fn report(line: fmt::Arguments<'_>) {
    drop(writeln!(io::stderr(), "b10x-llm-gateway: {line}"));
}

fn main() -> ExitCode {
    let args = Args::parse();
    let running = match llm_gateway_cli::load(&args.config)
        .and_then(|deployment| llm_gateway_cli::start(&deployment))
    {
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
