//! Adversarial pass 1 over `story:pod-proxy-tls`, the cases that fail against the change: the
//! bound on opening a pod connection, and the binary programs the documented release build
//! makes. The guards of the same pass are in `adversary_w07_pod_tls.rs`. Every pod here is a
//! loopback fixture; nothing leaves the loopback interface.

mod support;

use llm_gateway_cli::{PodConnector, ProxyConnector};
use std::{
    net::{TcpListener, TcpStream},
    path::Path,
    process::Command,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use support::tls_pod::{POD_HOST, TestRoot};

/// spec/domains/deployment.yaml (the pod endpoint rule) and docs/hosting.md: "A pod connection
/// gives up connecting after 10 s", and the handshake "completes inside the opening of the
/// connection". A pod endpoint that accepts the TCP connection and never answers the client
/// hello must end the relay's `connect` within that bound (here 10 s and 5 s of slack), not hold
/// the request for the 120 s read timeout, once per byte a slow endpoint drips.
#[test]
fn adversary_w07_a_silent_handshake_gives_up_within_the_connect_bound() {
    let root = TestRoot::new("adversary-w07 silent root");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let held = Arc::new(Mutex::new(Vec::<TcpStream>::new()));
    let holding = Arc::clone(&held);
    thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(connection) = connection else { return };
            // Accepted and kept open; nothing is ever written back.
            holding.lock().unwrap().push(connection);
        }
    });
    let connector =
        ProxyConnector::with_test_seam(std::slice::from_ref(&root.der), Some(address)).unwrap();
    let (done, finished) = mpsc::channel();
    let started = Instant::now();
    thread::spawn(move || {
        let opened = connector.connect(POD_HOST).is_ok();
        done.send(opened).ok();
    });
    match finished.recv_timeout(Duration::from_secs(15)) {
        Ok(opened) => assert!(!opened, "a silent endpoint opened a connection"),
        Err(error) => panic!(
            "the relay's connect to a silent pod endpoint was still waiting after {:?} \
             ({error}); the spec bounds connecting at 10 s",
            started.elapsed()
        ),
    }
}

fn workspace_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cargo(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO"))
        .args(args)
        .current_dir(workspace_root())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "cargo {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// The build the README (line 63) and the getting-started page document,
/// `cargo build --release --locked -p b10x-llm-gateway-cli`, builds every binary target of the
/// package that no required feature gates. None of them may be a program that takes its trust
/// root and its pod address from the environment: such a binary lands in `target/release`
/// beside `b10x-llm-gateway`, runs the shipped `main`, and trusts whatever root
/// `GATEWAY_TLS_SEAM_ROOT` names.
#[test]
fn adversary_w07_the_documented_release_build_builds_no_seam_program() {
    let metadata: serde_json::Value = serde_json::from_str(&cargo(&[
        "metadata",
        "--offline",
        "--locked",
        "--no-deps",
        "--format-version",
        "1",
    ]))
    .unwrap();
    let package = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["name"] == "b10x-llm-gateway-cli")
        .unwrap();
    let mut built = Vec::new();
    for target in package["targets"].as_array().unwrap() {
        let is_bin = target["kind"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"));
        let gated = target["required-features"]
            .as_array()
            .is_some_and(|features| !features.is_empty());
        if !is_bin || gated {
            continue;
        }
        let source = std::fs::read_to_string(target["src_path"].as_str().unwrap()).unwrap();
        built.push(target["name"].as_str().unwrap().to_owned());
        assert!(
            !source.contains("with_test_seam"),
            "the documented release build builds {:?}, which trusts a root from its environment",
            target["name"]
        );
    }
    assert!(
        built.iter().any(|name| name == "b10x-llm-gateway"),
        "control: {built:?}"
    );
}
