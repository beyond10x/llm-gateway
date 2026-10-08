//! The rules `spec/domains/gateway.yaml` declares that no conformance scenario can observe, one
//! test each, named `row_<n>_…` after the row of `docs/verification/spec-hardening-gateway-review.md`
//! it closes (story:gateway-spec-declarations).
//!
//! Two kinds of case live here. `row_4_*` and `row_25_*` read the specification itself and hold
//! it to the crate: every refusal the crate can emit is declared with its code, status and
//! message, and every `GatewayConfig` default is declared with its value. The rest drive the
//! public API over loopback sockets, against fixture targets in this file, for the rules the
//! specification carries as `ESS-LIMIT:` notes: a deadline, a stop, a dropped handle. Nothing
//! leaves the machine.

use llm_gateway::{
    Gateway, GatewayConfig, GatewayHandle, Label, OwnerToken, RefusalCode, Relay, RelayModel,
    RelayStream, RelayTarget, RelayTargets, RouteInventory, SharedSecretVerifier, TargetRefusal,
    Wire,
};
use std::{
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const SPEC: &str = include_str!("../../../spec/domains/gateway.yaml");
const AUTH: &str = include_str!("../src/auth.rs");
const RELAY: &str = include_str!("../src/relay.rs");
const OWNER: &str = "owner-token-spec-declarations-0123456789ab";
/// How long any side of a case waits for the other before it gives up.
const WAIT: Duration = Duration::from_secs(20);

// --- The specification -------------------------------------------------------------------------

/// The declaration of `name` in the specification: from its `- name:` line to the next
/// declaration or comment at the same indentation.
fn declaration(name: &str) -> &'static str {
    let start = SPEC
        .find(&format!("  - name: {name}\n"))
        .unwrap_or_else(|| panic!("spec/domains/gateway.yaml declares no {name}"));
    let rest = &SPEC[start + 1..];
    let end = [rest.find("\n  - name: "), rest.find("\n  #")]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn row_4_every_refusal_the_crate_emits_is_declared_with_its_code_status_and_message() {
    let refusal = declaration("llm-gateway.gateway.Refusal");
    let declared: Vec<&str> = refusal
        .lines()
        .filter(|line| line.trim_start().starts_with("- {name: "))
        .collect();
    let missing: Vec<String> = RefusalCode::ALL
        .iter()
        .map(|code| {
            format!(
                "- {{name: {code:?}, attributes: {{code: {}, status: {}, message: \"{}\"}}}}",
                code.wire(),
                code.status(),
                code.reason()
            )
        })
        .filter(|expected| !declared.iter().any(|line| line.trim() == expected))
        .collect();
    assert!(
        missing.is_empty(),
        "llm-gateway.gateway.Refusal does not declare these variants as the crate emits them: \
         {missing:#?}"
    );
    assert_eq!(
        declared.len(),
        RefusalCode::ALL.len(),
        "llm-gateway.gateway.Refusal declares a variant the crate cannot emit: {declared:#?}"
    );
}

/// The comment block directly above `field` in the `Gateway` entity.
fn field_comment(field: &str) -> String {
    let gateway = declaration("llm-gateway.gateway.Gateway");
    let lines: Vec<&str> = gateway.lines().collect();
    let at = lines
        .iter()
        .position(|line| {
            line.trim_start()
                .starts_with(&format!("- {{name: {field},"))
        })
        .unwrap_or_else(|| panic!("the Gateway entity has no field {field}"));
    lines[..at]
        .iter()
        .rev()
        .take_while(|line| line.trim_start().starts_with('#'))
        .map(|line| line.trim_start().trim_start_matches('#').trim())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn row_25_every_gateway_config_default_is_declared_with_its_value() {
    let defaults = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
    let read_timeout_ms = u64::try_from(defaults.read_timeout.as_millis()).unwrap();
    for (field, value) in [
        ("max_head_bytes", defaults.max_head_bytes.to_string()),
        (
            "max_concurrent_requests",
            defaults.max_concurrent_requests.to_string(),
        ),
        ("read_timeout_ms", read_timeout_ms.to_string()),
    ] {
        let comment = field_comment(field);
        let declared = format!("`GatewayConfig::new` defaults it to {value}");
        assert!(
            comment.contains(&declared),
            "Gateway.{field} must declare {declared:?}; its comment reads {comment:?}"
        );
    }
}

/// The body of `impl Drop for <owner>` in `source`, up to the next item.
fn drop_impl<'a>(source: &'a str, owner: &str) -> &'a str {
    let start = source
        .find(&format!("impl Drop for {owner} {{"))
        .unwrap_or_else(|| panic!("{owner} has no Drop impl"));
    let rest = &source[start..];
    &rest[..rest.find("\n}\n").unwrap_or(rest.len())]
}

/// `OwnerToken` and `TargetBearer` overwrite their bytes when dropped. A freed allocation cannot
/// be read without `unsafe`, which the crate forbids, so this reads the source: each type's
/// `Drop` writes zeros over its bytes and passes them to `black_box`, so the writes are kept.
#[test]
fn row_12_owner_material_and_a_target_bearer_overwrite_their_bytes_on_drop() {
    for (source, owner) in [(AUTH, "OwnerToken"), (RELAY, "TargetBearer")] {
        let body = drop_impl(source, owner);
        assert!(body.contains("self.0.fill(0);"), "{owner}: {body}");
        assert!(
            body.contains("std::hint::black_box(&self.0);"),
            "{owner}: {body}"
        );
    }
    let debug = format!("{:?}", OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap());
    assert!(!debug.contains(OWNER), "{debug}");
}

// --- Fixture targets -----------------------------------------------------------------------------

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Reads one request head and its `content-length` body.
fn read_request(connection: &mut TcpStream) -> Option<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 16_384];
    let end = loop {
        if let Some(end) = find(&buffer, b"\r\n\r\n") {
            break end;
        }
        let read = connection.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..end]).to_ascii_lowercase();
    let wanted: usize = head
        .split("\r\n")
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    while buffer.len() < end + 4 + wanted {
        let read = connection.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    Some(buffer)
}

/// A loopback pod that runs `answer` on the first connection it accepts, after reading its
/// request.
struct Pod {
    authority: String,
    thread: Option<JoinHandle<()>>,
}

impl Pod {
    fn start(answer: impl FnOnce(&mut TcpStream) + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let authority = listener.local_addr().unwrap().to_string();
        let thread = thread::spawn(move || {
            let Ok((mut connection, _)) = listener.accept() else {
                return;
            };
            connection.set_read_timeout(Some(WAIT)).unwrap();
            connection.set_write_timeout(Some(WAIT)).unwrap();
            if read_request(&mut connection).is_some() {
                answer(&mut connection);
            }
            drop(connection.shutdown(Shutdown::Both));
        });
        Self {
            authority,
            thread: Some(thread),
        }
    }
}

impl Drop for Pod {
    fn drop(&mut self) {
        // A pod nobody connected to is still blocked in `accept`.
        drop(TcpStream::connect(&self.authority));
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

/// How the fixture target connects.
#[derive(Clone)]
enum Connect {
    /// To a loopback pod at this authority.
    Pod(String),
    /// To a stream whose every write fails.
    Unwritable,
}

/// A connection that accepts nothing written to it.
struct Unwritable;

impl Read for Unwritable {
    fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }
}

impl Write for Unwritable {
    fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "the target is gone",
        ))
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "the target is gone",
        ))
    }
}

struct Source {
    connect: Connect,
    acquired: Mutex<usize>,
    invalidated: Mutex<Vec<(String, String)>>,
}

impl Source {
    fn new(connect: Connect) -> Arc<Self> {
        Arc::new(Self {
            connect,
            acquired: Mutex::new(0),
            invalidated: Mutex::new(Vec::new()),
        })
    }

    fn acquired(&self) -> usize {
        *self.acquired.lock().unwrap()
    }

    fn invalidated(&self) -> Vec<(String, String)> {
        self.invalidated.lock().unwrap().clone()
    }
}

impl RelayTargets for Source {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        *self.acquired.lock().unwrap() += 1;
        Ok(Box::new(Target(self.connect.clone())))
    }

    fn invalidate(&self, alias: &str, authority: &str) {
        self.invalidated
            .lock()
            .unwrap()
            .push((alias.to_string(), authority.to_string()));
    }
}

struct Target(Connect);

impl RelayTarget for Target {
    fn authority(&self) -> &str {
        match &self.0 {
            Connect::Pod(authority) => authority,
            Connect::Unwritable => "unwritable.invalid:80",
        }
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        match &self.0 {
            Connect::Pod(authority) => {
                let stream = TcpStream::connect(authority)?;
                stream.set_read_timeout(Some(WAIT))?;
                stream.set_write_timeout(Some(WAIT))?;
                Ok(Box::new(stream))
            }
            Connect::Unwritable => Ok(Box::new(Unwritable)),
        }
    }
}

fn relayed(source: &Arc<Source>, read_timeout: Duration) -> GatewayHandle {
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    );
    let model = RelayModel::new(
        Label::new("code").unwrap(),
        Label::new("served-code").unwrap(),
        vec![Wire::Chat, Wire::Responses, Wire::Messages],
    )
    .unwrap();
    let targets: Arc<dyn RelayTargets> = Arc::clone(source) as Arc<dyn RelayTargets>;
    let relay = Relay::new(vec![model], targets).unwrap();
    let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
    config.read_timeout = read_timeout;
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

fn head(version: &str, length: usize, extra: &str) -> Vec<u8> {
    format!(
        "POST /v1/chat/completions {version}\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\n{extra}content-length: {length}\r\n\r\n"
    )
    .into_bytes()
}

fn connect(addr: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream.set_write_timeout(Some(WAIT)).unwrap();
    stream
}

/// Reads until `needle` has arrived, or the peer closes.
fn read_until(stream: &mut TcpStream, received: &mut Vec<u8>, needle: &[u8]) -> bool {
    let mut chunk = [0_u8; 4096];
    while find(received, needle).is_none() {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return false,
            Ok(read) => received.extend_from_slice(&chunk[..read]),
        }
    }
    true
}

fn read_all(stream: &mut TcpStream) -> Vec<u8> {
    let mut received = Vec::new();
    // A reset after the answer is not a lost answer: keep what arrived.
    drop(stream.read_to_end(&mut received));
    received
}

/// The status and the refusal code of the final answer in `received`.
fn final_answer(received: &[u8]) -> (u16, String) {
    let text = String::from_utf8_lossy(received);
    let mut rest = text.as_ref();
    loop {
        let (head, after) = rest
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("no complete answer head in {text:?}"));
        let status: u16 = head.split(' ').nth(1).unwrap().parse().unwrap();
        if status >= 200 {
            let code = after
                .split_once("\"code\":\"")
                .and_then(|(_, code)| code.split_once('"'))
                .map_or_else(String::new, |(code, _)| code.to_string());
            return (status, code);
        }
        rest = after;
    }
}

// --- The relay -------------------------------------------------------------------------------

/// One `read_timeout` deadline covers the whole body: a body that keeps arriving a byte at a
/// time, each byte well inside the timeout, is still `body-incomplete` once the deadline passes.
#[test]
fn row_14_a_body_trickled_past_one_read_timeout_is_body_incomplete() {
    let source = Source::new(Connect::Unwritable);
    let handle = relayed(&source, Duration::from_millis(300));
    let mut stream = connect(handle.local_addr());
    let length = 40;
    stream.write_all(&head("HTTP/1.1", length, "")).unwrap();
    let mut writer = stream.try_clone().unwrap();
    let trickle = thread::spawn(move || {
        for _ in 0..length {
            thread::sleep(Duration::from_millis(50));
            if writer.write_all(b"x").is_err() {
                return;
            }
        }
    });
    let started = Instant::now();
    let received = read_all(&mut stream);
    let took = started.elapsed();
    drop(trickle.join());
    assert_eq!(
        final_answer(&received),
        (400, "body-incomplete".to_string()),
        "{}",
        String::from_utf8_lossy(&received)
    );
    assert!(
        took < Duration::from_millis(50 * 40),
        "the refusal waited for the whole trickled body: {took:?}"
    );
    assert_eq!(source.acquired(), 0);
}

/// `expect: 100-continue` is answered with an interim `100 Continue` for HTTP/1.1, before the
/// body is sent; an HTTP/1.0 client is not sent one.
#[test]
fn row_15_expect_100_continue_is_answered_for_http_1_1_only() {
    let source = Source::new(Connect::Unwritable);
    let handle = relayed(&source, WAIT);
    let body = b"{\"x\":1}";

    let mut stream = connect(handle.local_addr());
    stream
        .write_all(&head("HTTP/1.1", body.len(), "expect: 100-continue\r\n"))
        .unwrap();
    let mut received = Vec::new();
    assert!(read_until(&mut stream, &mut received, b"\r\n\r\n"));
    assert!(
        received.starts_with(b"HTTP/1.1 100 Continue\r\n\r\n"),
        "{}",
        String::from_utf8_lossy(&received)
    );
    stream.write_all(body).unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
    received.extend(read_all(&mut stream));
    assert_eq!(final_answer(&received), (400, "model-absent".to_string()));

    let mut stream = connect(handle.local_addr());
    stream
        .write_all(&head("HTTP/1.0", body.len(), "expect: 100-continue\r\n"))
        .unwrap();
    thread::sleep(Duration::from_millis(200));
    stream.write_all(body).unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
    let received = read_all(&mut stream);
    assert!(
        received.starts_with(b"HTTP/1.1 400 "),
        "an HTTP/1.0 client was sent an interim answer: {}",
        String::from_utf8_lossy(&received)
    );
    assert_eq!(final_answer(&received), (400, "model-absent".to_string()));
}

/// A request the target cannot be written, and an answer head that is not HTTP, are each
/// `upstream-failed` and report the target to `invalidate` by its authority.
#[test]
fn row_17_an_unwritable_request_or_a_malformed_answer_head_is_upstream_failed() {
    let body = b"{\"model\":\"code\"}";
    let unwritable = Source::new(Connect::Unwritable);
    let handle = relayed(&unwritable, WAIT);
    let mut stream = connect(handle.local_addr());
    stream
        .write_all(&[head("HTTP/1.1", body.len(), "").as_slice(), body].concat())
        .unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
    assert_eq!(
        final_answer(&read_all(&mut stream)),
        (502, "upstream-failed".to_string())
    );
    assert_eq!(
        unwritable.invalidated(),
        [("code".to_string(), "unwritable.invalid:80".to_string())]
    );

    let pod = Pod::start(|connection| {
        drop(connection.write_all(b"this is not an answer\r\n\r\n"));
    });
    let malformed = Source::new(Connect::Pod(pod.authority.clone()));
    let handle = relayed(&malformed, WAIT);
    let mut stream = connect(handle.local_addr());
    stream
        .write_all(&[head("HTTP/1.1", body.len(), "").as_slice(), body].concat())
        .unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
    assert_eq!(
        final_answer(&read_all(&mut stream)),
        (502, "upstream-failed".to_string())
    );
    assert_eq!(
        malformed.invalidated(),
        [("code".to_string(), pod.authority.clone())]
    );
}

/// A refused relay is closed gracefully: what the client still sends is read and discarded, so
/// a client that reads only once it has sent its whole body still reads its refusal, instead of
/// losing it to the reset that closing over unread bytes sends.
#[test]
fn row_19_a_client_still_sending_a_refused_body_reads_its_refusal() {
    let source = Source::new(Connect::Unwritable);
    let handle = relayed(&source, Duration::from_secs(5));
    let mut stream = connect(handle.local_addr());
    // One byte over the 32 MiB bound: refused from the head, before a byte of body is read.
    let mut writer = stream.try_clone().unwrap();
    let sending = thread::spawn(move || {
        let mut sent = writer.write_all(&head("HTTP/1.1", 33_554_433, ""));
        let block = vec![b'x'; 64 * 1024];
        for _ in 0..64 {
            if sent.is_err() {
                break;
            }
            sent = writer.write_all(&block);
            thread::sleep(Duration::from_millis(5));
        }
        drop(writer.shutdown(Shutdown::Write));
        sent.is_ok()
    });
    let whole_body_sent = sending.join().unwrap();
    let received = read_all(&mut stream);
    assert!(
        whole_body_sent,
        "the gateway stopped reading before the client finished sending"
    );
    assert_eq!(
        final_answer(&received),
        (413, "body-too-large".to_string()),
        "{}",
        String::from_utf8_lossy(&received)
    );
    assert_eq!(source.acquired(), 0);
}

/// A chunked answer from a pod that releases its second chunk only when told to.
fn held_stream(release: Receiver<()>) -> Pod {
    Pod::start(move |connection| {
        let first = "data: 1\n\n";
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{first}\r\n",
            first.len()
        );
        if connection.write_all(head.as_bytes()).is_err() {
            return;
        }
        drop(connection.flush());
        let _released = release.recv_timeout(WAIT);
        let second = "data: 2\n\n";
        drop(
            connection.write_all(format!("{:x}\r\n{second}\r\n0\r\n\r\n", second.len()).as_bytes()),
        );
    })
}

/// A stop waits for a relayed stream to end: the stream is not bounded by `read_timeout`, and
/// the stop reports it completed only after its last byte reached the client.
#[test]
fn row_23_a_stop_waits_for_a_relayed_stream_to_end() {
    let (release, released) = mpsc::channel();
    let pod = held_stream(released);
    let source = Source::new(Connect::Pod(pod.authority.clone()));
    let handle = relayed(&source, Duration::from_millis(300));
    let mut stream = connect(handle.local_addr());
    let body = b"{\"model\":\"code\"}";
    stream
        .write_all(&[head("HTTP/1.1", body.len(), "").as_slice(), body].concat())
        .unwrap();
    let mut received = Vec::new();
    assert!(read_until(&mut stream, &mut received, b"data: 1\n\n"));

    let (done, stopped) = mpsc::channel();
    let stopping = thread::spawn(move || {
        let report = handle.shutdown();
        drop(done.send(()));
        report
    });
    // Three read timeouts: a stop bounded by `read_timeout` would have returned by now.
    assert!(
        stopped.recv_timeout(Duration::from_millis(900)).is_err(),
        "the stop returned while a relayed stream was still open"
    );
    release.send(()).unwrap();
    received.extend(read_all(&mut stream));
    let report = stopping.join().unwrap();
    assert_eq!((report.accepted, report.completed), (1, 1), "{report:?}");
    let text = String::from_utf8_lossy(&received);
    assert!(text.contains("data: 2\n\n"), "{text}");
    assert!(text.ends_with("0\r\n\r\n"), "the stream was cut: {text}");
}

/// Each chunk written to the client has its own `read_timeout` deadline, so a client that stops
/// reading an endless stream cannot hold the stop.
#[test]
fn row_23_a_client_that_stops_reading_a_stream_cannot_hold_the_stop() {
    let pod = Pod::start(|connection| {
        let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n";
        if connection.write_all(head.as_bytes()).is_err() {
            return;
        }
        let event = vec![b'e'; 64 * 1024];
        let chunk = [format!("{:x}\r\n", event.len()).as_bytes(), &event, b"\r\n"].concat();
        let until = Instant::now() + WAIT;
        while Instant::now() < until {
            if connection.write_all(&chunk).is_err() {
                return;
            }
        }
    });
    let source = Source::new(Connect::Pod(pod.authority.clone()));
    let handle = relayed(&source, Duration::from_millis(300));
    let mut stream = connect(handle.local_addr());
    let body = b"{\"model\":\"code\"}";
    stream
        .write_all(&[head("HTTP/1.1", body.len(), "").as_slice(), body].concat())
        .unwrap();
    let mut received = Vec::new();
    assert!(read_until(&mut stream, &mut received, b"\r\n\r\n"));
    // From here the client reads nothing, and keeps its connection open.
    let started = Instant::now();
    let report = handle.shutdown();
    let took = started.elapsed();
    assert_eq!((report.accepted, report.completed), (1, 1), "{report:?}");
    assert!(
        took < Duration::from_secs(10),
        "a client that stopped reading held the stop for {took:?}"
    );
    drop(stream);
}

// --- The handle --------------------------------------------------------------------------------

/// Dropping the handle without a shutdown still stops the listener; it reports nothing.
#[test]
fn row_24_dropping_the_handle_without_a_shutdown_stops_the_listener() {
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    );
    let handle = Gateway::bind(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()),
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
    )
    .unwrap();
    let addr = handle.local_addr();
    let mut probe = connect(addr);
    probe.write_all(b"GET /health HTTP/1.1\r\n\r\n").unwrap();
    probe.shutdown(Shutdown::Write).unwrap();
    assert!(read_all(&mut probe).starts_with(b"HTTP/1.1 200 "));
    drop(handle);
    assert!(
        TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_err(),
        "the listener still accepts after its handle was dropped"
    );
}
