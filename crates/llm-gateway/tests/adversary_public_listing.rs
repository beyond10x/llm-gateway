//! Adversary cases for `story:public-model-listing`, pass 1: `GET /` (row R1) against
//! acceptance 2, "anything else, or no `User-Agent`: plain text".

use llm_gateway::{
    Gateway, GatewayConfig, GatewayHandle, Label, OwnerToken, Relay, RelayModel, RelayTarget,
    RelayTargets, RouteInventory, SharedSecretVerifier, TargetRefusal, ToolCalling, Wire,
};
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::Arc,
    time::Duration,
};

const OWNER_SECRET: &str = "owner-token-adversary-listing-0123456789ab";

struct NoTargets;

impl RelayTargets for NoTargets {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        panic!("a public route asked the target source for a target");
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

fn gateway() -> GatewayHandle {
    let model = RelayModel::new(
        Label::new("code").unwrap(),
        Label::new("served-code").unwrap(),
        Wire::ALL.to_vec(),
    )
    .unwrap()
    .with_tool_calling(ToolCalling::Parsed)
    .with_context_window(65_536);
    let relay = Relay::new(vec![model], Arc::new(NoTargets)).unwrap();
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER_SECRET.as_bytes().to_vec()).unwrap())
            .unwrap(),
    );
    let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
    config.read_timeout = Duration::from_secs(5);
    let handle = Gateway::bind_with_relay(
        config,
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    handle.mark_ready();
    handle
}

/// The whole answer, as bytes, to one raw request head.
fn exchange(handle: &GatewayHandle, request: &[u8]) -> String {
    let mut stream = TcpStream::connect(handle.local_addr()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(request).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    String::from_utf8_lossy(&raw).into_owned()
}

/// RFC 9110 section 5.5 admits obs-text (0x80-0xFF) in a field value, but the gateway reads a
/// request head only as UTF-8 and refuses any other head as `request-malformed`, on every route
/// (`docs/gateway.md`, the causes of `request-malformed`). So a `user-agent` that is not UTF-8
/// never reaches the selection rules of `GET /`: it gets the documented refusal, not one of the
/// four answers. No client the setup text addresses sends one. If the head reader starts to admit
/// obs-text, this case must change to expect the plain-text answer for "anything else".
#[test]
fn adversary_r1_a_user_agent_that_is_not_utf8_is_request_malformed() {
    let handle = gateway();
    let request = b"GET / HTTP/1.1\r\nhost: gateway\r\nuser-agent: curl/8.0 (Z\xfcrich)\r\n\r\n";
    let answer = exchange(&handle, request);
    handle.shutdown();
    assert!(answer.starts_with("HTTP/1.1 400 "), "{answer}");
    assert!(
        answer.contains("\"code\":\"request-malformed\""),
        "{answer}"
    );
    assert!(!answer.contains("upstream_name"), "{answer}");
}

/// The status of a raw answer and the refusal code its body carries, if any.
fn status_and_code(answer: &str) -> (String, Option<String>) {
    let status = answer.split(' ').nth(1).unwrap_or_default().to_owned();
    let code = answer
        .split_once("\"code\":\"")
        .and_then(|(_, rest)| rest.split_once('"').map(|(code, _)| code.to_owned()));
    (status, code)
}

/// Security review, wave 2026-10-30-w01 (story:probes-before-head-checks): only the literal
/// probe and public paths, read with `GET` or `HEAD`, skip the repeated-header check. A target
/// that merely resembles one (a trailing slash, a doubled slash, another case, a percent
/// escape, a parameter or fragment, another method), and every relayed wire path, reaches an
/// authenticated route, where a repeated `authorization` is still `credential-malformed`; an
/// absolute-form target is `request-malformed`. A query string is the one variation that is
/// still the probe or the public route.
#[test]
fn sec_w01_a_target_resembling_a_probe_or_public_route_still_refuses_a_repeated_credential() {
    let handle = gateway();
    let twice =
        format!("authorization: Bearer {OWNER_SECRET}\r\nauthorization: Bearer {OWNER_SECRET}\r\n");
    let send = |method: &str, target: &str| {
        status_and_code(&exchange(
            &handle,
            format!("{method} {target} HTTP/1.1\r\nhost: g\r\n{twice}\r\n").as_bytes(),
        ))
    };
    let mut wrong = Vec::new();
    for target in [
        "/health/",
        "//health",
        "/Health",
        "/HEALTH",
        "/health%2F",
        "/%68ealth",
        "/health;x",
        "/health#x",
        "/health/.",
        "/./health",
        "/ready/",
        "/READY",
        "/v1/models/",
        "//v1/models",
        "/v1//models",
        "/V1/models",
        "/v1%2Fmodels",
        "/v1/models#x",
        "//",
        "/%2F",
        "/v1/routes",
        "/v1/routes?x",
        "/metrics",
        "/v1/messages",
        "/v1/messages?x",
        "/v1/chat/completions",
        "/v1/responses",
        "/v1/models/code/status",
    ] {
        for method in ["GET", "HEAD"] {
            let (status, code) = send(method, target);
            if status != "401"
                || (method == "GET" && code.as_deref() != Some("credential-malformed"))
            {
                wrong.push(format!("{method} {target}: {status} {code:?}"));
            }
        }
    }
    for (method, target) in [
        ("POST", "/health"),
        ("PUT", "/v1/models"),
        ("OPTIONS", "/"),
        ("get", "/health"),
        ("POST", "/v1/messages"),
    ] {
        let (status, code) = send(method, target);
        if status != "401" || code.as_deref() != Some("credential-malformed") {
            wrong.push(format!("{method} {target}: {status} {code:?}"));
        }
    }
    for target in ["http://g/health", "http://g/v1/models", "*"] {
        let (status, code) = send("GET", target);
        if code.as_deref() != Some("request-malformed") {
            wrong.push(format!("GET {target}: {status} {code:?}"));
        }
    }
    let probes = [
        send("GET", "/health?x").0,
        send("HEAD", "/").0,
        send("GET", "/?x").0,
    ];
    handle.shutdown();
    assert!(
        wrong.is_empty(),
        "skipped the repeated-credential check:\n{}",
        wrong.join("\n")
    );
    assert_eq!(probes, ["200", "200", "200"]);
}

/// Security review, wave 2026-10-30-w01: a head another reader could split differently is
/// refused, and a probe that skips its header checks still answers one request per connection,
/// so a body it never reads, however it is framed, is never taken for a second request.
#[test]
fn sec_w01_no_head_shape_lets_a_second_request_ride_along() {
    let handle = gateway();
    let inner = format!("GET /v1/routes HTTP/1.1\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n");
    let length = inner.len();
    let mut wrong = Vec::new();
    for raw in [
        format!("GET /health HTTP/1.1\r\ncontent-length: {length}\r\n\r\n{inner}"),
        format!(
            "GET /health HTTP/1.1\r\ntransfer-encoding: chunked\r\ncontent-length: 4\r\n\r\n0\r\n\r\n{inner}"
        ),
        format!(
            "GET /health HTTP/1.1\r\ncontent-length: 0\r\ncontent-length: {length}\r\n\r\n{inner}"
        ),
        format!(
            "GET /ready HTTP/1.1\r\ntransfer-encoding: chunked\r\ntransfer-encoding: identity\r\n\r\n{inner}"
        ),
        format!("GET /health HTTP/1.1\r\n\r\n{inner}"),
    ] {
        let answer = exchange(&handle, raw.as_bytes());
        if !answer.starts_with("HTTP/1.1 200 ")
            || answer.matches("HTTP/1.1 ").count() != 1
            || answer.contains("\"routes\"")
            || !answer.contains("connection: close\r\n")
        {
            wrong.push(format!("{raw:?}: {answer}"));
        }
    }
    for raw in [
        "GET /v1/models HTTP/1.1\r\ncontent-length: 0\r\ncontent-length: 0\r\n\r\n".to_owned(),
        "GET /v1/models HTTP/1.1\r\ntransfer-encoding: chunked\r\ntransfer-encoding: chunked\r\n\r\n".to_owned(),
        format!("GET /v1/routes HTTP/1.1\r\nx-a: b\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"),
        format!("GET /v1/routes HTTP/1.1\r\nx-a: b\rauthorization: Bearer {OWNER_SECRET}\r\n\r\n"),
        format!("GET /v1/routes HTTP/1.1\r\nx-a: b\0\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"),
        format!("GET /v1/routes HTTP/1.1\r\nx-a: b\r\n authorization: Bearer {OWNER_SECRET}\r\n\r\n"),
        format!("GET /v1/routes HTTP/1.1\r\nx-a: b\r\n\tauthorization: Bearer {OWNER_SECRET}\r\n\r\n"),
        format!("GET /v1/routes HTTP/1.1\r\nauthorization : Bearer {OWNER_SECRET}\r\n\r\n"),
        format!("GET /v1/routes HTTP/1.1\r\nauthorization\t: Bearer {OWNER_SECRET}\r\n\r\n"),
        format!("GET /v1/routes HTTP/1.1\nauthorization: Bearer {OWNER_SECRET}\n\n"),
        format!("GET /v1/routes HTTP/1.1\r\nauthorization: Bearer {OWNER_SECRET}\n\r\n"),
        format!("POST /v1/messages HTTP/1.1\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-length: 2\r\ntransfer-encoding: chunked\nx: y\r\n\r\n{{}}"),
        format!("GET /health HTTP/1.1\r\nx: a\n\r\n{inner}"),
        format!("GET /health HTTP/1.1\r\n\0\r\n\r\n{inner}"),
        "GET /health\0 HTTP/1.1\r\n\r\n".to_owned(),
        "GET /health HTTP/1.1\r\r\n\r\n".to_owned(),
    ] {
        let (status, code) = status_and_code(&exchange(&handle, raw.as_bytes()));
        if status != "400" || code.as_deref() != Some("request-malformed") {
            wrong.push(format!("{raw:?}: {status} {code:?}"));
        }
    }
    handle.shutdown();
    assert!(
        wrong.is_empty(),
        "admitted a head another reader could split differently:\n{}",
        wrong.join("\n")
    );
}
