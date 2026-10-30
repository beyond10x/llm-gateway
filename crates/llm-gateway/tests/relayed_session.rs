//! story:relayed-session-header at the library: the caller a target is asked about
//! (`RelayTarget::caller_header` with a `Caller`) and the header it answers, written by the relay
//! in place of a static header of the same name, on the relayed request and on the output clamp's
//! count alike (`docs/gateway.md` "The upstream" and step 7).
//!
//! The target is in-process: each connection it opens records what the gateway writes and
//! answers one canned `200`. Nothing leaves the process but the client's loopback connection.

use llm_gateway::{
    Caller, Gateway, GatewayConfig, Label, OwnerToken, Relay, RelayModel, RelayStream, RelayTarget,
    RelayTargets, RouteInventory, SharedSecretVerifier, StaticHeader, TargetRefusal, Wire,
};
use std::{
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, TcpStream},
    sync::{Arc, Mutex},
    time::Duration,
};

const OWNER: &str = "owner-token-relayed-session-0123456789abcdef";
const ANSWER: &[u8] =
    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}";
const SESSION: &str = "x-opencode-session";
const STATIC_SESSION: &str = "static-fallback-session";

/// One connection: records every byte written into its own entry, answers [`ANSWER`].
struct Recording {
    written: Arc<Mutex<Vec<Vec<u8>>>>,
    index: usize,
    answer: io::Cursor<&'static [u8]>,
}

impl Read for Recording {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.answer.read(buffer)
    }
}

impl Write for Recording {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.written.lock().unwrap()[self.index].extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl RelayStream for Recording {}

/// What the target answers `caller_header` with.
#[derive(Clone, Copy)]
enum Answer {
    /// `x-opencode-session` with the caller's session, else `peer-<address>`, else nothing.
    Session,
    /// Nothing, whoever asks.
    Nothing,
}

/// What the source and its targets saw.
#[derive(Default)]
struct Seen {
    /// Every connection's bytes, in the order they were opened.
    written: Arc<Mutex<Vec<Vec<u8>>>>,
    /// Every caller a target was asked about, as `(session, peer)`.
    callers: Mutex<Vec<(Option<String>, Option<IpAddr>)>>,
}

struct Target {
    headers: Vec<StaticHeader>,
    answer: Answer,
    window: Option<u64>,
    seen: Arc<Seen>,
}

impl RelayTarget for Target {
    fn authority(&self) -> &'static str {
        "session.invalid:8000"
    }

    fn headers(&self) -> &[StaticHeader] {
        &self.headers
    }

    fn max_model_len(&self) -> Option<u64> {
        self.window
    }

    fn caller_header(&self, caller: &Caller<'_>) -> Option<StaticHeader> {
        self.seen
            .callers
            .lock()
            .unwrap()
            .push((caller.session().map(str::to_owned), caller.peer()));
        match self.answer {
            Answer::Nothing => None,
            Answer::Session => {
                let value = caller
                    .session()
                    .map(str::to_owned)
                    .or_else(|| caller.peer().map(|peer| format!("peer-{peer}")))?;
                StaticHeader::new(SESSION, &value).ok()
            }
        }
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        let mut written = self.seen.written.lock().unwrap();
        written.push(Vec::new());
        Ok(Box::new(Recording {
            written: Arc::clone(&self.seen.written),
            index: written.len() - 1,
            answer: io::Cursor::new(ANSWER),
        }))
    }
}

struct Source {
    answer: Answer,
    window: Option<u64>,
    seen: Arc<Seen>,
}

impl RelayTargets for Source {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Ok(Box::new(Target {
            headers: vec![
                StaticHeader::new("x-title", "b10x gateway").unwrap(),
                StaticHeader::new(SESSION, STATIC_SESSION).unwrap(),
            ],
            answer: self.answer,
            window: self.window,
            seen: Arc::clone(&self.seen),
        }))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

/// Relays one request per `extra` header block to a target answering `answer`, on the messages
/// wire with `max_tokens` so that a target with a `window` is counted first, and returns what
/// the source saw.
fn relay(answer: Answer, window: Option<u64>, extras: &[&str]) -> Arc<Seen> {
    let seen = Arc::new(Seen::default());
    let targets: Arc<dyn RelayTargets> = Arc::new(Source {
        answer,
        window,
        seen: Arc::clone(&seen),
    });
    let model = RelayModel::new(
        Label::new("go").unwrap(),
        Label::new("served").unwrap(),
        vec![Wire::Messages],
    )
    .unwrap();
    let relay = Relay::new(vec![model], targets).unwrap();
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    );
    let handle = Gateway::bind_with_relay(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()),
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    handle.mark_ready();
    let body = r#"{"model":"go","max_tokens":64,"messages":[]}"#;
    for extra in extras {
        let mut stream = TcpStream::connect(handle.local_addr()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        stream
            .write_all(
                format!(
                    "POST /v1/messages HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER}\r\n{extra}content-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .unwrap();
        let mut answered = Vec::new();
        drop(stream.read_to_end(&mut answered));
        let answered = String::from_utf8_lossy(&answered).into_owned();
        assert!(answered.starts_with("HTTP/1.1 200 "), "{answered}");
    }
    handle.shutdown();
    seen
}

/// Every `name` line of the head of `request`, as values.
fn values(request: &[u8], name: &str) -> Vec<String> {
    let text = String::from_utf8_lossy(request).into_owned();
    let head = text.split("\r\n\r\n").next().unwrap_or_default().to_owned();
    head.split("\r\n")
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .filter(|(key, _)| key.trim().eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim().to_owned())
        .collect()
}

/// A target is asked about each request's caller: the client's `x-opencode-session` when it is
/// 1 to 256 bytes of printable ASCII without a space, never otherwise, and the peer the
/// connection came from.
#[test]
fn session_the_caller_a_target_is_asked_about_has_the_clients_valid_session_and_its_peer() {
    let longest = "s".repeat(256);
    let longer = "t".repeat(257);
    let header = |value: &str| format!("X-OpenCode-Session: {value}\r\n");
    let extras = [
        header("caller-session-0123"),
        header(&longest),
        header("has a space"),
        header("control\u{1}byte"),
        header("delete\u{7f}byte"),
        header("ünicode"),
        header(&longer),
        String::new(),
    ];
    let extras: Vec<&str> = extras.iter().map(String::as_str).collect();
    let seen = relay(Answer::Nothing, None, &extras);
    let callers = seen.callers.lock().unwrap().clone();
    let local = Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let expected: Vec<(Option<String>, Option<IpAddr>)> = [
        Some("caller-session-0123".to_owned()),
        Some(longest),
        None,
        None,
        None,
        None,
        None,
        None,
    ]
    .into_iter()
    .map(|session| (session, local))
    .collect();
    assert_eq!(callers, expected);
}

/// The header a target answers for the caller replaces its static header of the same name, so
/// the target is sent exactly one, on the relayed request and on the output clamp's count; the
/// other static headers stay. A target answering nothing keeps its static one.
#[test]
fn session_a_callers_header_replaces_the_static_header_of_its_name_on_the_request_and_the_count() {
    let seen = relay(
        Answer::Session,
        Some(8192),
        &["x-opencode-session: caller-session-0123\r\n", ""],
    );
    let written = seen.written.lock().unwrap().clone();
    assert_eq!(written.len(), 4, "a count and a request per call");
    let peer = format!("peer-{}", Ipv4Addr::LOCALHOST);
    for (request, expected) in written.iter().zip([
        "caller-session-0123",
        "caller-session-0123",
        peer.as_str(),
        peer.as_str(),
    ]) {
        assert_eq!(values(request, SESSION), [expected], "{request:?}");
        assert_eq!(values(request, "x-title"), ["b10x gateway"]);
    }
    assert!(String::from_utf8_lossy(&written[0]).starts_with("POST /v1/messages/count_tokens "));
    assert!(String::from_utf8_lossy(&written[1]).starts_with("POST /v1/messages "));

    let seen = relay(
        Answer::Nothing,
        None,
        &["x-opencode-session: caller-session-0123\r\n"],
    );
    let written = seen.written.lock().unwrap().clone();
    assert_eq!(written.len(), 1);
    assert_eq!(values(&written[0], SESSION), [STATIC_SESSION]);
    assert!(!String::from_utf8_lossy(&written[0]).contains("caller-session-0123"));
}

/// A caller keeps a session of 1 to 256 printable ASCII bytes without a space and drops any
/// other; an IPv4 peer reached over an IPv6 socket is its IPv4 address.
#[test]
fn session_a_caller_keeps_only_a_valid_session_and_its_canonical_peer() {
    for (value, kept) in [
        ("a", true),
        ("~!\"#", true),
        ("", false),
        (" leading", false),
        ("tab\there", false),
        ("nul\0", false),
    ] {
        assert_eq!(
            Caller::new(Some(value), None).session().is_some(),
            kept,
            "{value:?}"
        );
    }
    assert_eq!(
        Caller::new(Some(&"x".repeat(256)), None)
            .session()
            .map(str::len),
        Some(256)
    );
    assert_eq!(Caller::new(Some(&"x".repeat(257)), None).session(), None);
    let mapped: IpAddr = "::ffff:10.1.2.3".parse().unwrap();
    assert_eq!(
        Caller::new(None, Some(mapped)).peer(),
        Some("10.1.2.3".parse().unwrap())
    );
    assert_eq!(Caller::default(), Caller::new(None, None));
}

// --- Adversary, wave 2026-10-30-w02 -------------------------------------------------------------

/// Adversary: a session is "1 to 256 bytes of printable ASCII without a space; any other value is
/// ignored, never sent" (`docs/gateway.md` "The `OpenCode` Go session", step 1). The head keeps a
/// field value after `str::trim`, which strips every Unicode `White_Space` character, not only the
/// SP and HTAB that RFC 9110 allows around a field value. So a value with a vertical tab, a form
/// feed, a NEL or a no-break space at either end, all of them bytes the rule refuses, reaches the
/// target as the printable remainder, and a 257-byte value whose last byte is a form feed is told
/// to the target as 256 bytes. Each such value must be dropped, as a control byte or a non-ASCII
/// byte inside the value is.
#[test]
fn adv_session_a_value_with_a_whitespace_control_or_non_ascii_byte_at_its_edge_is_dropped() {
    let longest = "s".repeat(256);
    let values = [
        "caller-session-0123\u{b}".to_owned(),
        "\u{c}caller-session-0123".to_owned(),
        "caller-session-0123\u{85}".to_owned(),
        "\u{a0}caller-session-0123".to_owned(),
        "caller-session-0123\u{3000}".to_owned(),
        format!("{longest}\u{c}"),
    ];
    let extras: Vec<String> = values
        .iter()
        .map(|value| format!("x-opencode-session: {value}\r\n"))
        .collect();
    let extras: Vec<&str> = extras.iter().map(String::as_str).collect();
    let seen = relay(Answer::Nothing, None, &extras);
    let callers = seen.callers.lock().unwrap().clone();
    let sessions: Vec<(String, Option<String>)> = values
        .iter()
        .map(|value| format!("{value:?}"))
        .zip(callers.into_iter().map(|(session, _)| session))
        .collect();
    let expected: Vec<(String, Option<String>)> = values
        .iter()
        .map(|value| (format!("{value:?}"), None))
        .collect();
    assert_eq!(
        sessions, expected,
        "a malformed client session was trimmed into one the target is told"
    );
}

/// Every caller a translator was handed, as `(session, peer)`.
type Handed = Arc<Mutex<Vec<(Option<String>, Option<IpAddr>)>>>;

/// A translator that records the caller of every request it is handed and refuses it.
struct RecordingTranslator {
    callers: Handed,
}

impl llm_gateway::Translator for RecordingTranslator {
    fn prepare(
        &self,
        request: &llm_gateway::TranslationRequest<'_>,
    ) -> Result<Box<dyn llm_gateway::Translation>, llm_gateway::RefusalCode> {
        self.callers.lock().unwrap().push((
            request.caller.session().map(str::to_owned),
            request.caller.peer(),
        ));
        Err(llm_gateway::RefusalCode::TranslationInvalid)
    }
}

/// Adversary: the translated path is "the same order" as the relayed one only if the relay hands
/// the translator the caller it hands a target (`TranslationRequest::caller`). No test drives a
/// request through the relay into a translator and reads that caller back, so a relay that hands
/// every translator `Caller::default()` stays green while every translated `OpenCode` Go turn
/// falls back to llm's per-request session. This case reads it back: the client's valid session
/// and its peer, and the peer alone when it sends none.
#[test]
fn adv_session_a_translated_request_hands_the_translator_the_callers_session_and_peer() {
    let callers: Handed = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::new(Seen::default());
    let targets: Arc<dyn RelayTargets> = Arc::new(Source {
        answer: Answer::Nothing,
        window: None,
        seen: Arc::clone(&seen),
    });
    let model = RelayModel::new(
        Label::new("go").unwrap(),
        Label::new("served").unwrap(),
        vec![Wire::Messages],
    )
    .unwrap();
    let relay = Relay::new(vec![model], targets)
        .unwrap()
        .with_translator(Arc::new(RecordingTranslator {
            callers: Arc::clone(&callers),
        }));
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    );
    let handle = Gateway::bind_with_relay(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()),
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    handle.mark_ready();
    let body = r#"{"model":"go","messages":[{"role":"user","content":"hi"}]}"#;
    for extra in ["x-opencode-session: caller-session-0123\r\n", ""] {
        let mut stream = TcpStream::connect(handle.local_addr()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        stream
            .write_all(
                format!(
                    "POST /v1/chat/completions HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER}\r\n{extra}content-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .unwrap();
        let mut answered = Vec::new();
        drop(stream.read_to_end(&mut answered));
    }
    handle.shutdown();
    let local = Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert_eq!(
        callers.lock().unwrap().clone(),
        [
            (Some("caller-session-0123".to_owned()), local),
            (None, local)
        ]
    );
    assert!(seen.written.lock().unwrap().is_empty());
}
