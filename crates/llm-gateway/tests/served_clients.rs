//! story:hosted-status-probe, acceptance 4: the status reports which clients a model serves, from
//! the gateway's own record of the model's last relayed calls (`docs/gateway.md` "The status
//! list" and "Bounds"). When, among those calls, every one from a client whose `user-agent` names
//! Claude Code was answered 2xx and every one from another client was answered `429` with an
//! error of type `rate_limit_error`, `GET /v1/status` keeps the model `ready` and adds the reason
//! `serves-claude-code-clients-only`, and the dashboard's route row carries it. Every call is
//! still relayed as the client sent it.
//!
//! The target is in-process: each connection records what the gateway writes and answers by the
//! client the request came from, as a local Messages proxy that serves one kind of client does:
//! by the forwarded `user-agent` on a route that forwards it, and from a script in call order on
//! the byte relay, which forwards none. Nothing leaves the process but the client's loopback
//! connection, and no request reaches a real endpoint.

use llm_gateway::{
    Gateway, GatewayConfig, IngressAuth, Label, ModelKind, ModelPhase, ModelStatus,
    NoOwnerVerifier, Relay, RelayModel, RelayStream, RelayTarget, RelayTargets, RouteInventory,
    TargetRefusal, Wire,
};
use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
    time::Duration,
};

/// The reason `GET /v1/status` and the dashboard name.
pub(crate) const REASON: &str = "serves-claude-code-clients-only";
/// What Claude Code sends as its `user-agent`.
const CLAUDE_CODE: &str = "claude-cli/2.1.318 (external, cli)";
/// Another client's `user-agent`.
const OTHER: &str = "codex_cli_rs/0.40.0 (Linux; x86_64)";
const MODEL: &str = "proxied";

/// What the target answers one call with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    /// `200` with a message.
    Ok,
    /// `429` with an error of type `rate_limit_error`.
    RateLimited,
    /// `429` with an error of another type.
    Overloaded,
}

impl Answer {
    fn bytes(self) -> Vec<u8> {
        let (status, body) = match self {
            Self::Ok => (
                "200 OK",
                r#"{"id":"msg_1","type":"message","role":"assistant","content":[],"usage":{"input_tokens":3,"output_tokens":1}}"#,
            ),
            Self::RateLimited => (
                "429 Too Many Requests",
                r#"{"type":"error","error":{"type":"rate_limit_error","message":"This request would exceed your account's rate limit."}}"#,
            ),
            Self::Overloaded => (
                "429 Too Many Requests",
                r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
            ),
        };
        format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\nx-should-retry: false\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }
}

/// One call: who sends it and what the target answers it.
type Call = (Client, Answer);

/// Who sends one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Client {
    ClaudeCode,
    Other,
}

impl Client {
    fn user_agent(self) -> &'static str {
        match self {
            Self::ClaudeCode => CLAUDE_CODE,
            Self::Other => OTHER,
        }
    }
}

/// What the fake decides by.
struct Fake {
    /// The answer to a request from Claude Code and to one from another client, read from the
    /// forwarded `user-agent`.
    by_agent: (Answer, Answer),
    /// The answers, in call order, to requests that carry no `user-agent` (the byte relay).
    script: Mutex<VecDeque<Answer>>,
    /// Every request the target was written, one entry per connection.
    seen: Mutex<Vec<Vec<u8>>>,
}

impl Fake {
    fn answer_for(&self, request: &[u8]) -> Answer {
        let text = String::from_utf8_lossy(request).to_ascii_lowercase();
        let head = text.split("\r\n\r\n").next().unwrap_or_default();
        match head
            .split("\r\n")
            .find_map(|line| line.strip_prefix("user-agent: "))
        {
            Some(agent) if agent.starts_with("claude-cli/") => self.by_agent.0,
            Some(_) => self.by_agent.1,
            None => self
                .script
                .lock()
                .unwrap()
                .pop_front()
                .expect("a request beyond the script reached the target"),
        }
    }
}

/// One connection: records what the gateway writes and, at its first read, chooses its answer.
struct Connection {
    fake: Arc<Fake>,
    written: Vec<u8>,
    answer: Option<io::Cursor<Vec<u8>>>,
}

impl Read for Connection {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.answer.is_none() {
            self.fake.seen.lock().unwrap().push(self.written.clone());
            self.answer = Some(io::Cursor::new(self.fake.answer_for(&self.written).bytes()));
        }
        self.answer.as_mut().unwrap().read(buffer)
    }
}

impl Write for Connection {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.written.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl RelayStream for Connection {}

struct Target(Arc<Fake>);

impl RelayTarget for Target {
    fn authority(&self) -> &'static str {
        "messages-proxy.invalid:7000"
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        Ok(Box::new(Connection {
            fake: Arc::clone(&self.0),
            written: Vec::new(),
            answer: None,
        }))
    }
}

struct Source {
    fake: Arc<Fake>,
    phase: ModelPhase,
}

impl RelayTargets for Source {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Ok(Box::new(Target(Arc::clone(&self.fake))))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}

    fn status(&self, _alias: &str) -> Option<ModelStatus> {
        Some(match self.phase {
            ModelPhase::Unreachable => ModelStatus {
                phase: ModelPhase::Unreachable,
                sub_phase: Some("status-404".to_owned()),
                profile: None,
            },
            phase => ModelStatus::new(phase),
        })
    }
}

/// How the model is relayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Path {
    /// The client's own headers forwarded (`RelayModel::with_client_credentials`).
    Forwarding,
    /// The byte relay, which forwards no client header.
    Bytes,
}

/// A gateway open on loopback relaying [`MODEL`] on `path` to the fake.
struct Served {
    gateway: Option<llm_gateway::GatewayHandle>,
    fake: Arc<Fake>,
    path: Path,
}

impl Served {
    fn start(path: Path, by_agent: (Answer, Answer), phase: ModelPhase) -> Self {
        let fake = Arc::new(Fake {
            by_agent,
            script: Mutex::new(VecDeque::new()),
            seen: Mutex::new(Vec::new()),
        });
        let targets: Arc<dyn RelayTargets> = Arc::new(Source {
            fake: Arc::clone(&fake),
            phase,
        });
        let mut model = RelayModel::new(
            Label::new(MODEL).unwrap(),
            Label::new(MODEL).unwrap(),
            vec![Wire::Messages],
        )
        .unwrap()
        .with_kind(ModelKind::Hosted);
        if path == Path::Forwarding {
            model = model.with_client_credentials();
        }
        let relay = Relay::new(vec![model], targets).unwrap();
        let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
        config.ingress_auth = IngressAuth::None;
        let gateway = Gateway::bind_with_relay(
            config,
            Arc::new(NoOwnerVerifier),
            RouteInventory::new(Vec::new()).unwrap(),
            relay,
        )
        .unwrap();
        gateway.mark_ready();
        Self {
            gateway: Some(gateway),
            fake,
            path,
        }
    }

    fn address(&self) -> SocketAddr {
        self.gateway.as_ref().unwrap().local_addr()
    }

    /// One call from `client`; on the byte relay the target answers it `scripted`. Returns the
    /// whole answer and the body the client sent.
    fn call(&self, client: Client, scripted: Answer) -> (String, String) {
        self.fake.script.lock().unwrap().push_back(scripted);
        let body = format!(
            r#"{{"model":"{MODEL}","max_tokens":64,"messages":[{{"role":"user","content":"which client am I? {client:?}"}}]}}"#
        );
        let request = format!(
            "POST /v1/messages HTTP/1.1\r\nhost: gateway\r\nuser-agent: {}\r\nauthorization: Bearer client-own-token-0001\r\nanthropic-version: 2023-06-01\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            client.user_agent(),
            body.len()
        );
        (exchange(self.address(), &request), body)
    }

    /// Calls in order, each from its client, each answer checked to have reached the client: on
    /// the byte relay the scripted one, on the forwarding path the one the fake chose by agent.
    fn calls(&self, calls: &[Call]) {
        for (client, scripted) in calls {
            let (answer, _) = self.call(*client, *scripted);
            let answered = match (self.path, client) {
                (Path::Bytes, _) => *scripted,
                (Path::Forwarding, Client::ClaudeCode) => self.fake.by_agent.0,
                (Path::Forwarding, Client::Other) => self.fake.by_agent.1,
            };
            let expected = if answered == Answer::Ok {
                "HTTP/1.1 200 "
            } else {
                "HTTP/1.1 429 "
            };
            assert!(answer.starts_with(expected), "{client:?}: {answer}");
        }
    }

    /// The status list, one line of JSON whose one entry is [`MODEL`]'s.
    fn status_text(&self) -> String {
        let answer = exchange(
            self.address(),
            "GET /v1/status HTTP/1.1\r\nhost: gateway\r\n\r\n",
        );
        assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
        body(&answer).to_owned()
    }

    /// The status list's entry: its `phase` and its `reason`, `None` when it has none.
    fn status_entry(&self) -> Entry {
        let text = self.status_text();
        assert!(
            text.starts_with(&format!(
                "{{\"format\":\"llm-gateway.status/1\",\"models\":[{{\"model\":\"{MODEL}\","
            )),
            "{text}"
        );
        Entry {
            phase: string_after(&text, "\"phase\":"),
            reason: string_after(&text, "\"reason\":"),
            text,
        }
    }

    /// The dashboard snapshot's route row of [`MODEL`]: its `reason`, written right after
    /// `route`, as JSON (`null` or a string).
    fn route_reason(&self) -> String {
        let answer = exchange(
            self.address(),
            "GET /dashboard/snapshot HTTP/1.1\r\nhost: gateway\r\n\r\n",
        );
        assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
        let snapshot = body(&answer);
        let start = format!("{{\"route\":\"{MODEL}\",\"reason\":");
        let row = snapshot
            .split_once(&start)
            .unwrap_or_else(|| panic!("no route row starting {start}: {snapshot}"))
            .1;
        row.split_once(",\"calls\":")
            .unwrap_or_else(|| panic!("a route row without calls: {row}"))
            .0
            .to_owned()
    }

    fn seen(&self) -> Vec<String> {
        self.fake
            .seen
            .lock()
            .unwrap()
            .iter()
            .map(|request| String::from_utf8_lossy(request).into_owned())
            .collect()
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        if let Some(gateway) = self.gateway.take() {
            gateway.shutdown();
        }
    }
}

fn exchange(address: SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut answer = Vec::new();
    drop(stream.read_to_end(&mut answer));
    String::from_utf8_lossy(&answer).into_owned()
}

fn body(answer: &str) -> &str {
    answer.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

/// One entry of the status list, read from its text.
struct Entry {
    phase: Option<String>,
    reason: Option<String>,
    text: String,
}

/// The string value written after the first `key` in `text`, `None` when the key is absent.
fn string_after(text: &str, key: &str) -> Option<String> {
    let rest = text.split_once(key)?.1.strip_prefix('"')?;
    Some(rest.split_once('"')?.0.to_owned())
}

/// Two calls of each client, alternating: Claude Code answered 2xx, the other rate limited.
const CLAUDE_CODE_ONLY: [(Client, Answer); 4] = [
    (Client::ClaudeCode, Answer::Ok),
    (Client::Other, Answer::RateLimited),
    (Client::ClaudeCode, Answer::Ok),
    (Client::Other, Answer::RateLimited),
];

/// The status entry's `reason` and the dashboard route row's are both `expected`.
fn assert_reason(served: &Served, expected: Option<&str>) {
    let entry = served.status_entry();
    assert_eq!(entry.reason.as_deref(), expected, "{}", entry.text);
    assert_eq!(
        served.route_reason(),
        expected.map_or_else(|| "null".to_owned(), |reason| format!("\"{reason}\""))
    );
}

/// The side that holds, on the route that forwards the client's headers: the status keeps the
/// model `ready` and callable and adds the reason after `callable`, the dashboard's route row
/// names it, and the other client's call reached the target exactly as it was sent and its `429`
/// reached the client.
#[test]
fn served_clients_claude_code_answered_and_others_rate_limited_adds_the_reason() {
    let served = Served::start(
        Path::Forwarding,
        (Answer::Ok, Answer::RateLimited),
        ModelPhase::Ready,
    );
    served.calls(&CLAUDE_CODE_ONLY[..3]);
    let (answer, sent) = served.call(Client::Other, Answer::RateLimited);
    assert!(answer.starts_with("HTTP/1.1 429 "), "{answer}");
    assert!(answer.contains("rate_limit_error"), "{answer}");
    assert_eq!(
        served.status_text(),
        format!(
            "{{\"format\":\"llm-gateway.status/1\",\"models\":[{{\"model\":\"{MODEL}\",\"kind\":\"hosted\",\"wires\":[\"messages\"],\"phase\":\"ready\",\"callable\":true,\"reason\":\"{REASON}\"}}]}}"
        )
    );
    assert_reason(&served, Some(REASON));
    let seen = served.seen();
    assert_eq!(seen.len(), 4, "{seen:?}");
    let last = &seen[3];
    let (head, relayed_body) = last.split_once("\r\n\r\n").unwrap();
    assert_eq!(relayed_body, sent, "the other client's body was altered");
    for line in [
        format!("user-agent: {OTHER}"),
        "authorization: Bearer client-own-token-0001".to_owned(),
        "anthropic-version: 2023-06-01".to_owned(),
    ] {
        assert_eq!(
            head.split("\r\n")
                .filter(|candidate| *candidate == line)
                .count(),
            1,
            "{line:?} did not reach the target once, as sent: {head}"
        );
    }
}

/// The same side on the byte relay, where the record is the gateway's alone: the target sees no
/// `user-agent` and answers from the script, and the reason is decided from the client
/// `user-agent` the gateway read and the answer it relayed.
#[test]
fn served_clients_the_byte_relay_decides_from_the_gateways_own_record() {
    let served = Served::start(Path::Bytes, (Answer::Ok, Answer::Ok), ModelPhase::Ready);
    served.calls(&CLAUDE_CODE_ONLY);
    assert_reason(&served, Some(REASON));
    for request in served.seen() {
        assert!(
            !request.to_ascii_lowercase().contains("\r\nuser-agent:"),
            "the byte relay forwarded a client header: {request}"
        );
    }
}

/// The other side: a record that does not show the rule adds no reason. Another client answered
/// 2xx, another client's 429 of another error type, a Claude Code call that was rate limited too,
/// and a model whose own status is not `ready` whatever its record holds.
#[test]
fn served_clients_a_record_that_breaks_the_rule_adds_no_reason() {
    let cases: [(&str, Vec<Call>, ModelPhase); 4] = [
        (
            "another client answered 2xx",
            vec![
                (Client::ClaudeCode, Answer::Ok),
                (Client::Other, Answer::RateLimited),
                (Client::ClaudeCode, Answer::Ok),
                (Client::Other, Answer::RateLimited),
                (Client::Other, Answer::Ok),
            ],
            ModelPhase::Ready,
        ),
        (
            "another client's 429 is not a rate_limit_error",
            vec![
                (Client::ClaudeCode, Answer::Ok),
                (Client::Other, Answer::Overloaded),
                (Client::ClaudeCode, Answer::Ok),
                (Client::Other, Answer::Overloaded),
            ],
            ModelPhase::Ready,
        ),
        (
            "a Claude Code call was rate limited",
            vec![
                (Client::ClaudeCode, Answer::Ok),
                (Client::Other, Answer::RateLimited),
                (Client::ClaudeCode, Answer::RateLimited),
                (Client::ClaudeCode, Answer::Ok),
                (Client::Other, Answer::RateLimited),
            ],
            ModelPhase::Ready,
        ),
        (
            "the model is unreachable",
            CLAUDE_CODE_ONLY.to_vec(),
            ModelPhase::Unreachable,
        ),
    ];
    for (case, calls, phase) in cases {
        let served = Served::start(Path::Bytes, (Answer::Ok, Answer::Ok), phase);
        served.calls(&calls);
        let entry = served.status_entry();
        assert_eq!(entry.reason, None, "{case}: {}", entry.text);
        if phase == ModelPhase::Ready {
            assert_eq!(
                entry.phase.as_deref(),
                Some("ready"),
                "{case}: {}",
                entry.text
            );
            assert_eq!(served.route_reason(), "null", "{case}");
        } else {
            assert_eq!(
                entry.phase.as_deref(),
                Some("unreachable"),
                "{case}: {}",
                entry.text
            );
        }
    }
}

/// Too few calls to say anything: with fewer than the published `served-clients-min-calls` of
/// either client in the record, no reason is added, however the calls were answered.
#[test]
fn served_clients_too_few_calls_add_no_reason() {
    let cases: [(&str, Vec<Call>); 3] = [
        ("no call", vec![]),
        (
            "one of each",
            vec![
                (Client::ClaudeCode, Answer::Ok),
                (Client::Other, Answer::RateLimited),
            ],
        ),
        (
            "Claude Code only",
            vec![
                (Client::ClaudeCode, Answer::Ok),
                (Client::ClaudeCode, Answer::Ok),
                (Client::ClaudeCode, Answer::Ok),
            ],
        ),
    ];
    for (case, calls) in cases {
        let served = Served::start(Path::Bytes, (Answer::Ok, Answer::Ok), ModelPhase::Ready);
        served.calls(&calls);
        let entry = served.status_entry();
        assert_eq!(
            entry.phase.as_deref(),
            Some("ready"),
            "{case}: {}",
            entry.text
        );
        assert_eq!(entry.reason, None, "{case}: {}", entry.text);
    }
}

/// `served-clients-min-calls`, measured: whether `n` calls of each client, Claude Code answered
/// 2xx and the other rate limited, add the reason.
pub(crate) fn n_calls_of_each_add_the_reason(n: u64) -> bool {
    let served = Served::start(Path::Bytes, (Answer::Ok, Answer::Ok), ModelPhase::Ready);
    for _ in 0..n {
        served.calls(&[
            (Client::ClaudeCode, Answer::Ok),
            (Client::Other, Answer::RateLimited),
        ]);
    }
    served.status_entry().reason.as_deref() == Some(REASON)
}

/// `served-clients-calls-kept`, measured: after one call that breaks the rule (another client
/// answered 2xx) and `n - 1` calls that keep it, whether that first call is still in the record,
/// so no reason is added.
pub(crate) fn the_first_of_n_calls_is_kept(n: u64) -> bool {
    let served = Served::start(Path::Bytes, (Answer::Ok, Answer::Ok), ModelPhase::Ready);
    served.calls(&[(Client::Other, Answer::Ok)]);
    for index in 1..n {
        let call = if index % 2 == 0 {
            (Client::ClaudeCode, Answer::Ok)
        } else {
            (Client::Other, Answer::RateLimited)
        };
        served.calls(&[call]);
    }
    served.status_entry().reason.is_none()
}
