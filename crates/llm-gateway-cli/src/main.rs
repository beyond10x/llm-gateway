#![forbid(unsafe_code)]

//! `b10x-llm-gateway`: one closed TOML deployment document, one gateway, a clean stop.
//!
//! Every pod connection is TLS verified against the roots compiled into the binary
//! (`llm_gateway_cli::ProxyConnector::shipped`); nothing here reads another root.

use llm_gateway_cli::ProxyConnector;
use std::process::ExitCode;

fn main() -> ExitCode {
    llm_gateway_cli::main_with(ProxyConnector::shipped())
}
