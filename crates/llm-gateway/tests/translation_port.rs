//! The translation port (story:gateway-translation): what the relay hands a [`Translator`],
//! when, and what it does with the answer. The translator here is a fixture that speaks no
//! protocol: it records what it was asked and writes a scripted answer through the writer the
//! relay provides, so these cases hold the relay's half of the contract with no llm crate in the
//! build. The binary's translator, over llm 0.6.0, is held by the `translate-*` conformance
//! scenarios and `crates/llm-gateway-cli/tests/translation.rs`.

use llm_gateway::{
    AnswerFraming, Gateway, GatewayConfig, GatewayHandle, Label, OwnerToken, RefusalCode, Relay,
    RelayModel, RelayStream, RelayTarget, RelayTargets, RouteInventory, SharedSecretVerifier,
    TargetBearer, TargetRefusal, TargetScheme, ToolCalling, TranslatedAnswer, Translation,
    TranslationFailure, TranslationRequest, Translator, Wire,
};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

const OWNER: &str = "owner-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const WAIT: Duration = Duration::from_secs(10);

fn label(value: &str) -> Label {
    Label::new(value).unwrap()
}

/// What the fixture translator answers once a target is acquired.
#[derive(Clone)]
enum Script {
    /// A head, then each piece in order.
    Answer {
        status: u16,
        content_type: &'static str,
        framing: AnswerFraming,
        pieces: Vec<&'static str>,
    },
    Fail(TranslationFailure),
    /// Asks `gone` until it is true or `for_` has passed, records what it last said, and then
    /// answers `{}` when the client is still there.
    Watch {
        for_: Duration,
    },
}

/// What the translator was asked, in order.
#[derive(Default)]
struct Asked {
    prepared: Mutex<Vec<String>>,
    ran: Mutex<Vec<String>>,
    /// Per piece written, whether the client was still there.
    writes: Mutex<Vec<bool>>,
    /// What `gone` last said, for a watching translation.
    gone: Mutex<Option<bool>>,
}

struct Fixture {
    refuse: Option<RefusalCode>,
    script: Script,
    asked: Arc<Asked>,
}

impl Translator for Fixture {
    fn prepare(
        &self,
        request: &TranslationRequest<'_>,
    ) -> Result<Box<dyn Translation>, RefusalCode> {
        self.asked.prepared.lock().unwrap().push(format!(
            "{}->{} {} {}",
            request.ingress.label(),
            request.upstream.label(),
            request.model.alias().as_str(),
            String::from_utf8_lossy(request.body)
        ));
        if let Some(code) = self.refuse {
            return Err(code);
        }
        Ok(Box::new(Run {
            script: self.script.clone(),
            asked: Arc::clone(&self.asked),
        }))
    }
}

struct Run {
    script: Script,
    asked: Arc<Asked>,
}

impl Translation for Run {
    fn run(
        self: Box<Self>,
        target: &dyn RelayTarget,
        answer: &mut dyn TranslatedAnswer,
    ) -> Result<(), TranslationFailure> {
        let scheme = match target.scheme() {
            TargetScheme::Http => "http",
            TargetScheme::Https => "https",
        };
        let bearer = target
            .bearer()
            .map(|bearer| String::from_utf8_lossy(bearer.expose()).into_owned());
        self.asked.ran.lock().unwrap().push(format!(
            "{scheme}://{} {}",
            target.authority(),
            bearer.as_deref().unwrap_or("anonymous")
        ));
        match self.script {
            Script::Fail(failure) => Err(failure),
            Script::Watch { for_ } => {
                let deadline = Instant::now() + for_;
                let mut gone = answer.gone();
                while !gone && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                    gone = answer.gone();
                }
                *self.asked.gone.lock().unwrap() = Some(gone);
                if !gone && answer.head(200, "application/json", AnswerFraming::Length(2)) {
                    answer.body(b"{}");
                }
                Ok(())
            }
            Script::Answer {
                status,
                content_type,
                framing,
                pieces,
            } => {
                if !answer.head(status, content_type, framing) {
                    return Ok(());
                }
                for piece in pieces {
                    let there = answer.body(piece.as_bytes());
                    self.asked.writes.lock().unwrap().push(there);
                    if !there {
                        break;
                    }
                }
                Ok(())
            }
        }
    }
}

/// Hands out one target per acquisition, counting acquisitions, releases and invalidations.
#[derive(Default)]
struct Targets {
    acquired: AtomicUsize,
    released: Arc<AtomicUsize>,
    invalidated: Mutex<Vec<String>>,
    https: bool,
}

struct Target {
    released: Arc<AtomicUsize>,
    bearer: TargetBearer,
    https: bool,
}

impl RelayTargets for Targets {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        self.acquired.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Target {
            released: Arc::clone(&self.released),
            bearer: TargetBearer::new(b"pod-key-1".to_vec()).unwrap(),
            https: self.https,
        }))
    }

    fn invalidate(&self, alias: &str, authority: &str) {
        self.invalidated
            .lock()
            .unwrap()
            .push(format!("{alias} {authority}"));
    }
}

impl RelayTarget for Target {
    fn authority(&self) -> &'static str {
        "pod.example.test:8000"
    }

    fn bearer(&self) -> Option<&TargetBearer> {
        Some(&self.bearer)
    }

    fn scheme(&self) -> TargetScheme {
        if self.https {
            TargetScheme::Https
        } else {
            TargetScheme::Http
        }
    }

    /// The byte relay's connection: a fixture that answers every request with one JSON body.
    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        Ok(Box::new(Canned {
            answer: io::Cursor::new(
                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\n\r\n{\"byte\":1}\n"
                    .to_vec(),
            ),
        }))
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        self.released.fetch_add(1, Ordering::SeqCst);
    }
}

struct Canned {
    answer: io::Cursor<Vec<u8>>,
}

impl Read for Canned {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.answer.read(buffer)
    }
}

impl Write for Canned {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn model(alias: &str, wires: Vec<Wire>) -> RelayModel {
    RelayModel::new(label(alias), label("served"), wires)
        .unwrap()
        .with_tool_calling(ToolCalling::Absent)
}

struct Composed {
    handle: GatewayHandle,
    targets: Arc<Targets>,
    asked: Arc<Asked>,
}

fn compose(translator: Option<Fixture>, https: bool) -> Composed {
    let targets = Arc::new(Targets {
        https,
        ..Targets::default()
    });
    let asked = translator.as_ref().map_or_else(
        || Arc::new(Asked::default()),
        |fixture| Arc::clone(&fixture.asked),
    );
    let mut relay = Relay::new(
        vec![
            model("chat-only", vec![Wire::Chat]),
            model("two", vec![Wire::Messages, Wire::Responses]),
        ],
        Arc::clone(&targets) as Arc<dyn RelayTargets>,
    )
    .unwrap();
    if let Some(translator) = translator {
        relay = relay.with_translator(Arc::new(translator));
    }
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    );
    let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
    config.read_timeout = WAIT;
    let handle = Gateway::bind_with_relay(
        config,
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    handle.mark_ready();
    Composed {
        handle,
        targets,
        asked,
    }
}

fn fixture(refuse: Option<RefusalCode>, script: Script) -> Fixture {
    Fixture {
        refuse,
        script,
        asked: Arc::new(Asked::default()),
    }
}

fn sse(pieces: Vec<&'static str>) -> Script {
    Script::Answer {
        status: 200,
        content_type: "text/event-stream",
        framing: AnswerFraming::Stream,
        pieces,
    }
}

fn send(addr: SocketAddr, path: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut raw = Vec::new();
    drop(stream.read_to_end(&mut raw));
    String::from_utf8(raw).unwrap()
}

fn get(addr: SocketAddr, path: &str) -> String {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    write!(stream, "GET {path} HTTP/1.1\r\nhost: gateway\r\n\r\n").unwrap();
    let mut raw = Vec::new();
    drop(stream.read_to_end(&mut raw));
    String::from_utf8(raw).unwrap()
}

fn body_of(raw: &str) -> &str {
    raw.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

const CHAT: &str = "/v1/chat/completions";

/// A relay composed without a translator still refuses a wire the model does not declare, and
/// asks for no target (row K11).
#[test]
fn translate_without_a_translator_an_undeclared_wire_is_still_wire_not_served() {
    let composed = compose(None, false);
    let raw = send(
        composed.handle.local_addr(),
        "/v1/messages",
        "{\"model\":\"chat-only\"}",
    );
    assert!(raw.starts_with("HTTP/1.1 400 "), "{raw}");
    assert!(body_of(&raw).contains("\"wire-not-served\""), "{raw}");
    assert_eq!(composed.targets.acquired.load(Ordering::SeqCst), 0);
}

/// With a translator a request on a wire the model does not declare goes to it, with the
/// client's wire, the model's first declared wire, the model and the body exactly as received;
/// a refusal it returns is the client's answer, and no target is asked for.
#[test]
fn translate_a_refused_preparation_answers_its_code_and_wakes_no_target() {
    for code in [
        RefusalCode::TranslationUnsupported,
        RefusalCode::TranslationInvalid,
        RefusalCode::TranslationOpaqueState,
    ] {
        let composed = compose(Some(fixture(Some(code), sse(vec![]))), false);
        let raw = send(
            composed.handle.local_addr(),
            CHAT,
            "{\"model\":\"two\",\"n\":2}",
        );
        assert!(raw.starts_with("HTTP/1.1 400 "), "{raw}");
        assert_eq!(
            body_of(&raw),
            format!(
                "{{\"error\":{{\"code\":\"{}\",\"message\":\"{}\"}}}}",
                code.wire(),
                code.reason()
            )
        );
        assert_eq!(
            *composed.asked.prepared.lock().unwrap(),
            vec!["chat->messages two {\"model\":\"two\",\"n\":2}".to_owned()]
        );
        assert!(composed.asked.ran.lock().unwrap().is_empty());
        assert_eq!(composed.targets.acquired.load(Ordering::SeqCst), 0);
    }
}

/// A prepared translation runs against the acquired target, which carries its scheme and its
/// bearer, and writes the client's answer: the head once, a chunked event stream, then the
/// target is released.
#[test]
fn translate_a_prepared_translation_streams_its_answer_and_releases_the_target() {
    for https in [false, true] {
        let composed = compose(
            Some(fixture(None, sse(vec!["data: one\n\n", "data: two\n\n"]))),
            https,
        );
        let raw = send(
            composed.handle.local_addr(),
            "/v1/responses",
            "{\"model\":\"chat-only\"}",
        );
        let (head, body) = raw.split_once("\r\n\r\n").unwrap();
        assert_eq!(
            head,
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-store\r\nconnection: close\r\ntransfer-encoding: chunked"
        );
        assert_eq!(
            body,
            "b\r\ndata: one\n\n\r\nb\r\ndata: two\n\n\r\n0\r\n\r\n"
        );
        let scheme = if https { "https" } else { "http" };
        assert_eq!(
            *composed.asked.ran.lock().unwrap(),
            vec![format!("{scheme}://pod.example.test:8000 pod-key-1")]
        );
        assert_eq!(
            *composed.asked.prepared.lock().unwrap(),
            vec!["responses->chat chat-only {\"model\":\"chat-only\"}".to_owned()]
        );
        assert_eq!(composed.targets.acquired.load(Ordering::SeqCst), 1);
        assert_eq!(composed.targets.released.load(Ordering::SeqCst), 1);
        assert!(composed.targets.invalidated.lock().unwrap().is_empty());
    }
}

/// A non-streamed answer is written with the `content-length` it declares.
#[test]
fn translate_a_whole_answer_is_written_with_its_content_length() {
    let composed = compose(
        Some(fixture(
            None,
            Script::Answer {
                status: 200,
                content_type: "application/json",
                framing: AnswerFraming::Length(11),
                pieces: vec!["{\"a\":", "\"bcd\"}"],
            },
        )),
        false,
    );
    let raw = send(
        composed.handle.local_addr(),
        "/v1/messages",
        "{\"model\":\"chat-only\"}",
    );
    assert_eq!(
        raw,
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncache-control: no-store\r\nconnection: close\r\ncontent-length: 11\r\n\r\n{\"a\":\"bcd\"}"
    );
}

/// An upstream that failed before any output is reported to the target source by the
/// target's authority and answered `upstream-failed`; one that refused is `upstream-refused`
/// and the target is kept; an encoder refusal before output is `translation-unrepresentable`.
#[test]
fn translate_a_failure_before_output_is_answered_and_only_an_unreachable_target_is_reported() {
    for (failure, code, reported) in [
        (
            TranslationFailure::UpstreamFailed,
            RefusalCode::UpstreamFailed,
            true,
        ),
        (
            TranslationFailure::Refused(RefusalCode::UpstreamRefused),
            RefusalCode::UpstreamRefused,
            false,
        ),
        (
            TranslationFailure::Refused(RefusalCode::TranslationUnrepresentable),
            RefusalCode::TranslationUnrepresentable,
            false,
        ),
    ] {
        let composed = compose(Some(fixture(None, Script::Fail(failure))), false);
        let raw = send(composed.handle.local_addr(), CHAT, "{\"model\":\"two\"}");
        assert!(raw.starts_with("HTTP/1.1 502 "), "{raw}");
        assert!(
            body_of(&raw).contains(&format!("\"{}\"", code.wire())),
            "{raw}"
        );
        let invalidated = composed.targets.invalidated.lock().unwrap().clone();
        if reported {
            assert_eq!(invalidated, vec!["two pod.example.test:8000".to_owned()]);
        } else {
            assert!(invalidated.is_empty(), "{invalidated:?}");
        }
        assert_eq!(composed.targets.released.load(Ordering::SeqCst), 1);
    }
}

/// A request on a declared wire takes the byte relay even with a translator, which is never
/// asked; and `tools-not-served` is still decided before a translator is.
#[test]
fn translate_a_declared_wire_is_relayed_byte_for_byte_and_tools_are_refused_first() {
    let composed = compose(Some(fixture(None, sse(vec!["data: x\n\n"]))), false);
    let addr = composed.handle.local_addr();
    let raw = send(addr, CHAT, "{\"model\":\"chat-only\"}");
    assert!(raw.starts_with("HTTP/1.1 200 "), "{raw}");
    assert!(raw.contains("{\"byte\":1}"), "{raw}");
    let raw = send(addr, CHAT, "{\"model\":\"two\",\"tools\":[{}]}");
    assert!(body_of(&raw).contains("\"tools-not-served\""), "{raw}");
    assert!(composed.asked.prepared.lock().unwrap().is_empty());
    assert_eq!(composed.targets.acquired.load(Ordering::SeqCst), 1);
}

/// A client that has gone is reported to the translation by the writer: the write fails.
#[test]
fn translate_a_client_that_has_gone_is_reported_by_the_writer() {
    let pieces: Vec<&'static str> =
        std::iter::repeat_n("data: chunk-of-an-answer\n\n", 4096).collect();
    let composed = compose(Some(fixture(None, sse(pieces))), false);
    let mut stream = TcpStream::connect(composed.handle.local_addr()).unwrap();
    let body = "{\"model\":\"two\"}";
    write!(
        stream,
        "POST {CHAT} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut first = [0_u8; 64];
    stream.set_read_timeout(Some(WAIT)).unwrap();
    assert!(stream.read(&mut first).unwrap() > 0);
    drop(stream);
    composed.handle.shutdown();
    let writes = composed.asked.writes.lock().unwrap().clone();
    assert_eq!(
        writes.last(),
        Some(&false),
        "{} writes, none failed",
        writes.len()
    );
    assert!(writes.len() < 4096);
}

/// The listing names every wire with a translator, and the declared ones as `upstream_wires`;
/// without one it is what it was.
#[test]
fn translate_the_listing_names_every_wire_and_the_upstream_wires_only_with_a_translator() {
    let composed = compose(Some(fixture(None, sse(vec![]))), false);
    assert_eq!(
        body_of(&get(composed.handle.local_addr(), "/v1/models")),
        "{\"object\":\"list\",\"data\":[{\"id\":\"chat-only\",\"object\":\"model\",\"owned_by\":\"llm-gateway\",\"wires\":[\"chat\",\"responses\",\"messages\"],\"upstream_wires\":[\"chat\"]},{\"id\":\"two\",\"object\":\"model\",\"owned_by\":\"llm-gateway\",\"wires\":[\"chat\",\"responses\",\"messages\"],\"upstream_wires\":[\"messages\",\"responses\"]}]}"
    );
    let composed = compose(None, false);
    assert_eq!(
        body_of(&get(composed.handle.local_addr(), "/v1/models")),
        "{\"object\":\"list\",\"data\":[{\"id\":\"chat-only\",\"object\":\"model\",\"owned_by\":\"llm-gateway\",\"wires\":[\"chat\"]},{\"id\":\"two\",\"object\":\"model\",\"owned_by\":\"llm-gateway\",\"wires\":[\"messages\",\"responses\"]}]}"
    );
}

/// A target's bearer reads back as the material it was built from, and stays redacted in
/// `Debug`; a target that says nothing of its scheme is `http`.
#[test]
fn translate_a_target_exposes_its_bearer_and_defaults_to_http() {
    struct Plain;
    impl RelayTarget for Plain {
        fn authority(&self) -> &'static str {
            "a"
        }
        fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
            Err(io::ErrorKind::Unsupported.into())
        }
    }
    let bearer = TargetBearer::new(b"pod-key-1".to_vec()).unwrap();
    assert_eq!(bearer.expose(), b"pod-key-1");
    assert_eq!(format!("{bearer:?}"), "TargetBearer([REDACTED])");
    assert_eq!(Plain.scheme(), TargetScheme::Http);
}

/// A client that closed its connection while it waited for an answer is reported gone by the
/// writer without a byte written; one still there is not, and gets its answer.
#[test]
fn translate_the_writer_reports_a_client_that_closed_while_it_waited() {
    let composed = compose(Some(fixture(None, Script::Watch { for_: WAIT })), false);
    let body = "{\"model\":\"two\"}";
    let request = format!(
        "POST {CHAT} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(composed.handle.local_addr()).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    drop(stream);
    composed.handle.shutdown();
    assert_eq!(*composed.asked.gone.lock().unwrap(), Some(true));

    let composed = compose(
        Some(fixture(
            None,
            Script::Watch {
                for_: Duration::from_millis(200),
            },
        )),
        false,
    );
    let mut stream = TcpStream::connect(composed.handle.local_addr()).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut raw = Vec::new();
    drop(stream.read_to_end(&mut raw));
    assert!(
        String::from_utf8_lossy(&raw).ends_with("\r\n\r\n{}"),
        "{raw:?}"
    );
    assert_eq!(*composed.asked.gone.lock().unwrap(), Some(false));
}
