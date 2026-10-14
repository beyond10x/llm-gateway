#![forbid(unsafe_code)]

//! `gateway-tls-seam`: the shipped `b10x-llm-gateway` `main` with the TLS test seam
//! (spec/domains/deployment.yaml). It is a test helper for `tests/pod_tls.rs`, never installed:
//! the shipped binary reads neither variable below and has no other way to the seam.
//!
//! * `GATEWAY_TLS_SEAM_ROOT`: a file holding one DER certificate, the only root trusted.
//! * `GATEWAY_TLS_SEAM_POD`: a loopback `address:port` every pod host is connected at, while the
//!   certificate is still verified for the pod's own host name.

use llm_gateway_cli::ProxyConnector;
use std::{
    env, fs,
    io::{self, Write},
    net::SocketAddr,
    process::ExitCode,
};

fn seam() -> Result<ProxyConnector, String> {
    let root = env::var_os("GATEWAY_TLS_SEAM_ROOT").ok_or("GATEWAY_TLS_SEAM_ROOT is not set")?;
    let root = fs::read(&root).map_err(|error| format!("GATEWAY_TLS_SEAM_ROOT: {error}"))?;
    let pod = match env::var("GATEWAY_TLS_SEAM_POD") {
        Ok(pod) => Some(
            pod.parse::<SocketAddr>()
                .map_err(|error| format!("GATEWAY_TLS_SEAM_POD: {error}"))?,
        ),
        Err(_) => None,
    };
    ProxyConnector::with_test_seam(&[root], pod).map_err(|error| format!("the seam: {error}"))
}

fn main() -> ExitCode {
    match seam() {
        Ok(pods) => llm_gateway_cli::main_with(pods),
        Err(error) => {
            drop(writeln!(io::stderr(), "gateway-tls-seam: {error}"));
            ExitCode::FAILURE
        }
    }
}
