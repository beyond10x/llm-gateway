//! story:hosted-status-probe: a hosted model names its status probe (`[hosted.<alias>]
//! status_probe`, `spec/domains/deployment.yaml` `HostedDeclaration`): `models`, the default and
//! the probe a hosted model always had, or `count-tokens`, one `POST <base path>/messages/count_tokens`
//! of a one-word message, for an endpoint that answers model calls and no model list.
//!
//! Every endpoint is a loopback fake that records each request whole and answers it one fixed
//! way. A test fails on any request to it other than the declared probe: the probe never asks
//! for a generation. Nothing leaves the loopback interface and no credential is real.

use crate::support;

use llm_gateway_cli::{Deployment, PodConnector, load, parse, start_relaying};
use llm_runpod::{EmulatedRunpod, ManualClock};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use support::{
    ENDPOINT_CREDENTIAL, Fixture, OWNER_SECRET, bearer_keys, document, hosted_table, with_vllm_key,
};

const WAIT: Duration = Duration::from_secs(10);
const ALIAS: &str = "proxied";
const UPSTREAM: &str = "vendor-proxied";

/// A loopback endpoint answering every connection with `answer` (or closing without one) after
/// `delay`, and keeping every request it was sent, head and body, in arrival order.
struct Endpoint {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Endpoint {
    fn start(answer: Option<&'static str>, delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&requests);
        thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut connection) = connection else {
                    return;
                };
                let kept = Arc::clone(&kept);
                thread::spawn(move || {
                    connection.set_read_timeout(Some(WAIT)).unwrap();
                    let request = read_request(&mut connection);
                    kept.lock().unwrap().push(request);
                    thread::sleep(delay);
                    if let Some(answer) = answer {
                        drop(connection.write_all(answer.as_bytes()));
                    }
                });
            }
        });
        Self { address, requests }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

/// One request: its head to the blank line, then as many body bytes as its `content-length`.
fn read_request(connection: &mut TcpStream) -> String {
    let mut seen = Vec::new();
    let mut byte = [0_u8; 1];
    while !seen.ends_with(b"\r\n\r\n") {
        match connection.read(&mut byte) {
            Ok(1) => seen.push(byte[0]),
            _ => return String::from_utf8_lossy(&seen).into_owned(),
        }
    }
    let head = String::from_utf8_lossy(&seen).to_ascii_lowercase();
    let length = head
        .split("\r\n")
        .find_map(|line| line.strip_prefix("content-length: "))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0_u8; length];
    if connection.read_exact(&mut body).is_ok() {
        seen.extend_from_slice(&body);
    }
    String::from_utf8_lossy(&seen).into_owned()
}

/// Reaches no pod: the document's pod model is never called here.
struct NoPod;

impl PodConnector for NoPod {
    fn connect(&self, _authority: &str) -> io::Result<Box<dyn llm_gateway::RelayStream>> {
        Err(io::Error::other("no pod in this test"))
    }
}

/// The support document with [`ALIAS`] at `base_url`, a `bearer` endpoint with the static header
/// `x-team: blue`, and `lines`.
fn deployment(fixture: &Fixture, base_url: &str, lines: &str) -> Deployment {
    let secret = fixture.owner_secret();
    let key = fixture.vllm_key();
    let credential = fixture.endpoint_credential();
    let text = with_vllm_key(&document("127.0.0.1:0", &secret), &key);
    let table = hosted_table(
        ALIAS,
        base_url,
        &format!(
            "upstream_model = \"{UPSTREAM}\"\n{}{lines}\n[hosted.{ALIAS}.headers]\nx-team = \"blue\"\n",
            bearer_keys(&credential)
        ),
    );
    load(&fixture.config(&format!("{text}\n{table}"))).unwrap()
}

/// `GET /v1/status` with the owner credential, `reads` times one after the other; the body of
/// the last.
fn status_lists(deployment: &Deployment, reads: usize) -> Vec<String> {
    let runpod = EmulatedRunpod::new();
    let running = start_relaying(
        deployment,
        runpod.clone(),
        Arc::new(NoPod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let address = running.local_addr();
    let lists = (0..reads).map(|_| status_list(address)).collect();
    running.shutdown();
    assert_eq!(runpod.create_calls(), 0);
    lists
}

fn status_list(address: SocketAddr) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    write!(
        stream,
        "GET /v1/status HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
    )
    .unwrap();
    let mut answer = String::new();
    drop(stream.read_to_string(&mut answer));
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    answer
        .split_once("\r\n\r\n")
        .map_or("", |(_, body)| body)
        .to_owned()
}

/// The status list's entry of [`ALIAS`], the hosted model listed first.
fn entry(list: &str) -> &str {
    let start = format!("{{\"model\":\"{ALIAS}\",");
    let from = list
        .find(&start)
        .unwrap_or_else(|| panic!("no entry for {ALIAS}: {list}"));
    let rest = &list[from..];
    &rest[..=rest.find('}').unwrap()]
}

/// The hosted model's entry with `status` after its wires.
fn listed(status: &str) -> String {
    format!(
        "{{\"model\":\"{ALIAS}\",\"kind\":\"hosted\",\"wires\":[\"chat\",\"messages\"],{status}}}"
    )
}

/// The one-word message the count-tokens probe sends for [`UPSTREAM`].
const PROBE_BODY: &str =
    r#"{"model":"vendor-proxied","messages":[{"role":"user","content":"ping"}]}"#;

/// The count-tokens probe, exactly, for an endpoint at `address` under `path`.
fn count_tokens_probe(address: SocketAddr, path: &str) -> String {
    format!(
        "POST {path} HTTP/1.1\r\nhost: {address}\r\nx-team: blue\r\nauthorization: Bearer {ENDPOINT_CREDENTIAL}\r\nanthropic-version: 2023-06-01\r\naccept: application/json\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{PROBE_BODY}",
        PROBE_BODY.len()
    )
}

/// The models probe, exactly, as a hosted model has always been probed.
fn models_probe(address: SocketAddr) -> String {
    format!(
        "GET /v1/models HTTP/1.1\r\nhost: {address}\r\nx-team: blue\r\nauthorization: Bearer {ENDPOINT_CREDENTIAL}\r\naccept: application/json\r\nconnection: close\r\n\r\n"
    )
}

/// The status of a `count-tokens` model whose endpoint answers `answer`, and that the endpoint
/// was sent the probe once and nothing else.
fn count_tokens_status(name: &str, answer: Option<&'static str>) -> String {
    let fixture = Fixture::new(name);
    let endpoint = Endpoint::start(answer, Duration::ZERO);
    let deployment = deployment(
        &fixture,
        &format!("http://{}/v1", endpoint.address),
        "status_probe = \"count-tokens\"\n",
    );
    let list = status_lists(&deployment, 1).remove(0);
    assert_eq!(
        endpoint.requests(),
        [count_tokens_probe(
            endpoint.address,
            "/v1/messages/count_tokens"
        )],
        "the endpoint was sent something other than the declared probe"
    );
    entry(&list).to_owned()
}

/// Acceptance 1: the probe is one `POST <base path>/messages/count_tokens` of a one-word message
/// for the model's `upstream_model`, with the model's static headers and credential, and asks
/// for no generation; a base path replaces the leading `/v1` as for a relayed request.
#[test]
fn status_probe_count_tokens_is_one_post_of_a_one_word_message_and_nothing_else() {
    for (base, path) in [
        ("/v1", "/v1/messages/count_tokens"),
        ("/anthropic", "/anthropic/messages/count_tokens"),
        ("", "/v1/messages/count_tokens"),
    ] {
        let fixture = Fixture::new("status-probe-request");
        let endpoint = Endpoint::start(
            Some(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 18\r\nconnection: close\r\n\r\n{\"input_tokens\":8}",
            ),
            Duration::ZERO,
        );
        let deployment = deployment(
            &fixture,
            &format!("http://{}{base}", endpoint.address),
            "status_probe = \"count-tokens\"\n",
        );
        let list = status_lists(&deployment, 1).remove(0);
        assert_eq!(
            entry(&list),
            listed("\"phase\":\"ready\",\"callable\":true"),
            "{base:?}"
        );
        assert_eq!(
            endpoint.requests(),
            [count_tokens_probe(endpoint.address, path)],
            "{base:?}"
        );
        let request = &endpoint.requests()[0];
        assert!(
            !request.contains("max_tokens") && !request.contains("/v1/messages HTTP"),
            "the probe asked for a generation: {request}"
        );
    }
}

/// Acceptance 2 and 3, one answer each: a 2xx answer is `ready`.
#[test]
fn status_probe_count_tokens_answered_200_is_ready() {
    assert_eq!(
        count_tokens_status(
            "status-probe-200",
            Some("HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}")
        ),
        listed("\"phase\":\"ready\",\"callable\":true")
    );
}

#[test]
fn status_probe_count_tokens_answered_401_is_credential_refused() {
    assert_eq!(
        count_tokens_status(
            "status-probe-401",
            Some("HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
        ),
        listed("\"phase\":\"unreachable\",\"sub_phase\":\"credential-refused\",\"callable\":false")
    );
}

#[test]
fn status_probe_count_tokens_answered_404_is_status_404() {
    assert_eq!(
        count_tokens_status(
            "status-probe-404",
            Some("HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
        ),
        listed("\"phase\":\"unreachable\",\"sub_phase\":\"status-404\",\"callable\":false")
    );
}

#[test]
fn status_probe_count_tokens_answered_429_is_status_429() {
    assert_eq!(
        count_tokens_status(
            "status-probe-429",
            Some(
                "HTTP/1.1 429 Too Many Requests\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            )
        ),
        listed("\"phase\":\"unreachable\",\"sub_phase\":\"status-429\",\"callable\":false")
    );
}

#[test]
fn status_probe_count_tokens_answered_503_is_status_503() {
    assert_eq!(
        count_tokens_status(
            "status-probe-503",
            Some("HTTP/1.1 503 Busy\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
        ),
        listed("\"phase\":\"unreachable\",\"sub_phase\":\"status-503\",\"callable\":false")
    );
}

#[test]
fn status_probe_count_tokens_answered_without_a_status_line_is_no_answer() {
    assert_eq!(
        count_tokens_status("status-probe-garbage", Some("not http at all\r\n\r\n")),
        listed("\"phase\":\"unreachable\",\"sub_phase\":\"no-answer\",\"callable\":false")
    );
}

#[test]
fn status_probe_count_tokens_closed_without_an_answer_is_no_answer() {
    assert_eq!(
        count_tokens_status("status-probe-closed", None),
        listed("\"phase\":\"unreachable\",\"sub_phase\":\"no-answer\",\"callable\":false")
    );
}

/// Acceptance 3: the default is unchanged. A model that names no probe, and one that names
/// `models`, is probed `GET <base path>/models` exactly as before, and a 404 to it is still
/// `status-404`.
#[test]
fn status_probe_the_default_is_the_models_probe_unchanged() {
    for lines in ["", "status_probe = \"models\"\n"] {
        for (answer, status) in [
            (
                "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                "\"phase\":\"ready\",\"callable\":true",
            ),
            (
                "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                "\"phase\":\"unreachable\",\"sub_phase\":\"status-404\",\"callable\":false",
            ),
        ] {
            let fixture = Fixture::new("status-probe-default");
            let endpoint = Endpoint::start(Some(answer), Duration::ZERO);
            let deployment =
                deployment(&fixture, &format!("http://{}/v1", endpoint.address), lines);
            let list = status_lists(&deployment, 1).remove(0);
            assert_eq!(entry(&list), listed(status), "{lines:?}");
            assert_eq!(
                endpoint.requests(),
                [models_probe(endpoint.address)],
                "{lines:?}"
            );
        }
    }
}

/// Acceptance 2: the `count-tokens` probe keeps the probe's bounds. Two reads inside the reuse
/// time send one probe, and two reads at once, while the endpoint takes 300 ms to answer, send one
/// probe and both read its result. (The 5 s deadline: `route_status_a_count_tokens_probe_not_answered_inside_its_deadline_is_no_answer_at_the_deadline`
/// in `crates/llm-gateway-cli/src/hosted.rs`.)
#[test]
fn status_probe_count_tokens_is_reused_and_sent_once_at_a_time() {
    let ok = "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}";
    let fixture = Fixture::new("status-probe-bounds");
    let endpoint = Endpoint::start(Some(ok), Duration::from_millis(300));
    let deployment = deployment(
        &fixture,
        &format!("http://{}/v1", endpoint.address),
        "status_probe = \"count-tokens\"\n",
    );
    let runpod = EmulatedRunpod::new();
    let running = start_relaying(
        &deployment,
        runpod,
        Arc::new(NoPod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let address = running.local_addr();
    let readers: Vec<_> = (0..2)
        .map(|_| thread::spawn(move || status_list(address)))
        .collect();
    let lists: Vec<String> = readers
        .into_iter()
        .map(|reader| reader.join().unwrap())
        .collect();
    let again = status_list(address);
    running.shutdown();
    let ready = listed("\"phase\":\"ready\",\"callable\":true");
    for list in lists.iter().chain([&again]) {
        assert_eq!(entry(list), ready);
    }
    assert_eq!(
        endpoint.requests(),
        [count_tokens_probe(
            endpoint.address,
            "/v1/messages/count_tokens"
        )],
        "more than one probe for three reads inside the reuse time"
    );
}

/// Acceptance 1: `status_probe` is `models` or `count-tokens`; `count-tokens` is a Messages-wire
/// model's alone. Every refusal is `config:value` naming `hosted.<alias>.status_probe` and quoting
/// no value; an Ollama or Bedrock model, whose status is not a probe of its endpoint, takes no
/// `status_probe` at all.
#[test]
fn status_probe_is_models_or_count_tokens_and_count_tokens_needs_the_messages_wire() {
    let fixture = Fixture::new("status-probe-config");
    let secret = fixture.owner_secret();
    let credential = fixture.endpoint_credential();
    let base = format!(
        "listen = \"127.0.0.1:0\"\nowner_secret_file = \"{}\"\n\n",
        secret.display()
    );
    let endpoint = |wires: &str, probe: &str| {
        format!(
            "{base}[hosted.{ALIAS}]\nwires = {wires}\nbase_url = \"http://127.0.0.1:7000/v1\"\n{}status_probe = \"{probe}\"\n",
            bearer_keys(&credential)
        )
    };
    for (wires, probe) in [
        ("[\"messages\"]", "count-tokens"),
        ("[\"chat\", \"messages\"]", "count-tokens"),
        ("[\"chat\"]", "models"),
        ("[\"messages\"]", "models"),
    ] {
        let parsed = parse(&endpoint(wires, probe));
        assert!(parsed.is_ok(), "{wires} {probe}: {:?}", parsed.err());
    }
    let ollama =
        format!("{base}[hosted.{ALIAS}]\nkind = \"ollama\"\nstatus_probe = \"count-tokens\"\n");
    let bedrock = format!(
        "{base}[hosted.{ALIAS}]\nkind = \"bedrock\"\nregion = \"eu-central-1\"\nmodel_id = \"anthropic.claude-sonnet-4-5\"\ncredential_source = \"container\"\nstatus_probe = \"models\"\n"
    );
    for (case, text) in [
        (
            "count-tokens on a chat model",
            endpoint("[\"chat\"]", "count-tokens"),
        ),
        (
            "count-tokens on a responses and chat model",
            endpoint("[\"chat\", \"responses\"]", "count-tokens"),
        ),
        ("an unknown probe", endpoint("[\"messages\"]", "health-x7")),
        ("the empty probe", endpoint("[\"messages\"]", "")),
        ("an Ollama model", ollama),
        ("a Bedrock model", bedrock),
    ] {
        let refusal = parse(&text).expect_err(case);
        assert_eq!(refusal.code(), "config:value", "{case}: {refusal}");
        assert!(
            refusal
                .message()
                .starts_with(&format!("hosted.{ALIAS}.status_probe")),
            "{case}: {refusal}"
        );
        assert!(
            !refusal.message().contains("health-x7"),
            "{case}: {refusal}"
        );
    }
}
