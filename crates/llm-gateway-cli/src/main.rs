#![forbid(unsafe_code)]

//! `b10x-llm-gateway`: one closed TOML deployment document, one gateway, a clean stop.

use clap::Parser;
use std::{path::PathBuf, process::ExitCode};

const PREFIX: &str = "b10x-llm-gateway:";

/// The authenticated single-owner llm-gateway, configured by one closed TOML document.
#[derive(Debug, Parser)]
#[command(name = "b10x-llm-gateway", version)]
struct Args {
    /// Path to the closed TOML deployment document.
    #[arg(long)]
    config: PathBuf,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let running = match llm_gateway_cli::load(&args.config)
        .and_then(|deployment| llm_gateway_cli::start(&deployment))
    {
        Ok(running) => running,
        Err(refusal) => {
            eprintln!("{PREFIX} refused {refusal}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("{PREFIX} listening on {}", running.local_addr());
    let stopped = running.wait_for_stop();
    eprintln!(
        "{PREFIX} stopped by {} accepted={} completed={}",
        stopped.signal, stopped.report.accepted, stopped.report.completed
    );
    ExitCode::SUCCESS
}
