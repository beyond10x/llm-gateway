//! `RestRunpod` against a loopback HTTP fixture: a `std` listener on `127.0.0.1` that records
//! each request and plays a scripted answer. No call leaves the machine, no pod is created and
//! no keychain is read; the API key is a test literal.

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use llm_runpod::{
    AccountSpend, CloudType, CreateAnswer, PodPlacement, PodRequest, PodStatus, PodStream, PodTls,
    RestBinding, RestRunpod, RunpodAccount, RunpodTransport, TerminateAnswer,
};
use serde_json::{Value, json};

const KEY: &str = "rpa_TESTKEY0123456789abcdef";

/// One scripted answer.
enum Play {
    /// These bytes, then the connection closes.
    Bytes(Vec<u8>),
    /// The connection closes with no answer.
    Close,
    /// Nothing is answered for this long, then the connection closes.
    Stall(Duration),
}

/// One recorded request.
#[derive(Debug, Clone)]
struct Seen {
    line: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl Seen {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("a JSON body")
    }
}

struct Server {
    address: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Server {
    /// Serves `script` in order, one connection per answer.
    fn start(script: Vec<Play>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address").to_string();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        thread::spawn(move || {
            for play in script {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                if let Some(request) = read_request(&mut stream) {
                    record.lock().expect("record").push(request);
                }
                match play {
                    Play::Bytes(bytes) => {
                        let _ = stream.write_all(&bytes);
                    }
                    Play::Close => {}
                    Play::Stall(duration) => thread::sleep(duration),
                }
            }
        });
        Self { address, seen }
    }

    fn base(&self) -> String {
        format!("http://{}/v1", self.address)
    }

    fn transport(&self) -> RestRunpod {
        RestRunpod::new(
            RestBinding {
                base_url: self.base(),
                graphql_url: format!("http://{}/graphql", self.address),
                timeout: Duration::from_millis(1_500),
            },
            KEY.as_bytes().to_vec(),
        )
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().expect("seen").clone()
    }

    fn lines(&self) -> Vec<String> {
        self.seen().into_iter().map(|seen| seen.line).collect()
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Seen> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let end = loop {
        if let Some(at) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break at + 4;
        }
        let read = stream.read(&mut buffer).ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..read]);
    };
    let head = String::from_utf8(bytes[..end].to_vec()).ok()?;
    let mut lines = head.split("\r\n");
    let line = lines.next()?.to_owned();
    let headers: BTreeMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length: usize = headers
        .get("content-length")
        .and_then(|length| length.parse().ok())
        .unwrap_or(0);
    let mut body = bytes[end..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    Some(Seen {
        line,
        headers,
        body,
    })
}

fn answer(status: u16, body: &str) -> Play {
    Play::Bytes(
        format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes(),
    )
}

fn pod(id: &str, request: &str) -> Value {
    json!({
        "id": id,
        "name": "b10x-llm-qwen",
        "desiredStatus": "RUNNING",
        "costPerHr": 0.99,
        "gpu": {"id": "NVIDIA L40S", "count": 1},
        "env": {"B10X_LLM_OWNER": "owner-a", "B10X_LLM_EPOCH": "1", "B10X_LLM_REQUEST": request},
        "lastStartedAt": "2026-11-04T10:00:00.000Z",
    })
}

fn request() -> PodRequest {
    PodRequest {
        name: "b10x-llm-qwen".to_owned(),
        image: "vllm/vllm-openai:v0.27.1".to_owned(),
        gpu_type: "NVIDIA L40S".to_owned(),
        gpu_count: 1,
        cloud_type: CloudType::Secure,
        container_disk_gb: 80,
        ports: vec!["8000/http".to_owned()],
        interruptible: false,
        env: BTreeMap::from([
            ("B10X_LLM_OWNER".to_owned(), "owner-a".to_owned()),
            ("B10X_LLM_EPOCH".to_owned(), "1".to_owned()),
            ("B10X_LLM_REQUEST".to_owned(), "req-1".to_owned()),
            (
                "VLLM_API_KEY".to_owned(),
                "{{ RUNPOD_SECRET_qwen_key }}".to_owned(),
            ),
        ]),
        docker_entrypoint: vec!["vllm".to_owned(), "serve".to_owned()],
        network_volume_id: None,
        volume_mount_path: None,
        data_center_ids: Vec::new(),
    }
}

/// Every request carries the bearer, the user agent and `Connection: close`.
fn assert_headers(seen: &Seen) {
    assert_eq!(
        seen.headers.get("authorization").map(String::as_str),
        Some(format!("Bearer {KEY}").as_str()),
        "{}",
        seen.line
    );
    assert_eq!(
        seen.headers.get("user-agent").map(String::as_str),
        Some("b10x-llm-gateway")
    );
    assert_eq!(
        seen.headers.get("connection").map(String::as_str),
        Some("close")
    );
    assert!(seen.headers.contains_key("host"));
}

#[test]
fn the_binding_names_runpods_endpoints() {
    let binding = RestBinding::runpod();
    assert_eq!(binding.base_url, "https://rest.runpod.io/v1");
    assert_eq!(binding.graphql_url, "https://api.runpod.io/graphql");
    assert_eq!(binding.timeout, Duration::from_secs(30));
}

#[test]
fn list_pods_is_get_pods_with_the_bearer() {
    let good = json!([pod("abc123", "req-1"), pod("def456", "req-2")]).to_string();
    let partly =
        json!([pod("abc123", "req-1"), {"id": "x", "desiredStatus": "WHATEVER"}]).to_string();
    let server = Server::start(vec![
        answer(200, &good),
        answer(200, &partly),
        answer(500, "{}"),
    ]);
    let mut transport = server.transport();
    let listing = transport.list_pods().expect("listing");
    assert!(listing.complete);
    assert_eq!(listing.pods.len(), 2);
    assert_eq!(listing.pods[0].id, "abc123");
    assert_eq!(listing.pods[0].status, PodStatus::Running);
    assert_eq!(listing.pods[0].gpu_type.as_deref(), Some("NVIDIA L40S"));
    assert_eq!(listing.pods[0].env["B10X_LLM_REQUEST"], "req-1");
    let partial = transport.list_pods().expect("listing");
    assert!(!partial.complete, "an unreadable entry makes it partial");
    assert_eq!(partial.pods.len(), 1);
    let failed = transport.list_pods().expect("listing");
    assert!(!failed.complete && failed.pods.is_empty());
    let seen = server.seen();
    assert_eq!(seen.len(), 3);
    for request in &seen {
        assert_eq!(request.line, "GET /v1/pods HTTP/1.1");
        assert_headers(request);
        assert!(request.body.is_empty());
    }
}

#[test]
fn create_pod_posts_the_unwrapped_body_and_maps_a_2xx_to_created() {
    let server = Server::start(vec![answer(201, &pod("abc123", "req-1").to_string())]);
    let answer = server.transport().create_pod(&request());
    let CreateAnswer::Created(created) = answer else {
        panic!("{answer:?}");
    };
    assert_eq!(created.id, "abc123");
    let seen = server.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].line, "POST /v1/pods HTTP/1.1");
    assert_headers(&seen[0]);
    assert_eq!(
        seen[0].headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    let body = seen[0].json();
    assert!(body.get("body").is_none(), "not wrapped: {body}");
    assert_eq!(body["cloudType"], "SECURE");
    assert_eq!(body["interruptible"], false);
    assert_eq!(body["imageName"], "vllm/vllm-openai:v0.27.1");
    assert_eq!(body["gpuTypeIds"], json!(["NVIDIA L40S"]));
    assert_eq!(body["name"], "b10x-llm-qwen");
    assert_eq!(body["env"]["VLLM_API_KEY"], "{{ RUNPOD_SECRET_qwen_key }}");
    for key in body.as_object().expect("object").keys() {
        assert!(
            llm_runpod::POD_CREATE_BODY_KEYS.contains(&key.as_str()),
            "{key}"
        );
    }
}

#[test]
fn a_refused_create_is_no_capacity_only_for_runpods_capacity_answer() {
    for (status, body, expected) in [
        (
            400,
            r#"{"error":"create pod: There are no instances currently available"}"#,
            CreateAnswer::NoCapacity,
        ),
        (
            400,
            r#"{"error":"invalid gpuTypeIds"}"#,
            CreateAnswer::Refused,
        ),
        (401, r#"{"error":"unauthorized"}"#, CreateAnswer::Refused),
        (403, "", CreateAnswer::Refused),
        (429, "", CreateAnswer::Refused),
    ] {
        let server = Server::start(vec![answer(status, body)]);
        assert_eq!(
            server.transport().create_pod(&request()),
            expected,
            "{status} {body}"
        );
        assert_eq!(server.seen().len(), 1, "{status}: no second call");
    }
}

/// No answer, a 5xx or a timeout: one `GET /pods?name=…` resolves the create by its request
/// tag; nothing is ever sent twice.
#[test]
fn a_lost_create_is_resolved_once_by_name_and_request_tag() {
    let ours = json!([pod("abc123", "req-1"), pod("zzz999", "req-other")]).to_string();
    let none = json!([pod("zzz999", "req-other")]).to_string();
    let two = json!([pod("abc123", "req-1"), pod("abd124", "req-1")]).to_string();
    let cases: Vec<(&str, Play, Play, CreateAnswer)> = vec![
        (
            "5xx, found",
            answer(502, ""),
            answer(200, &ours),
            CreateAnswer::Lost,
        ),
        (
            "closed, absent",
            Play::Close,
            answer(200, &none),
            CreateAnswer::Lost,
        ),
        (
            "5xx, two",
            answer(500, ""),
            answer(200, &two),
            CreateAnswer::Lost,
        ),
        (
            "5xx, unreadable listing",
            answer(503, ""),
            answer(500, ""),
            CreateAnswer::Lost,
        ),
        (
            "timeout, found",
            Play::Stall(Duration::from_millis(2_500)),
            answer(200, &ours),
            CreateAnswer::Lost,
        ),
    ];
    for (case, first, second, mut expected) in cases {
        if case.ends_with("found") {
            let found = serde_json::from_str::<Value>(&ours).expect("json")[0].clone();
            expected = CreateAnswer::Created(llm_runpod::Pod {
                id: found["id"].as_str().expect("id").to_owned(),
                name: "b10x-llm-qwen".to_owned(),
                status: PodStatus::Running,
                env: BTreeMap::from([
                    ("B10X_LLM_OWNER".to_owned(), "owner-a".to_owned()),
                    ("B10X_LLM_EPOCH".to_owned(), "1".to_owned()),
                    ("B10X_LLM_REQUEST".to_owned(), "req-1".to_owned()),
                ]),
                gpu_type: Some("NVIDIA L40S".to_owned()),
            });
        }
        let server = Server::start(vec![first, second]);
        assert_eq!(
            server.transport().create_pod(&request()),
            expected,
            "{case}"
        );
        let lines = server.lines();
        assert_eq!(
            lines,
            [
                "POST /v1/pods HTTP/1.1",
                "GET /v1/pods?name=b10x-llm-qwen HTTP/1.1"
            ],
            "{case}"
        );
    }
}

#[test]
fn terminate_is_delete_by_id() {
    for (play, expected) in [
        (answer(200, "{}"), TerminateAnswer::Terminated),
        (
            Play::Bytes(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_vec()),
            TerminateAnswer::Terminated,
        ),
        (answer(404, "{}"), TerminateAnswer::Refused),
        (answer(400, "{}"), TerminateAnswer::Refused),
        (answer(500, "{}"), TerminateAnswer::Lost),
        (Play::Close, TerminateAnswer::Lost),
    ] {
        let server = Server::start(vec![play]);
        assert_eq!(server.transport().terminate_pod("abc123"), expected);
        let seen = server.seen();
        assert_eq!(seen[0].line, "DELETE /v1/pods/abc123 HTTP/1.1");
        assert_headers(&seen[0]);
    }
    // A pod id is one path segment, whatever it holds.
    let server = Server::start(vec![answer(404, "{}")]);
    let _ = server.transport().terminate_pod("a/../b?c");
    assert_eq!(server.lines(), ["DELETE /v1/pods/a%2F..%2Fb%3Fc HTTP/1.1"]);
}

#[test]
fn the_start_time_is_last_started_at_of_get_pod() {
    let server = Server::start(vec![
        answer(200, &pod("abc123", "req-1").to_string()),
        answer(200, &pod("other", "req-1").to_string()),
    ]);
    let mut transport = server.transport();
    assert_eq!(
        transport.container_started_at("abc123"),
        llm_runpod::started_at_ms("2026-11-04T10:00:00.000Z")
    );
    assert_eq!(
        transport.container_started_at("abc123"),
        None,
        "an answer about another pod"
    );
    assert_eq!(server.lines()[0], "GET /v1/pods/abc123 HTTP/1.1");
}

#[test]
fn the_placement_is_read_with_the_machine() {
    let with = |machine: Value, interruptible: Option<bool>| {
        let mut pod = pod("abc123", "req-1");
        pod["machine"] = machine;
        if let Some(interruptible) = interruptible {
            pod["interruptible"] = json!(interruptible);
        }
        answer(200, &pod.to_string())
    };
    let cases = [
        (
            with(
                json!({"secureCloud": true, "dataCenterId": "EU-RO-1"}),
                None,
            ),
            Some(PodPlacement {
                secure_cloud: Some(true),
                interruptible: None,
            }),
        ),
        (
            with(json!({"secureCloud": false}), Some(false)),
            Some(PodPlacement {
                secure_cloud: Some(false),
                interruptible: Some(false),
            }),
        ),
        (
            with(json!({"secureCloud": true}), Some(true)),
            Some(PodPlacement {
                secure_cloud: Some(true),
                interruptible: Some(true),
            }),
        ),
        (
            answer(200, &pod("abc123", "req-1").to_string()),
            Some(PodPlacement {
                secure_cloud: None,
                interruptible: None,
            }),
        ),
        (answer(404, "{}"), None),
        (Play::Close, None),
    ];
    for (at, (play, expected)) in cases.into_iter().enumerate() {
        let server = Server::start(vec![play]);
        let placement = server.transport().pod_placement("abc123");
        assert_eq!(placement, expected, "case {at}");
        assert_eq!(
            server.lines(),
            ["GET /v1/pods/abc123?includeMachine=true HTTP/1.1"],
            "case {at}"
        );
    }
    assert!(
        PodPlacement {
            secure_cloud: Some(true),
            interruptible: None
        }
        .is_secure_on_demand()
    );
}

#[test]
fn spend_is_read_from_billing_pods_by_day() {
    let billed = json!([
        {"amount": 1.5, "time": "2026-11-03T00:00:00.000Z"},
        {"amount": 0.25, "time": "2026-11-04T00:00:00.000Z"},
    ])
    .to_string();
    let day_start = llm_runpod::started_at_ms("2026-11-04T00:00:00.000Z").expect("day");
    let server = Server::start(vec![
        answer(200, &billed),
        answer(
            200,
            r#"[{"amount": -1, "time": "2026-11-04T00:00:00.000Z"}]"#,
        ),
        answer(500, "[]"),
    ]);
    let mut transport = server.transport();
    assert_eq!(
        transport.account_spend(day_start),
        Some(AccountSpend {
            daily_micro_usd: 250_000,
            total_micro_usd: 1_750_000,
        })
    );
    assert_eq!(
        transport.account_spend(day_start),
        None,
        "a negative amount"
    );
    assert_eq!(transport.account_spend(day_start), None, "a 5xx");
    assert_eq!(
        server.lines()[0],
        "GET /v1/billing/pods?bucketSize=day HTTP/1.1"
    );
}

#[test]
fn the_account_is_the_pods_and_the_graphql_balance() {
    let pods = json!([pod("abc123", "req-1"), {"id": "foreign", "name": "someone-else", "desiredStatus": "EXITED", "costPerHr": "0.44", "machine": {"gpuTypeId": "NVIDIA RTX PRO 6000 Blackwell Server Edition"}, "lastStartedAt": "2026-11-04 13:43:01.245 +0000 UTC"}])
        .to_string();
    let balance = r#"{"data":{"myself":{"clientBalance":12.5,"currentSpendPerHr":0.99}}}"#;
    let server = Server::start(vec![answer(200, &pods), answer(200, balance)]);
    let account = server.transport().account().expect("account");
    assert_eq!(account.balance_usd, Some(12.5));
    assert_eq!(account.spend_per_hour_usd, Some(0.99));
    assert_eq!(account.pods.len(), 2);
    let ours = &account.pods[0];
    assert_eq!(ours.id, "abc123");
    assert_eq!(ours.name, "b10x-llm-qwen");
    assert_eq!(ours.desired_status, "RUNNING");
    assert_eq!(ours.cost_per_hour_usd, Some(0.99));
    assert_eq!(ours.gpu.as_deref(), Some("NVIDIA L40S"));
    assert_eq!(ours.env_owner.as_deref(), Some("owner-a"));
    assert_eq!(
        ours.last_started_at_ms,
        llm_runpod::started_at_ms("2026-11-04T10:00:00.000Z")
    );
    let foreign = &account.pods[1];
    assert_eq!(foreign.env_owner, None);
    assert_eq!(
        foreign.gpu.as_deref(),
        Some("NVIDIA RTX PRO 6000 Blackwell Server Edition"),
        "the REST listing names the GPU under machine.gpuTypeId"
    );
    assert_eq!(
        foreign.last_started_at_ms,
        llm_runpod::started_at_ms("2026-11-04T13:43:01.245Z"),
        "the REST listing writes the start as `<date> <time> +0000 UTC`"
    );
    assert_eq!(foreign.cost_per_hour_usd, Some(0.44));
    let seen = server.seen();
    assert_eq!(seen[0].line, "GET /v1/pods?includeMachine=true HTTP/1.1");
    assert_eq!(seen[1].line, "POST /graphql HTTP/1.1");
    assert_headers(&seen[1]);
    let query = seen[1].json()["query"].as_str().expect("query").to_owned();
    assert!(query.contains("myself"), "{query}");
    assert!(query.contains("clientBalance") && query.contains("currentSpendPerHr"));

    // A failed balance read leaves only the balance unreported; a failed listing is no answer.
    let server = Server::start(vec![
        answer(200, "[]"),
        answer(200, r#"{"errors":[{"message":"no"}]}"#),
    ]);
    let account = server.transport().account().expect("account");
    assert_eq!(
        (account.balance_usd, account.spend_per_hour_usd),
        (None, None)
    );
    let server = Server::start(vec![answer(500, "{}")]);
    assert!(server.transport().account().is_none());
}

#[test]
fn a_chunked_answer_is_decoded() {
    let body = json!([pod("abc123", "req-1")]).to_string();
    let (first, second) = body.split_at(10);
    let chunked = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{first}\r\n{:x}\r\n{second}\r\n0\r\n\r\n",
        first.len(),
        second.len()
    );
    let server = Server::start(vec![Play::Bytes(chunked.into_bytes())]);
    let listing = server.transport().list_pods().expect("listing");
    assert!(listing.complete);
    assert_eq!(listing.pods.len(), 1);
}

/// An answer is read up to 8 MiB, head included, and not one byte more.
#[test]
fn an_answer_past_eight_mebibytes_is_unreadable() {
    let small = json!([pod("abc123", "req-1")]).to_string();
    let head = |length: usize| {
        format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n")
    };
    let limit = 8 * 1024 * 1024;
    // The body's length has as many digits as the total's, so the head is as long for both.
    let padded = |total: usize| {
        let body_length = total - head(total).len();
        let mut body = small.clone().into_bytes();
        body.resize(body_length, b' ');
        let mut bytes = head(body_length).into_bytes();
        bytes.extend_from_slice(&body);
        bytes
    };
    let at_limit = padded(limit);
    assert_eq!(at_limit.len(), limit);
    let past = padded(limit + 1);
    let server = Server::start(vec![Play::Bytes(at_limit), Play::Bytes(past)]);
    let mut transport = server.transport();
    let read = transport.list_pods().expect("listing");
    assert!(read.complete, "exactly 8 MiB is read");
    let unread = transport.list_pods().expect("listing");
    assert!(
        !unread.complete && unread.pods.is_empty(),
        "one byte more is not"
    );
}

/// The key reaches only the `Authorization` header: never `Debug`, never an answer.
#[test]
fn the_key_is_never_in_debug_text() {
    let server = Server::start(vec![answer(
        401,
        &format!("{{\"error\":\"bad key {KEY}\"}}"),
    )]);
    let mut transport = server.transport();
    transport.set_vllm_key("qwen", b"vllm-secret-value".to_vec());
    let debug = format!("{transport:?}");
    assert!(!debug.contains(KEY), "{debug}");
    assert!(!debug.contains("vllm-secret-value"), "{debug}");
    assert!(debug.contains("REDACTED"), "{debug}");
    let answer = transport.create_pod(&request());
    assert_eq!(answer, CreateAnswer::Refused);
    assert!(!format!("{answer:?}").contains(KEY));
}

/// Plain HTTP only to a loopback host, and `https://` only over the embedding's TLS: without
/// one, nothing is sent at all.
#[test]
fn no_call_leaves_without_tls_or_to_a_non_loopback_http_host() {
    for base in ["https://rest.runpod.io/v1", "http://192.0.2.1/v1"] {
        let mut transport = RestRunpod::new(
            RestBinding {
                base_url: base.to_owned(),
                graphql_url: "https://api.runpod.io/graphql".to_owned(),
                timeout: Duration::from_millis(200),
            },
            KEY.as_bytes().to_vec(),
        );
        let listing = transport.list_pods().expect("listing");
        assert!(!listing.complete, "{base}");
        assert_eq!(
            transport.create_pod(&request()),
            CreateAnswer::Lost,
            "{base}"
        );
        assert_eq!(transport.terminate_pod("abc123"), TerminateAnswer::Lost);
        assert!(transport.account().is_none());
    }
}

/// A `PodTls` that opens a plain loopback connection in place of TLS, recording the authority
/// it was asked for.
struct LoopbackTls {
    address: String,
    asked: Arc<Mutex<Vec<String>>>,
}

impl PodTls for LoopbackTls {
    fn connect(&self, authority: &str, timeout: Duration) -> std::io::Result<Box<dyn PodStream>> {
        self.asked.lock().expect("asked").push(authority.to_owned());
        let stream = TcpStream::connect(&self.address)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        Ok(Box::new(stream))
    }
}

#[test]
fn an_https_call_goes_over_the_embeddings_tls() {
    let server = Server::start(vec![answer(200, "[]")]);
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut transport = RestRunpod::new(RestBinding::runpod(), KEY.as_bytes().to_vec());
    transport.set_tls(Arc::new(LoopbackTls {
        address: server.address.clone(),
        asked: Arc::clone(&asked),
    }));
    let listing = transport.list_pods().expect("listing");
    assert!(listing.complete);
    assert_eq!(*asked.lock().expect("asked"), ["rest.runpod.io"]);
    let seen = server.seen();
    assert_eq!(seen[0].line, "GET /v1/pods HTTP/1.1");
    assert_eq!(
        seen[0].headers.get("host").map(String::as_str),
        Some("rest.runpod.io")
    );
    assert_headers(&seen[0]);
}
