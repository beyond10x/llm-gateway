//! Adversarial cases for `story:gateway-auth`.
//!
//! Each case drives the implementation against a statement the unit itself published, in
//! `docs/gateway.md`, `crates/llm-gateway/src/lib.rs` or `tests/dependency_boundary.rs`. Nothing
//! here edits an implementation file; the two mutation probes below are expressed as strings
//! inside the case that needs them.

use llm_gateway::{
    AuthKind, BillingKind, Gateway, GatewayConfig, GatewayHandle, Label, OwnerToken,
    RouteInventory, RouteSummary, SharedSecretVerifier, TargetLimits, TargetProvenance,
    TargetSummary,
};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::Arc,
    thread,
    time::Duration,
};

const OWNER_SECRET: &str = "owner-token-0123456789abcdef0123456789abcdef";

fn label(value: &str) -> Label {
    Label::new(value).unwrap()
}

fn target(provenance: TargetProvenance) -> TargetSummary {
    TargetSummary {
        target_id: label("coding-primary"),
        position: 0,
        provenance,
        auth_kind: AuthKind::Bearer,
        billing_kind: BillingKind::Metered,
        limits: TargetLimits::default(),
    }
}

fn provenance(account: &str, endpoint: &str) -> TargetProvenance {
    TargetProvenance {
        protocol: label("chat-completions"),
        provider: label("my-lab"),
        account: label(account),
        endpoint: label(endpoint),
        model: label("large"),
        binding_revision: label("rev-1"),
    }
}

fn inventory_with(provenance: TargetProvenance) -> RouteInventory {
    let route = RouteSummary::new(
        label("coding"),
        label("code"),
        false,
        vec![target(provenance)],
    )
    .unwrap();
    RouteInventory::new(vec![route]).unwrap()
}

fn bind(configure: impl FnOnce(&mut GatewayConfig)) -> GatewayHandle {
    let token = OwnerToken::new(OWNER_SECRET.as_bytes().to_vec()).unwrap();
    let verifier = Arc::new(SharedSecretVerifier::new(token).unwrap());
    let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
    config.read_timeout = Duration::from_secs(5);
    configure(&mut config);
    Gateway::bind(
        config,
        verifier,
        inventory_with(provenance("remote", "remote-models")),
    )
    .unwrap()
}

#[derive(Debug)]
struct Response {
    status: u16,
    body: String,
    raw: String,
}

fn parse_response(raw: &str) -> Response {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let status = head
        .split("\r\n")
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    Response {
        status,
        body: body.to_string(),
        raw: raw.to_string(),
    }
}

fn exchange(addr: SocketAddr, request: &[u8]) -> Response {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(request).unwrap();
    stream.flush().unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    parse_response(&String::from_utf8_lossy(&raw))
}

// ---------------------------------------------------------------------------------------------
// 1. docs/gateway.md:72 — "`/health` | none | Liveness. Always `200 {"status":"live"}` while the
//    process serves." and docs/gateway.md:77 — the probe surface is "matched before anything
//    else". The concurrency guard in src/server.rs:229 runs before both.
// ---------------------------------------------------------------------------------------------

#[test]
fn liveness_stays_two_hundred_while_the_process_serves_under_load() {
    let handle = bind(|config| config.max_concurrent_requests = 1);
    handle.mark_ready();
    let addr = handle.local_addr();

    // One connection occupies the single permitted slot: its head is not yet terminated, so the
    // gateway is still reading it. This is ordinary load, not a fault.
    let mut held = TcpStream::connect(addr).unwrap();
    held.write_all(b"GET /v1/routes HTTP/1.1\r\nhost: h\r\n")
        .unwrap();
    held.flush().unwrap();
    thread::sleep(Duration::from_millis(250));

    let probe = exchange(addr, b"GET /health HTTP/1.1\r\nhost: h\r\n\r\n");
    drop(held);

    assert_eq!(
        probe.status, 200,
        "docs/gateway.md:72 says /health is always 200 while the process serves; \
         a liveness probe that fails under load restarts the process. Got: {}",
        probe.raw
    );
    assert!(
        probe.body.contains("\"status\":\"live\""),
        "liveness answered with a refusal body: {}",
        probe.body
    );
}

// ---------------------------------------------------------------------------------------------
// 2. docs/gateway.md:118 — "`shutdown` then stops accepting, lets every accepted connection
//    finish". src/server.rs:147 stores `ready = false` at the top of `shutdown`, so a connection
//    already accepted, and counted in `in_flight_at_signal`, is answered `unavailable`.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_connection_accepted_before_shutdown_is_finished_rather_than_refused() {
    let handle = bind(|_| {});
    handle.mark_ready();
    let addr = handle.local_addr();

    let mut pending = TcpStream::connect(addr).unwrap();
    pending
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    pending
        .write_all(b"GET /v1/routes HTTP/1.1\r\nhost: h\r\n")
        .unwrap();
    pending.flush().unwrap();
    // The gateway has accepted it and is blocked reading the rest of the head.
    thread::sleep(Duration::from_millis(250));

    let stopper = thread::spawn(move || handle.shutdown());
    thread::sleep(Duration::from_millis(250));

    pending
        .write_all(format!("authorization: Bearer {OWNER_SECRET}\r\n\r\n").as_bytes())
        .unwrap();
    pending.flush().unwrap();
    let mut raw = Vec::new();
    pending.read_to_end(&mut raw).unwrap();
    let response = parse_response(&String::from_utf8_lossy(&raw));
    let report = stopper.join().unwrap();

    assert_eq!(
        response.status, 200,
        "docs/gateway.md:118 says a graceful stop lets every accepted connection finish; \
         this one was accepted before the signal ({report:?}) and was refused. Got: {}",
        response.raw
    );
}

// ---------------------------------------------------------------------------------------------
// Four cases from this pass were retired by the coordinator, not by the unit.
//
// Each asserted a property of a mechanism that the correction replaced, and three of them did so
// through a copy of the old mechanism kept inside this file — so no change to the implementation
// could satisfy them. Retiring them is the coordinator's call and is recorded in
// review-result:adversary-gateway-pass-1 and in docs/plan/wave-1.md. What each one established is
// still asserted, by a case that reads the shipped code rather than a copy of the replaced code:
//
//   - the declared-dependency scan reading past the first dependency
//     -> tests/dependency_boundary.rs, the_declared_dependency_scan_reads_the_whole_array,
//        with a positive control that fails if the scan goes blind
//   - the transitive closure being examined at all
//     -> tests/dependency_boundary.rs, the_transitive_closure_contains_nothing_that_can_resolve_
//        a_secret_or_provision, computed by cargo rather than parsed here
//   - a refusal code reaching a client without being declared
//     -> src/error.rs, the refusal_codes! macro: the defect is now a compile error
//   - an inspection response carrying an endpoint URL
//     -> src/inventory.rs, provenance_renders_exactly_the_bytes_the_composer_supplied. The
//        original case asserted that no URL can render. That is not true and is not required: the
//        rendering carries exactly the identifiers the composer supplied, and supplying a URL is
//        the composer's doing. The replacement pins that as a fact rather than leaving it to a
//        fixture's choice of opaque names.
// ---------------------------------------------------------------------------------------------
