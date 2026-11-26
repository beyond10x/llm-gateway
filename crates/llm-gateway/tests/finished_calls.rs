//! One finished-call value per model call, and a bounded copy of its bytes when the relay
//! declares one (story:finished-call-facts; `FinishedCall`, `CallBytes` and `CallBytesBound` in
//! `spec/domains/telemetry.yaml`, docs/gateway.md "Finished calls").
//!
//! Every upstream here is a fake on loopback, port 0: it reads each request and writes a scripted
//! answer. The translator is a fixture that reaches that upstream and writes a fixed translated
//! answer, so the relay's half of the contract is held with no llm crate in the build.

use crate::upstream::{self, Upstream as Shared};
use llm_gateway::{
    AnswerBytes, AnswerFraming, CallBytes, CallEnding, CallPricing, FinishedCall, FinishedCalls,
    Gateway, GatewayConfig, GatewayHandle, Harness, IngressAuth, Label, Metrics, NoOwnerVerifier,
    OwnerToken, OwnerVerifier, RefusalCode, Relay, RelayModel, RelayTarget, RelayTargets,
    RouteInventory, SharedSecretVerifier, TargetRefusal, TranslatedAnswer, Translation,
    TranslationFailure, TranslationRequest, Translator, Usage, UsageRecord, UsageRecords, Wire,
};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const OWNER: &str = "owner-token-finished-calls-0123456789abcdefgh";
const WAIT: Duration = Duration::from_secs(10);
/// The id line every answer to a model call carries; it differs per call.
const ID_LINE: &[u8] = b"x-llm-gateway-call: ";

// --- The fake upstream -------------------------------------------------------------------------

/// What the upstream writes on one connection, after reading its request.
#[derive(Clone)]
enum Reply {
    /// These pieces, a short pause between them, then the connection is closed.
    Pieces(Vec<Vec<u8>>),
    /// The first piece, then `tick` every 20 ms until a write fails or 4 s have passed: a stream
    /// that outlasts a client that leaves.
    Ticking(Vec<u8>, Vec<u8>),
    /// Nothing: the connection is closed without an answer.
    Close,
}

/// A loopback upstream answering its rig's calls with [`Reply`]s in the order their requests
/// arrive: a shared upstream (`crate::upstream`), which answers only requests carrying its token
/// and closes any other connection unanswered, without using a reply.
struct Upstream {
    authority: String,
    shared: Shared,
}

impl Upstream {
    fn start(replies: Vec<Reply>) -> Self {
        let shared = Shared::scripted(
            replies
                .into_iter()
                .map(|reply| -> upstream::Reply {
                    Box::new(move |connection| answer(connection, reply))
                })
                .collect(),
        );
        Self {
            authority: shared.authority().to_owned(),
            shared,
        }
    }
}

fn answer(connection: &mut TcpStream, reply: Reply) {
    match reply {
        Reply::Pieces(pieces) => {
            for piece in pieces {
                if connection.write_all(&piece).is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(2));
            }
        }
        Reply::Ticking(first, tick) => {
            if connection.write_all(&first).is_err() {
                return;
            }
            let until = Instant::now() + Duration::from_secs(4);
            while Instant::now() < until {
                thread::sleep(Duration::from_millis(20));
                if connection.write_all(&tick).is_err() {
                    return;
                }
            }
        }
        Reply::Close => {}
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// A `200` JSON answer framed by length.
fn json(body: &str) -> Reply {
    Reply::Pieces(vec![
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes(),
    ])
}

/// The head of a chunked event stream.
fn stream_head() -> Vec<u8> {
    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n"
        .to_vec()
}

fn chunk(piece: &str) -> Vec<u8> {
    format!("{:x}\r\n{piece}\r\n", piece.len()).into_bytes()
}

/// A `200` event stream, each of `pieces` one chunk.
fn events(pieces: &[&str]) -> Reply {
    let mut out = vec![stream_head()];
    out.extend(pieces.iter().map(|piece| chunk(piece)));
    out.push(b"0\r\n\r\n".to_vec());
    Reply::Pieces(out)
}

// --- Targets, the translator and the ports -----------------------------------------------------

/// Targets on one upstream, whose requests carry its token.
struct Source(upstream::Target);

impl Source {
    fn of(upstream: &Upstream) -> Self {
        Self(upstream.shared.target(WAIT))
    }
}

impl RelayTargets for Source {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Ok(Box::new(self.0.clone()))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

/// What the fixture translator writes once it has read the upstream's answer.
#[derive(Clone, Copy)]
enum Translated {
    /// A head and these events, each one piece.
    Stream(&'static [&'static str]),
    /// A head and one event, then its wire's terminal error event, told to the relay under this
    /// code, as the binary's translator ends an answer whose upstream failed after output.
    FailAfterHead(RefusalCode),
}

/// The pieces a translated stream writes.
const TRANSLATED: [&str; 2] = [
    "event: message_start\ndata: {\"type\":\"message_start\"}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
];

/// A translated stream that ends in the messages wire's error event, its upstream's own error,
/// with no failure told to the relay.
const TRANSLATED_ERROR: [&str; 2] = [
    "event: message_start\ndata: {\"type\":\"message_start\"}\n\n",
    WIRE_ERROR,
];

/// The messages wire's terminal error event.
const WIRE_ERROR: &str = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";

struct Fixture(Translated);

impl Translator for Fixture {
    fn prepare(
        &self,
        _request: &TranslationRequest<'_>,
    ) -> Result<Box<dyn Translation>, RefusalCode> {
        Ok(Box::new(Run(self.0)))
    }
}

struct Run(Translated);

impl Translation for Run {
    fn run(
        self: Box<Self>,
        target: &dyn RelayTarget,
        answer: &mut dyn TranslatedAnswer,
    ) -> Result<(), TranslationFailure> {
        let mut connection = target
            .connect()
            .map_err(|_| TranslationFailure::UpstreamFailed)?;
        let mut lines = String::new();
        for header in target.headers() {
            lines.push_str(header.name());
            lines.push_str(": ");
            lines.push_str(header.value());
            lines.push_str("\r\n");
        }
        connection
            .write_all(
                format!(
                    "POST /v1/chat/completions HTTP/1.1\r\nhost: up\r\n{lines}content-length: 2\r\n\r\n{{}}"
                )
                .as_bytes(),
            )
            .map_err(|_| TranslationFailure::UpstreamFailed)?;
        let mut upstream = Vec::new();
        drop(connection.read_to_end(&mut upstream));
        answer.target_status(200);
        answer.head(200, "text/event-stream", AnswerFraming::Stream);
        match self.0 {
            Translated::Stream(pieces) => {
                for piece in pieces {
                    answer.body(piece.as_bytes());
                }
            }
            Translated::FailAfterHead(code) => {
                answer.body(TRANSLATED[0].as_bytes());
                answer.body(WIRE_ERROR.as_bytes());
                answer.failed(code);
            }
        }
        Ok(())
    }
}

/// The finished calls a port was handed, each with its copy.
type Held = Vec<(FinishedCall, Option<CallBytes>)>;

/// Every value the port was handed, in the order the calls ended.
#[derive(Default)]
struct Port {
    held: Mutex<Held>,
    /// A value of this ending is handed on only after this pause, as when the thread of its call
    /// is not scheduled for as long under load.
    hold: Option<(CallEnding, Duration)>,
}

impl FinishedCalls for Port {
    fn finished(&self, call: &FinishedCall, bytes: Option<&CallBytes>) {
        if let Some((ending, pause)) = self.hold
            && call.ending == ending
        {
            thread::sleep(pause);
        }
        self.held
            .lock()
            .unwrap()
            .push((call.clone(), bytes.cloned()));
    }
}

impl Port {
    /// The values once there are at least `count`, or what there is after [`WAIT`].
    fn wait(&self, count: usize) -> Held {
        let deadline = Instant::now() + WAIT;
        loop {
            let held = self.held.lock().unwrap().clone();
            if held.len() >= count || Instant::now() > deadline {
                return held;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}

#[derive(Default)]
struct Records(Mutex<Vec<UsageRecord>>);

impl UsageRecords for Records {
    fn record(&self, record: &UsageRecord) {
        self.0.lock().unwrap().push(record.clone());
    }
}

impl Records {
    fn all(&self) -> Vec<UsageRecord> {
        self.0.lock().unwrap().clone()
    }
}

/// A price every call of a priced model costs, whatever it reported.
struct Price;

impl CallPricing for Price {
    fn cost_usd(&self, _wire: Wire, _usage: Usage) -> Option<String> {
        Some("0.25".to_owned())
    }
}

// --- The gateway ------------------------------------------------------------------------------

/// How a rig composes its gateway.
#[derive(Clone, Copy, Default)]
struct Options {
    call_bytes: Option<usize>,
    translated: Option<Translated>,
    /// `IngressAuth::None`: a client-credential model passes its client's request through.
    open: bool,
    max_concurrent_requests: Option<u64>,
    /// [`Port::hold`].
    hold: Option<(CallEnding, Duration)>,
}

/// A gateway over one fake upstream, its port and its usage records. Its models: `code` on every
/// wire, priced; `chat-only` on chat, which a translator serves on the other wires; `sub`, a
/// client-credential model on messages.
struct Rig {
    handle: Option<GatewayHandle>,
    addr: SocketAddr,
    port: Arc<Port>,
    records: Arc<Records>,
    metrics: Arc<Metrics>,
    open: bool,
    upstream: Upstream,
}

impl Rig {
    fn new(replies: Vec<Reply>, options: Options) -> Self {
        let upstream = Upstream::start(replies);
        let label = |value: &str| Label::new(value).unwrap();
        let models = vec![
            RelayModel::new(
                label("code"),
                label("served-code"),
                vec![Wire::Chat, Wire::Responses, Wire::Messages],
            )
            .unwrap()
            .with_pricing(Arc::new(Price)),
            RelayModel::new(label("chat-only"), label("served-chat"), vec![Wire::Chat]).unwrap(),
            RelayModel::new(label("sub"), label("served-sub"), vec![Wire::Messages])
                .unwrap()
                .with_client_credentials(),
        ];
        let port = Arc::new(Port {
            hold: options.hold,
            ..Port::default()
        });
        let records = Arc::new(Records::default());
        let metrics = Arc::new(Metrics::default());
        let mut relay = Relay::new(models, Arc::new(Source::of(&upstream)))
            .unwrap()
            .with_metrics(Arc::clone(&metrics))
            .with_records(Arc::clone(&records) as Arc<dyn UsageRecords>)
            .with_finished_calls(Arc::clone(&port) as Arc<dyn FinishedCalls>);
        if let Some(bound) = options.call_bytes {
            relay = relay.with_call_bytes(bound);
        }
        if let Some(translated) = options.translated {
            relay = relay.with_translator(Arc::new(Fixture(translated)));
        }
        let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
        if let Some(most) = options.max_concurrent_requests {
            config.max_concurrent_requests = most;
        }
        let verifier: Arc<dyn OwnerVerifier> = if options.open {
            config.ingress_auth = IngressAuth::None;
            Arc::new(NoOwnerVerifier)
        } else {
            Arc::new(
                SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap())
                    .unwrap(),
            )
        };
        let handle = Gateway::bind_with_relay(
            config,
            verifier,
            RouteInventory::new(Vec::new()).unwrap(),
            relay,
        )
        .unwrap();
        handle.mark_ready();
        Self {
            addr: handle.local_addr(),
            handle: Some(handle),
            port,
            records,
            metrics,
            open: options.open,
            upstream,
        }
    }

    /// Sends a `POST` of `body` to `path` with `headers` (each line with its CRLF), as the owner
    /// unless the gateway is open, and reads the answer to its end.
    fn call(&self, path: &str, body: &str, headers: &str) -> Vec<u8> {
        self.raw(&self.request("HTTP/1.1", path, body, headers), None)
    }

    fn request(&self, version: &str, path: &str, body: &str, headers: &str) -> Vec<u8> {
        let owner = if self.open {
            String::new()
        } else {
            format!("authorization: Bearer {OWNER}\r\n")
        };
        format!(
            "POST {path} {version}\r\nhost: gateway\r\n{owner}{headers}content-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    /// Writes `request` and reads the answer to its end, or only until `leave_after` bytes have
    /// arrived, then closes.
    fn raw(&self, request: &[u8], leave_after: Option<usize>) -> Vec<u8> {
        let mut stream = TcpStream::connect(self.addr).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        stream.write_all(request).unwrap();
        let mut received = Vec::new();
        let mut piece = [0_u8; 4096];
        loop {
            match stream.read(&mut piece) {
                Ok(0) | Err(_) => break,
                Ok(read) => received.extend_from_slice(&piece[..read]),
            }
            if leave_after.is_some_and(|most| received.len() >= most) {
                break;
            }
        }
        received
    }

    fn scrape(&self) -> String {
        let request = format!(
            "GET /metrics HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\n\r\n"
        );
        String::from_utf8(self.raw(request.as_bytes(), None)).unwrap()
    }

    fn finished(&self, count: usize) -> Held {
        let held = self.port.wait(count);
        assert_eq!(held.len(), count, "the port was handed {held:#?}");
        held
    }

    fn snapshot(&self) -> String {
        self.handle.as_ref().unwrap().dashboard().snapshot()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.shutdown();
        }
    }
}

/// `answer` without its call id line, which differs on every call.
fn without_id(answer: &[u8]) -> Vec<u8> {
    let Some(at) = find(answer, ID_LINE) else {
        return answer.to_vec();
    };
    let end = at + find(&answer[at..], b"\r\n").unwrap() + 2;
    [&answer[..at], &answer[end..]].concat()
}

fn status(answer: &[u8]) -> u16 {
    String::from_utf8_lossy(answer)
        .split(' ')
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or(0)
}

const CHAT: &str = "/v1/chat/completions";
const MESSAGES: &str = "/v1/messages";
const RESPONSES: &str = "/v1/responses";
const CHAT_ANSWER: &str = r#"{"id":"c-1","usage":{"prompt_tokens":10,"completion_tokens":4}}"#;
/// A messages stream: two named events, a comment line, a two-line data field and CRLF line ends
/// in its last event.
const MESSAGES_STREAM: [&str; 4] = [
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":9,\"output_tokens\":1}}}\n\n",
    ": a comment\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\n",
    "data: \"delta\":{\"text\":\"hi\"}}\n\n",
    "event: message_stop\r\ndata: {\"type\":\"message_stop\"}\r\n\r\n",
];

fn payloads() -> Vec<Vec<u8>> {
    vec![
        b"{\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":9,\"output_tokens\":1}}}"
            .to_vec(),
        b"{\"type\":\"content_block_delta\",\n\"delta\":{\"text\":\"hi\"}}".to_vec(),
        b"{\"type\":\"message_stop\"}".to_vec(),
    ]
}

// --- Once per call, never for a call without one -----------------------------------------------

#[test]
fn a_relayed_call_hands_one_value_with_its_record_s_id_path_route_and_times() {
    let rig = Rig::new(vec![json(CHAT_ANSWER)], Options::default());
    let answer = rig.call(CHAT, r#"{"model":"code","messages":[]}"#, "");
    assert_eq!(status(&answer), 200);
    let held = rig.finished(1);
    let (call, bytes) = &held[0];
    let record = &rig.records.all()[0];
    assert_eq!(call.call_id, record.call);
    assert_eq!(call.path, CHAT);
    assert_eq!(call.wire, Wire::Chat);
    assert_eq!(call.route.as_deref(), Some("code"));
    assert_eq!(
        call.upstream.as_deref(),
        Some(rig.upstream.authority.as_str())
    );
    assert_eq!(call.attempts, 1);
    assert!(
        call.queue_wait_ms.is_some() && call.ttft_ms.is_some(),
        "{call:?}"
    );
    assert_eq!(call.ending, CallEnding::Complete);
    assert_eq!(call.finished_ms - call.started_ms, record.duration_ms);
    assert_eq!(*bytes, None, "no copy without with_call_bytes");
    // Exactly once: a second look finds the same one value.
    thread::sleep(Duration::from_millis(50));
    assert_eq!(rig.port.wait(0).len(), 1);
}

#[test]
fn each_wire_and_path_hands_one_value_with_its_inbound_wire() {
    let translated = Options {
        translated: Some(Translated::Stream(&TRANSLATED)),
        ..Options::default()
    };
    let rig = Rig::new(
        vec![json("{}"), json("{}"), events(&MESSAGES_STREAM), json("{}")],
        translated,
    );
    rig.call(CHAT, r#"{"model":"code"}"#, "");
    rig.call(RESPONSES, r#"{"model":"code"}"#, "");
    rig.call(MESSAGES, r#"{"model":"code","stream":true}"#, "");
    rig.call(MESSAGES, r#"{"model":"chat-only","messages":[]}"#, "");
    let held = rig.finished(4);
    let seen: Vec<(&str, Wire, Option<&str>)> = held
        .iter()
        .map(|(call, _)| (call.path.as_str(), call.wire, call.route.as_deref()))
        .collect();
    assert_eq!(
        seen,
        [
            (CHAT, Wire::Chat, Some("code")),
            (RESPONSES, Wire::Responses, Some("code")),
            (MESSAGES, Wire::Messages, Some("code")),
            (MESSAGES, Wire::Messages, Some("chat-only")),
        ]
    );
    let translated = &held[3].0;
    assert_eq!(translated.ending, CallEnding::Complete);
    assert_eq!(translated.attempts, 1);
    let records = rig.records.all();
    let ids: Vec<&str> = held.iter().map(|(call, _)| call.call_id.as_str()).collect();
    let recorded: Vec<&str> = records.iter().map(|record| record.call.as_str()).collect();
    assert_eq!(ids, recorded);

    let open = Rig::new(
        vec![events(&MESSAGES_STREAM)],
        Options {
            open: true,
            ..Options::default()
        },
    );
    let answer = open.call(
        MESSAGES,
        r#"{"model":"sub","stream":true}"#,
        "x-api-key: client-key\r\nx-stainless-retry-count: 0\r\n",
    );
    assert_eq!(status(&answer), 200);
    let passed = &open.finished(1)[0].0;
    assert_eq!(passed.route.as_deref(), Some("sub"));
    assert_eq!(passed.attempts, 1);
    assert_eq!(passed.ending, CallEnding::Complete);
    assert!(passed.queue_wait_ms.is_some() && passed.ttft_ms.is_some());
}

#[test]
fn a_refused_call_hands_one_value_and_a_count_a_shed_and_an_unauthenticated_call_none() {
    let rig = Rig::new(vec![json(r#"{"input_tokens":3}"#)], Options::default());
    let refused = rig.call(CHAT, r#"{"model":"nobody"}"#, "");
    assert_eq!(status(&refused), 404);
    let counted = rig.call(
        "/v1/messages/count_tokens",
        r#"{"model":"code","messages":[]}"#,
        "",
    );
    assert_eq!(status(&counted), 200);
    let request = format!(
        "POST {CHAT} HTTP/1.1\r\nhost: gateway\r\ncontent-length: 16\r\n\r\n{{\"model\":\"code\"}}"
    );
    assert_eq!(status(&rig.raw(request.as_bytes(), None)), 401);
    // The count made a usage record, and still no value.
    assert_eq!(rig.records.all().len(), 2);
    thread::sleep(Duration::from_millis(50));
    let held = rig.finished(1);
    let call = &held[0].0;
    assert_eq!(call.ending, CallEnding::Refused(RefusalCode::ModelUnknown));
    assert_eq!(call.route, None);
    assert_eq!(call.upstream, None);
    assert_eq!(call.attempts, 0);
    assert_eq!((call.queue_wait_ms, call.ttft_ms), (None, None));

    let shedding = Rig::new(
        Vec::new(),
        Options {
            max_concurrent_requests: Some(0),
            ..Options::default()
        },
    );
    assert_eq!(status(&shedding.call(CHAT, r#"{"model":"code"}"#, "")), 503);
    thread::sleep(Duration::from_millis(50));
    assert!(shedding.port.wait(0).is_empty());
    assert!(shedding.records.all().is_empty());
}

// --- The session -------------------------------------------------------------------------------

/// The `session` of every finished call the dashboard's snapshot lists, oldest first.
fn dashboard_sessions(snapshot: &str) -> Vec<String> {
    let calls =
        &snapshot[snapshot.find("\"calls\":").unwrap()..snapshot.find("\"in_flight\":").unwrap()];
    let mut sessions: Vec<String> = calls
        .split("\"session\":\"")
        .skip(1)
        .map(|rest| rest.split('"').next().unwrap().to_owned())
        .collect();
    sessions.reverse();
    sessions
}

#[test]
fn the_session_hash_and_harness_are_the_dashboard_s_for_each_header() {
    // Each hash is agentspec's `wire::session_hash(source, value)`: the first 12 hex digits of
    // SHA-256 over `llm-gateway.session/1\0<source>\0<value>`.
    let cases: [(&str, &str, Harness); 5] = [
        (
            "x-claude-code-session-id: s-claude\r\n",
            "e5333c6d3823",
            Harness::ClaudeCode,
        ),
        (
            "session-id: s-codex\r\nuser-agent: codex_cli_rs/0.63.0\r\n",
            "a32027201cc7",
            Harness::Codex,
        ),
        ("session_id: s-under\r\n", "15eb0983e6e7", Harness::Codex),
        (
            "x-opencode-session: s-open\r\nuser-agent: opencode/1.0\r\n",
            "c6bdf2f25774",
            Harness::Other,
        ),
        // No header names it: the user agent and the client's address.
        ("user-agent: curl/8.0\r\n", "5be58fada9ac", Harness::Other),
    ];
    let rig = Rig::new(vec![json("{}"); cases.len()], Options::default());
    for (headers, _, _) in &cases {
        rig.call(CHAT, r#"{"model":"code"}"#, headers);
    }
    let held = rig.finished(cases.len());
    let hashes: Vec<&str> = held
        .iter()
        .map(|(call, _)| call.session_hash.as_str())
        .collect();
    let expected: Vec<&str> = cases.iter().map(|(_, hash, _)| *hash).collect();
    assert_eq!(hashes, expected);
    let harnesses: Vec<Harness> = held.iter().map(|(call, _)| call.harness).collect();
    let named: Vec<Harness> = cases.iter().map(|(_, _, harness)| *harness).collect();
    assert_eq!(harnesses, named);
    assert_eq!(dashboard_sessions(&rig.snapshot()), expected);
}

/// Decision D93: a call refused at admission, before its headers were read into the dashboard,
/// still hands the session its header names; the dashboard shows it without one.
#[test]
fn a_call_refused_at_admission_hands_the_session_hash_of_its_header() {
    let rig = Rig::new(Vec::new(), Options::default());
    let request = format!(
        "POST {CHAT} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\nx-claude-code-session-id: s-claude\r\ncontent-length: 1\r\ntransfer-encoding: chunked\r\n\r\n"
    );
    let answer = rig.raw(request.as_bytes(), None);
    assert_eq!(status(&answer), 400, "{}", String::from_utf8_lossy(&answer));
    let held = rig.finished(1);
    let call = &held[0].0;
    assert!(
        matches!(call.ending, CallEnding::Refused(_)),
        "{:?}",
        call.ending
    );
    assert_eq!(call.session_hash, "e5333c6d3823");
    assert_eq!(call.harness, Harness::ClaudeCode);
    assert_eq!(dashboard_sessions(&rig.snapshot()), [""]);
}

// --- How a call ended ---------------------------------------------------------------------------

#[test]
fn each_ending_is_named_and_the_cost_is_present_exactly_when_the_record_s_is() {
    let first_event = [stream_head(), chunk(MESSAGES_STREAM[0])].concat();
    let rig = Rig::new(
        vec![
            json(CHAT_ANSWER),
            Reply::Ticking(first_event, chunk(MESSAGES_STREAM[2])),
            Reply::Pieces(vec![
                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100\r\n\r\n{\"partial\":"
                    .to_vec(),
            ]),
            events(&[
                MESSAGES_STREAM[0],
                "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\"}}\n\n",
            ]),
            Reply::Close,
        ],
        Options {
            // The call whose client left ends on its own thread while the next call runs.
            hold: Some((CallEnding::ClientDisconnected, Duration::from_millis(300))),
            ..Options::default()
        },
    );
    rig.call(CHAT, r#"{"model":"code"}"#, "");
    let request = rig.request(
        "HTTP/1.1",
        MESSAGES,
        r#"{"model":"code","stream":true}"#,
        "",
    );
    let left = rig.raw(&request, Some(200));
    assert_eq!(status(&left), 200);
    // The call whose client left ends when the gateway notices, on the call's own thread: the
    // next call starts once its value is held, so the values stay in the calls' order.
    assert_eq!(rig.port.wait(2).len(), 2);
    rig.call(CHAT, r#"{"model":"code"}"#, "");
    // Another process on the host reaches the upstream's port, as a test whose own listener
    // held that port before, or one that probes a port it chose, may.
    let foreign = upstream::foreign_request(&rig.upstream.authority);
    assert!(foreign.is_empty(), "{}", String::from_utf8_lossy(&foreign));
    rig.call(MESSAGES, r#"{"model":"code","stream":true}"#, "");
    rig.call(CHAT, r#"{"model":"code"}"#, "");
    rig.call(CHAT, r#"{"model":"nobody"}"#, "");
    let held = rig.finished(6);
    let endings: Vec<CallEnding> = held.iter().map(|(call, _)| call.ending).collect();
    assert_eq!(
        endings,
        [
            CallEnding::Complete,
            CallEnding::ClientDisconnected,
            CallEnding::UpstreamCut,
            CallEnding::WireError,
            CallEnding::Refused(RefusalCode::UpstreamFailed),
            CallEnding::Refused(RefusalCode::ModelUnknown),
        ]
    );
    let records = rig.records.all();
    for (call, _) in &held {
        let record = records
            .iter()
            .find(|record| record.call == call.call_id)
            .unwrap();
        assert_eq!(call.declared_cost_usd, record.cost_usd, "{call:?}");
    }
    assert_eq!(held[0].0.declared_cost_usd.as_deref(), Some("0.25"));
    // The target that closed without an answer was attempted once.
    assert_eq!(held[4].0.attempts, 1);
}

/// The ending of one translated messages call whose translator writes `translated`.
fn translated_ending(translated: Translated) -> FinishedCall {
    let rig = Rig::new(
        vec![json("{}")],
        Options {
            translated: Some(translated),
            ..Options::default()
        },
    );
    let answer = rig.call(MESSAGES, r#"{"model":"chat-only","messages":[]}"#, "");
    assert_eq!(status(&answer), 200);
    let text = String::from_utf8_lossy(&answer).into_owned();
    assert!(text.contains("overloaded_error"), "{text}");
    let mut held = rig.finished(1);
    held.remove(0).0
}

#[test]
fn a_translated_answer_that_ends_in_its_wire_s_error_is_a_wire_error() {
    for translated in [
        Translated::Stream(&TRANSLATED_ERROR),
        Translated::FailAfterHead(RefusalCode::TranslationTooLarge),
    ] {
        let call = translated_ending(translated);
        assert_eq!(call.ending, CallEnding::WireError);
        assert_eq!(call.declared_cost_usd, None);
    }
}

/// Decision D91: the translator tells `upstream-failed` when its upstream failed after the answer's
/// head, so the call was cut by its upstream, though the client read the wire's error event.
#[test]
fn a_translated_answer_whose_upstream_failed_after_its_head_is_an_upstream_cut() {
    let call = translated_ending(Translated::FailAfterHead(RefusalCode::UpstreamFailed));
    assert_eq!(call.ending, CallEnding::UpstreamCut);
    assert_eq!(call.declared_cost_usd, None);
}

// --- The byte copy -----------------------------------------------------------------------------

/// The answers a client receives on every wire and path, each with the copy `call_bytes`.
fn every_path(call_bytes: Option<usize>) -> (Vec<Vec<u8>>, Held) {
    let options = Options {
        call_bytes,
        translated: Some(Translated::Stream(&TRANSLATED)),
        ..Options::default()
    };
    let rig = Rig::new(
        vec![
            json(CHAT_ANSWER),
            events(&MESSAGES_STREAM),
            events(&MESSAGES_STREAM),
            json(r#"{"id":"r-1"}"#),
            json("{}"),
        ],
        options,
    );
    let mut answers = vec![
        rig.call(CHAT, r#"{"model":"code","messages":[]}"#, ""),
        rig.call(MESSAGES, r#"{"model":"code","stream":true}"#, ""),
    ];
    // An HTTP/1.0 client is sent the stream unframed.
    let old = rig.request(
        "HTTP/1.0",
        MESSAGES,
        r#"{"model":"code","stream":true}"#,
        "",
    );
    answers.push(rig.raw(&old, None));
    answers.push(rig.call(RESPONSES, r#"{"model":"code","input":"hi"}"#, ""));
    answers.push(rig.call(MESSAGES, r#"{"model":"chat-only","messages":[]}"#, ""));
    answers.push(rig.call(CHAT, r#"{"model":"nobody"}"#, ""));
    let held = rig.finished(6);
    drop(rig);
    let open = Rig::new(
        vec![events(&MESSAGES_STREAM)],
        Options {
            call_bytes,
            open: true,
            ..Options::default()
        },
    );
    answers.push(open.call(
        MESSAGES,
        r#"{"model":"sub","stream":true}"#,
        "x-api-key: client-key\r\nx-stainless-retry-count: 0\r\n",
    ));
    let mut held = held;
    held.extend(open.finished(1));
    (
        answers.iter().map(|answer| without_id(answer)).collect(),
        held,
    )
}

#[test]
fn the_client_s_answer_is_the_same_byte_for_byte_with_the_copy_on_on_every_wire_and_path() {
    let (off, off_held) = every_path(None);
    let (on, on_held) = every_path(Some(1 << 20));
    assert_eq!(off.len(), 7);
    for (index, (off, on)) in off.iter().zip(&on).enumerate() {
        assert!(
            status(off) >= 200,
            "answer {index}: {}",
            String::from_utf8_lossy(off)
        );
        assert_eq!(
            String::from_utf8_lossy(off),
            String::from_utf8_lossy(on),
            "answer {index}"
        );
    }
    assert!(off_held.iter().all(|(_, bytes)| bytes.is_none()));
    assert!(on_held.iter().all(|(_, bytes)| bytes.is_some()));
}

#[test]
fn each_copy_is_the_request_and_the_answer_as_the_client_received_it() {
    let (_, held) = every_path(Some(1 << 20));
    let copy = |index: usize| held[index].1.clone().unwrap();
    assert_eq!(
        copy(0),
        CallBytes::Copied {
            request: br#"{"model":"code","messages":[]}"#.to_vec(),
            answer: Some(AnswerBytes::Body(CHAT_ANSWER.as_bytes().to_vec())),
        }
    );
    // A stream, chunked or unframed: the data payloads in order.
    for index in [1, 2] {
        assert_eq!(
            copy(index),
            CallBytes::Copied {
                request: br#"{"model":"code","stream":true}"#.to_vec(),
                answer: Some(AnswerBytes::Events(payloads())),
            },
            "answer {index}"
        );
    }
    // A translated call: the inbound request, and the answer as translated for the client.
    assert_eq!(
        copy(4),
        CallBytes::Copied {
            request: br#"{"model":"chat-only","messages":[]}"#.to_vec(),
            answer: Some(AnswerBytes::Events(vec![
                b"{\"type\":\"message_start\"}".to_vec(),
                b"{\"type\":\"message_stop\"}".to_vec(),
            ])),
        }
    );
    // A refusal: the request and no answer.
    assert_eq!(
        copy(5),
        CallBytes::Copied {
            request: br#"{"model":"nobody"}"#.to_vec(),
            answer: None,
        }
    );
    // A passthrough stream, as the client received it.
    assert_eq!(
        copy(6),
        CallBytes::Copied {
            request: br#"{"model":"sub","stream":true}"#.to_vec(),
            answer: Some(AnswerBytes::Events(payloads())),
        }
    );
}

#[test]
fn a_call_at_the_bound_is_copied_and_one_byte_over_it_is_over_bound_with_both_sizes() {
    let request = r#"{"model":"code","messages":[]}"#;
    let total = request.len() + CHAT_ANSWER.len();
    let stream_text: usize = MESSAGES_STREAM.iter().map(|piece| piece.len()).sum();
    let payload_bytes: usize = payloads().iter().map(Vec::len).sum();
    let streamed = r#"{"model":"code","stream":true}"#;
    for (bound, copied) in [(total, true), (total - 1, false)] {
        let rig = Rig::new(
            vec![json(CHAT_ANSWER)],
            Options {
                call_bytes: Some(bound),
                ..Options::default()
            },
        );
        rig.call(CHAT, request, "");
        let bytes = rig.finished(1)[0].1.clone().unwrap();
        if copied {
            assert!(matches!(bytes, CallBytes::Copied { .. }), "{bytes:?}");
        } else {
            assert_eq!(
                bytes,
                CallBytes::OverBound {
                    request_bytes: request.len() as u64,
                    answer_bytes: CHAT_ANSWER.len() as u64,
                }
            );
        }
    }
    // A stream counts its whole text, not only its payloads.
    let rig = Rig::new(
        vec![events(&MESSAGES_STREAM)],
        Options {
            call_bytes: Some(streamed.len() + payload_bytes),
            ..Options::default()
        },
    );
    rig.call(MESSAGES, streamed, "");
    assert_eq!(
        rig.finished(1)[0].1.clone().unwrap(),
        CallBytes::OverBound {
            request_bytes: streamed.len() as u64,
            answer_bytes: stream_text as u64,
        }
    );
}

// --- The three series --------------------------------------------------------------------------

#[test]
fn the_three_interaction_record_series_start_at_zero_and_count_what_the_embedding_counts() {
    let rig = Rig::new(Vec::new(), Options::default());
    let before = rig.scrape();
    assert!(
        before.contains("\nllmgw_interaction_records_dropped_total 0\n"),
        "{before}"
    );
    assert!(
        before.contains("\nllmgw_interaction_records_skipped_total 0\n"),
        "{before}"
    );
    assert!(
        before.contains("\nllmgw_interaction_records_failed_total 0\n"),
        "{before}"
    );
    rig.metrics.count_interaction_record_dropped();
    rig.metrics.count_interaction_record_dropped();
    rig.metrics.count_interaction_record_skipped();
    for _ in 0..3 {
        rig.metrics.count_interaction_record_failed();
    }
    let after = rig.scrape();
    assert!(
        after.contains("\nllmgw_interaction_records_dropped_total 2\n"),
        "{after}"
    );
    assert!(
        after.contains("\nllmgw_interaction_records_skipped_total 1\n"),
        "{after}"
    );
    assert!(
        after.contains("\nllmgw_interaction_records_failed_total 3\n"),
        "{after}"
    );
    let order = |text: &str, name: &str| text.find(&format!("# HELP {name} ")).unwrap();
    assert!(
        order(&after, "llmgw_pod_start_spend_refusals_total")
            < order(&after, "llmgw_interaction_records_dropped_total")
            && order(&after, "llmgw_interaction_records_dropped_total")
                < order(&after, "llmgw_interaction_records_skipped_total")
            && order(&after, "llmgw_interaction_records_skipped_total")
                < order(&after, "llmgw_interaction_records_failed_total")
            && order(&after, "llmgw_interaction_records_failed_total")
                < order(&after, "llmgw_route_requests_total"),
        "{after}"
    );
}
