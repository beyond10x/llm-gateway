//! Adversary pass on story:gateway-observability (row O3). `spec/domains/deployment.yaml` says a
//! `RUST_LOG` that cannot be read means `info`, that the process's published lines stay as
//! written, and that at the default level the shipped binary writes only those lines.
//! `logging.rs` says a directive it cannot read is dropped. Nothing leaves the loopback
//! interface.

mod support;

use rustix::process::{Pid, Signal, kill_process};
use std::{
    io::{BufRead, BufReader, Read},
    path::PathBuf,
    process::{Command, Stdio},
};
use support::Fixture;

const LISTENING: &str = "b10x-llm-gateway: listening on ";

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
}

#[test]
fn o3_an_unreadable_rust_log_is_info_and_adds_no_line_to_standard_error() {
    let fixture = Fixture::new("o3-unreadable-rust-log");
    let config = fixture.valid_config("127.0.0.1:0");
    let mut child = Command::new(binary())
        .args(["--config", config.to_str().unwrap()])
        // A level name that does not exist: the directive cannot be read.
        .env("RUST_LOG", "llm_gateway_cli=loud")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        assert!(
            stderr.read_line(&mut line).unwrap() > 0,
            "the gateway exited before listening: {lines:?}"
        );
        let line = line.trim_end().to_owned();
        let listening = line.starts_with(LISTENING);
        lines.push(line);
        if listening {
            break;
        }
    }
    kill_process(Pid::from_child(&child), Signal::TERM).unwrap();
    let mut rest = String::new();
    stderr.read_to_string(&mut rest).unwrap();
    lines.extend(rest.lines().map(str::to_owned));
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(0), "{lines:?}");
    // Exactly the published lines, as with RUST_LOG unset: `listening on`, then `stopped by`.
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].starts_with(LISTENING), "{lines:?}");
    assert!(
        lines[1].starts_with("b10x-llm-gateway: stopped by SIGTERM accepted="),
        "{lines:?}"
    );
}
