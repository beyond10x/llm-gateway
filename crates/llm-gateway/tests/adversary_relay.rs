//! Adversarial pass over `story:wire-relay`: the body reader, the framing, the relay and the
//! refusal shapes, attacked over a real socket against loopback fixture pods only.
//!
//! Every case here drives the public API (`Gateway::bind_with_relay`) and asserts what the
//! published contract (`docs/gateway.md`, `spec/domains/gateway.yaml`, the capability matrix
//! rows R6-R8, W1-W5, W7, K11) or HTTP/1.1 (RFC 9110, RFC 9112) requires. The cases run one at
//! a time under a file-local lock, because one of them measures the process's resident memory.

use llm_gateway::{
    Gateway, GatewayConfig, GatewayHandle, Label, OwnerToken, Relay, RelayModel, RelayStream,
    RelayTarget, RelayTargets, RouteInventory, SharedSecretVerifier, TargetRefusal, Wire,
};
use std::{
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex, MutexGuard},
    thread::{self, JoinHandle},
    time::Duration,
};

const OWNER: &str = "owner-token-adversary-relay-0123456789abcdef";
const WAIT: Duration = Duration::from_secs(20);
/// `request-head-bytes`, the published default (`docs/gateway.md`).
const HEAD_BOUND: usize = 8192;

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn label(value: &str) -> Label {
    Label::new(value).unwrap()
}

fn code_model() -> RelayModel {
    RelayModel::new(
        label("code"),
        label("served-code"),
        vec![Wire::Chat, Wire::Responses, Wire::Messages],
    )
    .unwrap()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// --- A loopback pod that records raw requests and answers raw bytes ---------------------------

struct Pod {
    authority: String,
    seen: Arc<Mutex<Vec<Vec<u8>>>>,
    thread: Option<JoinHandle<()>>,
}

impl Pod {
    /// Answers each accepted connection with the next raw answer, written verbatim, then closes.
    fn start(answers: Vec<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let authority = listener.local_addr().unwrap().to_string();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recording = Arc::clone(&seen);
        let thread = thread::spawn(move || {
            for answer in answers {
                let Ok((mut connection, _)) = listener.accept() else {
                    return;
                };
                connection.set_read_timeout(Some(WAIT)).unwrap();
                connection.set_write_timeout(Some(WAIT)).unwrap();
                let Some(raw) = read_request(&mut connection) else {
                    // The unblocking connection from `Drop`, or a gateway that sent nothing.
                    return;
                };
                recording.lock().unwrap().push(raw);
                drop(connection.write_all(&answer));
                drop(connection.flush());
                drop(connection.shutdown(Shutdown::Both));
            }
        });
        Self {
            authority,
            seen,
            thread: Some(thread),
        }
    }

    fn seen(&self) -> Vec<Vec<u8>> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for Pod {
    fn drop(&mut self) {
        drop(TcpStream::connect(&self.authority));
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

/// Reads one request head and its `content-length` body, raw.
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

fn json_answer(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

// --- The target source ------------------------------------------------------------------------

struct Source {
    authority: String,
    acquired: Mutex<Vec<String>>,
    invalidated: Mutex<Vec<(String, String)>>,
}

impl Source {
    fn of(pod: &Pod) -> Arc<Self> {
        Arc::new(Self {
            authority: pod.authority.clone(),
            acquired: Mutex::new(Vec::new()),
            invalidated: Mutex::new(Vec::new()),
        })
    }

    fn acquired(&self) -> usize {
        self.acquired.lock().unwrap().len()
    }

    fn invalidated(&self) -> Vec<(String, String)> {
        self.invalidated.lock().unwrap().clone()
    }
}

impl RelayTargets for Source {
    fn acquire(&self, alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        self.acquired.lock().unwrap().push(alias.to_string());
        Ok(Box::new(Target {
            authority: self.authority.clone(),
        }))
    }

    fn invalidate(&self, alias: &str, authority: &str) {
        self.invalidated
            .lock()
            .unwrap()
            .push((alias.to_string(), authority.to_string()));
    }
}

struct Target {
    authority: String,
}

impl RelayTarget for Target {
    fn authority(&self) -> &str {
        &self.authority
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        let stream = TcpStream::connect(&self.authority)?;
        stream.set_read_timeout(Some(WAIT))?;
        stream.set_write_timeout(Some(WAIT))?;
        Ok(Box::new(stream))
    }
}

// --- The gateway --------------------------------------------------------------------------------

struct Relayed {
    handle: Option<GatewayHandle>,
    addr: SocketAddr,
}

impl Relayed {
    fn start(source: &Arc<Source>, read_timeout: Duration) -> Self {
        let verifier = Arc::new(
            SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
        );
        let targets: Arc<dyn RelayTargets> = Arc::clone(source) as Arc<dyn RelayTargets>;
        let relay = Relay::new(vec![code_model()], targets).unwrap();
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
        let addr = handle.local_addr();
        Self {
            handle: Some(handle),
            addr,
        }
    }
}

impl Drop for Relayed {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.shutdown();
        }
    }
}

/// The answer as raw bytes, split at its head.
#[derive(Debug)]
struct Raw {
    status: u16,
    head: String,
    rest: Vec<u8>,
}

impl Raw {
    fn header(&self, name: &str) -> Option<String> {
        self.head.split("\r\n").skip(1).find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case(name)
                .then(|| value.trim().to_string())
        })
    }

    fn header_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .head
            .split("\r\n")
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .map(|(key, _)| key.trim().to_ascii_lowercase())
            .collect();
        names.sort();
        names
    }

    fn code(&self) -> Option<String> {
        let text = String::from_utf8_lossy(&self.rest);
        let marker = "\"code\":\"";
        let start = text.find(marker)? + marker.len();
        let end = text[start..].find('"')?;
        Some(text[start..start + end].to_string())
    }

    fn refused(&self) -> (u16, Option<String>) {
        (self.status, self.code())
    }
}

fn exchange(addr: SocketAddr, raw: &[u8]) -> Raw {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream.set_write_timeout(Some(WAIT)).unwrap();
    drop(stream.write_all(raw));
    drop(stream.flush());
    let mut received = Vec::new();
    drop(stream.read_to_end(&mut received));
    split(&received)
}

/// Splits at the final head; an interim `1xx` head before it is skipped.
fn split(mut received: &[u8]) -> Raw {
    let (head, status, end) = loop {
        let end = find(received, b"\r\n\r\n").expect("the gateway wrote no head");
        let head = String::from_utf8(received[..end].to_vec()).unwrap();
        let status: u16 = head.split(' ').nth(1).unwrap().parse().unwrap();
        if status >= 200 {
            break (head, status, end);
        }
        received = &received[end + 4..];
    };
    Raw {
        status,
        head,
        rest: received[end + 4..].to_vec(),
    }
}

/// Decodes a chunked body, and whether its last chunk arrived.
fn dechunk(mut rest: &[u8]) -> (Vec<u8>, bool) {
    let mut body = Vec::new();
    loop {
        let Some(line_end) = find(rest, b"\r\n") else {
            return (body, false);
        };
        let Ok(size) = usize::from_str_radix(String::from_utf8_lossy(&rest[..line_end]).trim(), 16)
        else {
            return (body, false);
        };
        rest = &rest[line_end + 2..];
        if size == 0 {
            return (body, true);
        }
        let available = size.min(rest.len());
        body.extend_from_slice(&rest[..available]);
        if available < size || rest.len() < size + 2 {
            return (body, false);
        }
        rest = &rest[size + 2..];
    }
}

fn post(path: &str, extra: &str, body: &[u8]) -> Vec<u8> {
    let mut raw = format!(
        "POST {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\n{extra}content-length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);
    raw
}

fn chunked_head(path: &str) -> Vec<u8> {
    format!(
        "POST {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\ntransfer-encoding: chunked\r\n\r\n"
    )
    .into_bytes()
}

fn resident_kib() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|value| value.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap()
}

// --- Red: the trailer section is not bounded ----------------------------------------------------

/// W1 bounds the decoded body and the head is bounded at `request-head-bytes`, but the trailer
/// section of a chunked body is read line by line with no total bound (`relay.rs:323`). A trailer
/// section larger than the whole permitted head is accepted and the request relayed.
#[test]
fn adv_w1_a_trailer_section_larger_than_the_head_bound_is_refused() {
    let _serial = serial();
    let pod = Pod::start(vec![json_answer("{}")]);
    let source = Source::of(&pod);
    let gateway = Relayed::start(&source, WAIT);
    let body = b"{\"model\":\"code\"}";
    let mut raw = chunked_head("/v1/chat/completions");
    raw.extend_from_slice(format!("{:x}\r\n", body.len()).as_bytes());
    raw.extend_from_slice(body);
    raw.extend_from_slice(b"\r\n0\r\n");
    // Five trailer fields of 4008 bytes each: past twice the whole head bound, each line under it.
    let trailers_from = raw.len();
    for index in 0..5 {
        raw.extend_from_slice(format!("x-t{index}: ").as_bytes());
        raw.extend_from_slice(&[b'a'; 4000]);
        raw.extend_from_slice(b"\r\n");
    }
    raw.extend_from_slice(b"\r\n");
    let trailer_bytes = raw.len() - trailers_from;
    assert!(trailer_bytes > 2 * HEAD_BOUND);

    let answer = exchange(gateway.addr, &raw);
    assert_ne!(
        answer.status, 200,
        "a {trailer_bytes}-byte trailer section past request-head-bytes ({HEAD_BOUND}) was read and the request relayed"
    );
    assert_eq!(source.acquired(), 0, "the oversized request woke a target");
}

/// The relay's line reader (`Buffered::fill`, `relay.rs:234-253`) only empties its buffer when
/// every buffered byte has been consumed. While it reads trailer lines, a read that ends inside
/// a line appends to the buffer without dropping what was already consumed, so a client that
/// never lets a read end on a line boundary has every trailer byte it sends kept in memory until
/// the request ends. Measured as the process's resident memory while the trailer section is open.
#[test]
fn adv_w1_trailer_bytes_already_read_are_not_kept_in_memory() {
    // One trailer line is 8100 bytes with its CRLF (under the 8192-byte line limit); writes are
    // 12007 bytes, coprime with 8100, so no write ends on a line boundary for 8100 writes.
    const LINE: usize = 8100;
    const WRITE: usize = 12_007;
    const SENT: usize = 90 * 1024 * 1024;
    let _serial = serial();
    let pod = Pod::start(vec![json_answer("{}")]);
    let source = Source::of(&pod);
    let gateway = Relayed::start(&source, Duration::from_secs(120));

    let pattern: Vec<u8> = {
        let mut line = b"x-t: ".to_vec();
        line.resize(LINE - 2, b'a');
        line.extend_from_slice(b"\r\n");
        line
    };

    let mut client = TcpStream::connect(gateway.addr).unwrap();
    client.set_write_timeout(Some(WAIT)).unwrap();
    let body = b"{\"model\":\"code\"}";
    let mut prefix = chunked_head("/v1/chat/completions");
    prefix.extend_from_slice(format!("{:x}\r\n", body.len()).as_bytes());
    prefix.extend_from_slice(body);
    prefix.extend_from_slice(b"\r\n0\r\n");
    client.write_all(&prefix).unwrap();
    thread::sleep(Duration::from_millis(200));
    let before = resident_kib();

    let mut offset = 0_usize;
    let mut segment = vec![0_u8; WRITE];
    while offset < SENT {
        for (index, byte) in segment.iter_mut().enumerate() {
            *byte = pattern[(offset + index) % LINE];
        }
        if client.write_all(&segment).is_err() {
            break;
        }
        offset += WRITE;
        // Paced so that each gateway read returns one write, ending inside a line.
        thread::sleep(Duration::from_micros(150));
    }
    // Finish the line in progress but not the trailer section, so the request stays open.
    let in_line = offset % LINE;
    if in_line != 0 {
        drop(client.write_all(&pattern[in_line..]));
    }
    thread::sleep(Duration::from_millis(500));
    let held_kib = resident_kib().saturating_sub(before);

    drop(client.write_all(b"\r\n"));
    drop(client.shutdown(Shutdown::Write));
    let mut sink = Vec::new();
    client.set_read_timeout(Some(WAIT)).unwrap();
    drop(client.read_to_end(&mut sink));

    // Nothing the relay needs after a trailer line is read is more than one line and one read.
    assert!(
        held_kib < 32 * 1024,
        "after {} MiB of trailer lines the process held {} MiB more than before they arrived",
        offset / (1024 * 1024),
        held_kib / 1024
    );
}

// --- Red: chunk-size leniency -------------------------------------------------------------------

/// RFC 9112 section 7.1: `chunk-size = 1*HEXDIG`. `chunk_size` (`relay.rs:291-297`) trims the
/// line and hands it to `u64::from_str_radix`, which takes a leading `+`, so `+10` and ` 10` are
/// read as sixteen. A front end that reads the size strictly disagrees about where the body
/// ends.
#[test]
fn adv_w1_a_chunk_size_that_is_not_hex_digits_is_request_malformed() {
    let _serial = serial();
    let pod = Pod::start(vec![json_answer("{}"), json_answer("{}")]);
    let source = Source::of(&pod);
    let gateway = Relayed::start(&source, WAIT);
    let body = b"{\"model\":\"code\"}";
    assert_eq!(body.len(), 0x10);
    for size_line in ["+10", " 10"] {
        let mut raw = chunked_head("/v1/messages");
        raw.extend_from_slice(size_line.as_bytes());
        raw.extend_from_slice(b"\r\n");
        raw.extend_from_slice(body);
        raw.extend_from_slice(b"\r\n0\r\n\r\n");
        let answer = exchange(gateway.addr, &raw);
        assert_eq!(
            answer.refused(),
            (400, Some("request-malformed".to_string())),
            "chunk-size line {size_line:?} was accepted"
        );
    }
    assert_eq!(source.acquired(), 0);
}

// --- Red: chunked framing sent to an HTTP/1.0 client --------------------------------------------

/// RFC 9112 section 6.1: a server MUST NOT send a response containing `transfer-encoding` unless
/// the request indicates HTTP/1.1 or later. The head parser admits `HTTP/1.0`
/// (`server.rs:408`), and `stream_answer` (`relay.rs:564-566`) frames every relayed answer as
/// chunked regardless, so an HTTP/1.0 client reads the chunk-size lines as body bytes.
#[test]
fn adv_w5_an_http_1_0_client_is_not_sent_a_chunked_answer() {
    let _serial = serial();
    let pod = Pod::start(vec![json_answer("{\"id\":\"one-oh\"}")]);
    let source = Source::of(&pod);
    let gateway = Relayed::start(&source, WAIT);
    let body = b"{\"model\":\"code\"}";
    let mut raw = format!(
        "POST /v1/chat/completions HTTP/1.0\r\nauthorization: Bearer {OWNER}\r\ncontent-length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);
    let answer = exchange(gateway.addr, &raw);
    assert_eq!(answer.status, 200, "{answer:?}");
    assert_eq!(
        answer.header("transfer-encoding"),
        None,
        "an HTTP/1.0 client was sent a chunked answer"
    );
    assert_eq!(answer.rest, b"{\"id\":\"one-oh\"}");
}

// --- Red: a bare LF inside a header value -------------------------------------------------------

/// RFC 9110 section 5.5: a field value containing CR, LF or NUL is invalid, and a recipient MUST
/// reject it or replace each such octet with SP. `Request::parse` splits on CRLF only
/// (`server.rs:400`), so `x-note: a<LF>transfer-encoding: chunked` is one `x-note` field to the
/// gateway and a `transfer-encoding` field to any intermediary that accepts a bare LF as a line
/// end. Before this unit no body was framed, so the field never mattered; now it frames one.
#[test]
fn adv_w1_a_header_value_holding_a_bare_lf_is_request_malformed() {
    let _serial = serial();
    let pod = Pod::start(vec![json_answer("{}")]);
    let source = Source::of(&pod);
    let gateway = Relayed::start(&source, WAIT);
    let raw = post(
        "/v1/chat/completions",
        "x-note: a\ntransfer-encoding: chunked\r\n",
        b"{\"model\":\"code\"}",
    );
    let answer = exchange(gateway.addr, &raw);
    assert_eq!(
        answer.refused(),
        (400, Some("request-malformed".to_string())),
        "a header value holding a bare LF framed a relayed body"
    );
    assert_eq!(source.acquired(), 0);
}

// --- Pins: what the relay already gets right and no test held ---------------------------------

/// The owner credential and every other client header stay at the gateway: the target sees
/// exactly `host`, `content-type`, `content-length` and `connection`.
#[test]
fn adv_r6_the_owner_credential_and_client_headers_never_reach_the_target() {
    let _serial = serial();
    let pod = Pod::start(vec![json_answer("{}")]);
    let source = Source::of(&pod);
    let gateway = Relayed::start(&source, WAIT);
    let raw = post(
        "/v1/messages?beta=true",
        "x-api-key: client-key\r\ncookie: s=1\r\nx-forwarded-host: evil\r\nanthropic-version: 2023-06-01\r\nexpect: 100-continue\r\n",
        b"{\"model\":\"code\"}",
    );
    let answer = exchange(gateway.addr, &raw);
    assert_eq!(answer.status, 200, "{answer:?}");
    let seen = pod.seen();
    assert_eq!(seen.len(), 1);
    let request = &seen[0];
    assert!(
        find(request, OWNER.as_bytes()).is_none(),
        "the owner credential reached the target"
    );
    let end = find(request, b"\r\n\r\n").unwrap();
    let head = String::from_utf8(request[..end].to_vec()).unwrap();
    let mut lines = head.split("\r\n");
    assert_eq!(lines.next(), Some("POST /v1/messages HTTP/1.1"));
    let mut names: Vec<String> = lines
        .map(|line| line.split_once(':').unwrap().0.to_ascii_lowercase())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["connection", "content-length", "content-type", "host"]
    );
    assert_eq!(&request[end + 4..], b"{\"model\":\"served-code\"}");
}

/// Hop-by-hop and every other target header stay at the target; the client sees the target's
/// content type, `cache-control: no-store`, `connection: close` and the gateway's own framing.
#[test]
fn adv_w5_target_headers_other_than_content_type_never_reach_the_client() {
    let _serial = serial();
    let answer = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nSet-Cookie: s=1\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nWWW-Authenticate: Basic\r\nUpgrade: h2c\r\nProxy-Authenticate: Basic\r\nTrailer: x-t\r\nCache-Control: max-age=600\r\nContent-Length: 2\r\n\r\n{}".to_vec();
    let pod = Pod::start(vec![answer]);
    let source = Source::of(&pod);
    let gateway = Relayed::start(&source, WAIT);
    let relayed = exchange(
        gateway.addr,
        &post("/v1/responses", "", b"{\"model\":\"code\"}"),
    );
    assert_eq!(relayed.status, 200);
    assert_eq!(
        relayed.header_names(),
        [
            "cache-control",
            "connection",
            "content-type",
            "transfer-encoding"
        ]
    );
    assert_eq!(relayed.header("connection").as_deref(), Some("close"));
    assert_eq!(relayed.header("cache-control").as_deref(), Some("no-store"));
    assert_eq!(dechunk(&relayed.rest), (b"{}".to_vec(), true));
}

/// A stream the target cuts mid-event reaches the client without its last chunk, so the client
/// sees it cut; the endpoint is kept, because its head had already been relayed.
#[test]
fn adv_w5_a_stream_cut_mid_event_reaches_the_client_cut_and_keeps_the_endpoint() {
    let _serial = serial();
    let first = "data: {\"n\":1}\n\n";
    let chunked = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{first}\r\n20\r\ndata: {{\"n\":",
        first.len()
    )
    .into_bytes();
    let length = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100\r\n\r\n{\"partial\":".to_vec();
    let pod = Pod::start(vec![chunked, length]);
    let source = Source::of(&pod);
    let gateway = Relayed::start(&source, WAIT);
    for path in ["/v1/chat/completions", "/v1/messages"] {
        let relayed = exchange(gateway.addr, &post(path, "", b"{\"model\":\"code\"}"));
        assert_eq!(relayed.status, 200);
        let (_, complete) = dechunk(&relayed.rest);
        assert!(
            !complete,
            "{path}: a cut answer reached the client as complete"
        );
    }
    assert_eq!(source.invalidated(), Vec::<(String, String)>::new());
}

/// Smuggling-shaped framings are refused from the head, before a target is asked for.
#[test]
fn adv_w1_conflicting_or_unknown_framings_are_refused_before_a_target_is_asked() {
    let _serial = serial();
    let pod = Pod::start(Vec::new());
    let source = Source::of(&pod);
    let gateway = Relayed::start(&source, WAIT);
    let body = "{\"model\":\"code\"}";
    let heads = [
        format!(
            "content-length: {}\r\ntransfer-encoding: chunked\r\n",
            body.len()
        ),
        format!(
            "transfer-encoding: chunked\r\ncontent-length: {}\r\n",
            body.len()
        ),
        "transfer-encoding: chunked, chunked\r\n".to_string(),
        "transfer-encoding: identity\r\n".to_string(),
        "transfer-encoding: xchunked\r\n".to_string(),
        format!("content-length: {0}\r\nContent-Length: {0}\r\n", body.len()),
        format!("content-length: {0}, {0}\r\n", body.len()),
        "transfer-encoding: chunked\r\nTransfer-Encoding: chunked\r\n".to_string(),
    ];
    for framing in &heads {
        let raw = format!(
            "POST /v1/chat/completions HTTP/1.1\r\nauthorization: Bearer {OWNER}\r\n{framing}\r\n{body}"
        );
        let answer = exchange(gateway.addr, raw.as_bytes());
        assert_eq!(
            answer.refused(),
            (400, Some("request-malformed".to_string())),
            "{framing:?}"
        );
    }
    // A chunk announcing more than the bound in one size line.
    for (size_line, expected) in [
        ("2000001", (413, "body-too-large")),
        ("ffffffffffffffff", (413, "body-too-large")),
    ] {
        let mut raw = chunked_head("/v1/responses");
        raw.extend_from_slice(format!("{size_line}\r\n").as_bytes());
        let answer = exchange(gateway.addr, &raw);
        assert_eq!(
            answer.refused(),
            (expected.0, Some(expected.1.to_string())),
            "{size_line}"
        );
    }
    assert_eq!(source.acquired(), 0);
}

/// JSON edge cases the scanner must take or refuse exactly as RFC 8259 does, and the rewrite
/// must change only the recorded values.
#[test]
fn adv_w2_w3_w4_json_edge_cases_are_read_as_rfc_8259_and_rewritten_in_place() {
    let _serial = serial();
    let deep = format!(
        "{{\"model\":\"code\",\"x\":{}{}}}",
        "[".repeat(200_000),
        "]".repeat(200_000)
    );
    let relayed: Vec<(&str, Vec<u8>, Vec<u8>)> = vec![
        (
            "/v1/chat/completions",
            b"{\"\\u006dodel\":\"code\"}".to_vec(),
            b"{\"\\u006dodel\":\"served-code\"}".to_vec(),
        ),
        (
            "/v1/chat/completions",
            b"\t\r\n {\"model\" :\t\"code\" , \"x\" :[ ] }\r\n".to_vec(),
            b"\t\r\n {\"model\" :\t\"served-code\" , \"x\" :[ ] }\r\n".to_vec(),
        ),
        (
            "/v1/responses",
            b"{\"model\":\"code\",\"x\":1E400,\"y\":-0.0e-0,\"z\":\"\\ud83d\\ude00\"}".to_vec(),
            b"{\"model\":\"served-code\",\"x\":1E400,\"y\":-0.0e-0,\"z\":\"\\ud83d\\ude00\"}".to_vec(),
        ),
        (
            "/v1/messages",
            b"{\"model\":\"code\",\"output\\u005fconfig\":{\"effort\":\"high\"}}".to_vec(),
            b"{\"model\":\"served-code\",\"output\\u005fconfig\":{\"effort\":\"xhigh\"}}".to_vec(),
        ),
        (
            "/v1/messages",
            b"{\"model\":\"code\",\"output_config\":[{\"effort\":\"high\"}],\"chat_template_kwargs\":{\"effort\":\"high\"},\"x\":{\"output_config\":{\"effort\":\"high\"}}}".to_vec(),
            b"{\"model\":\"served-code\",\"output_config\":[{\"effort\":\"high\"}],\"chat_template_kwargs\":{\"effort\":\"high\"},\"x\":{\"output_config\":{\"effort\":\"high\"}}}".to_vec(),
        ),
        (
            "/v1/messages",
            b"{\"output_config\":{\"effort\":\"high\",\"effort\":\"low\"},\"model\":\"code\"}".to_vec(),
            b"{\"output_config\":{\"effort\":\"xhigh\",\"effort\":\"low\"},\"model\":\"served-code\"}".to_vec(),
        ),
        ("/v1/chat/completions", deep.clone().into_bytes(), deep.replacen("\"code\"", "\"served-code\"", 1).into_bytes()),
    ];
    let refused: Vec<(&[u8], &str)> = vec![
        (b"\xef\xbb\xbf{\"model\":\"code\"}", "body-not-json"),
        (
            b"{\"model\":\"code\",\"\\u006dodel\":\"other\"}",
            "body-not-json",
        ),
        (b"{\"model\":\"code\"}\x00", "body-not-json"),
        (b"{\"model\":\"code\"}\x0c", "body-not-json"),
        (
            b"{\"model\":\"code\",\"x\":\"\\udc00\\ud800\"}",
            "body-not-json",
        ),
        (b"{\"model\":\"code\",\"x\":\"\xc0\xaf\"}", "body-not-json"),
        (
            b"{\"model\":\"code\",\"x\":\"\xed\xa0\x80\"}",
            "body-not-json",
        ),
        (b"{1:2,\"model\":\"code\"}", "body-not-json"),
        (b"{true:2,\"model\":\"code\"}", "body-not-json"),
        (b"{\"model\":\"code\",\"x\":+1}", "body-not-json"),
        (b"{\"model\":\"code\",\"x\":.5}", "body-not-json"),
        (b"{\"model\":\"code\",\"x\":0x10}", "body-not-json"),
        (b"{\"model\":\"code\",\"x\":NaN}", "body-not-json"),
        (b"{\"model\":\"code\",\"x\":\"\\u00g0\"}", "body-not-json"),
        (b"{\"model\":\"code\"}{\"model\":\"code\"}", "body-not-json"),
        (b"{\"model\":{\"model\":\"code\"}}", "model-absent"),
        (b"{\"model\":\"code\\u0000\"}", "model-unknown"),
    ];
    let pod = Pod::start(relayed.iter().map(|_| json_answer("{}")).collect());
    let source = Source::of(&pod);
    let gateway = Relayed::start(&source, WAIT);
    for (body, code) in &refused {
        let answer = exchange(gateway.addr, &post("/v1/chat/completions", "", body));
        assert_eq!(
            answer.code().as_deref(),
            Some(*code),
            "{}",
            String::from_utf8_lossy(body)
        );
    }
    assert_eq!(source.acquired(), 0);
    for (path, sent, _) in &relayed {
        let answer = exchange(gateway.addr, &post(path, "", sent));
        assert_eq!(answer.status, 200, "{path}: {:?}", answer.code());
    }
    let seen = pod.seen();
    assert_eq!(seen.len(), relayed.len());
    for (request, (path, _, expected)) in seen.iter().zip(&relayed) {
        let end = find(request, b"\r\n\r\n").unwrap();
        assert!(
            request[end + 4..] == expected[..],
            "{path}: {}",
            String::from_utf8_lossy(&request[end + 4..request.len().min(end + 200)])
        );
    }
}
