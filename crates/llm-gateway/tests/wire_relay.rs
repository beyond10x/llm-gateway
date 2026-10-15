//! The relay of the three model-call wires (story:wire-relay), one test per behaviour, each
//! named after the row of `docs/llmgw-capability-matrix.md` it closes: R6, R7, R8, W1, W2, W3,
//! W4, W5, W6, W7, K11 and B8, and tool calling (`tools_*`), and what Claude Code sends on the
//! messages wire (`claude_code_*`, story:claude-code-wire), a model's declared effort
//! (`effort_*`), the output clamp (`clamp_*`, story:max-tokens-clamp) and the usage block of an
//! answer (`usage_*`, story:usage-details-pass-through). The specification is
//! the `Relay` command of `spec/domains/gateway.yaml`.
//!
//! Every target is a loopback fixture pod in this file: a `TcpListener` on `127.0.0.1:0` that
//! answers a fixed script. The gateway never connects anywhere itself; it asks the injected
//! [`RelayTargets`] for a target and the target opens the connection. Nothing leaves the
//! machine, and no provider is involved.

use llm_gateway::{
    CredentialHeader, CredentialHeaderError, Gateway, GatewayConfig, GatewayHandle, Label,
    OwnerToken, ReasoningEffort, Relay, RelayError, RelayModel, RelayStream, RelayTarget,
    RelayTargets, RouteInventory, SharedSecretVerifier, TargetBearer, TargetRefusal, TokenError,
    ToolCalling, Wire,
};
use std::{
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const OWNER: &str = "owner-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// The body bound the relay publishes: 32 MiB, as llmgw `src/lib.rs:35`.
const BODY_BOUND: usize = 33_554_432;
/// How long any side of a test waits for the other before it gives up.
const WAIT: Duration = Duration::from_secs(10);
const UPSTREAM_FAILED: &str = "{\"error\":{\"code\":\"upstream-failed\",\"message\":\"the model target could not be reached or failed; the next request asks for a replacement\"}}";

fn label(value: &str) -> Label {
    Label::new(value).unwrap()
}

fn all_wires() -> Vec<Wire> {
    vec![Wire::Chat, Wire::Responses, Wire::Messages]
}

/// The model most tests relay: alias `code`, served upstream as `served-code`, on every wire.
fn code_model() -> RelayModel {
    RelayModel::new(label("code"), label("served-code"), all_wires()).unwrap()
}

// --- Fixture pods ----------------------------------------------------------------------------

/// How a scripted reply frames its body.
#[derive(Clone, Copy)]
enum Framing {
    /// `content-length`.
    Length,
    /// `transfer-encoding: chunked`, one HTTP chunk per scripted chunk.
    Chunked,
    /// Neither: the body ends when the pod closes the connection.
    Close,
}

/// One scripted answer of a fixture pod.
enum Answer {
    Reply {
        status: u16,
        content_type: &'static str,
        chunks: Vec<Vec<u8>>,
        framing: Framing,
        /// Waited on before the head is written.
        hold_head: Option<Receiver<()>>,
        /// Waited on before every chunk after the first.
        hold_chunks: Option<Receiver<()>>,
    },
    /// Reads the request, then closes without a byte of answer.
    Close,
}

fn reply(status: u16, content_type: &'static str, body: &[u8]) -> Answer {
    Answer::Reply {
        status,
        content_type,
        chunks: vec![body.to_vec()],
        framing: Framing::Length,
        hold_head: None,
        hold_chunks: None,
    }
}

fn ok_json(body: &str) -> Answer {
    reply(200, "application/json", body.as_bytes())
}

fn stream(chunks: &[&str]) -> Answer {
    Answer::Reply {
        status: 200,
        content_type: "text/event-stream",
        chunks: chunks
            .iter()
            .map(|chunk| chunk.as_bytes().to_vec())
            .collect(),
        framing: Framing::Chunked,
        hold_head: None,
        hold_chunks: None,
    }
}

/// One request exactly as a pod received it.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    host: Option<String>,
    content_length: Option<String>,
    transfer_encoding: Option<String>,
    authorization: Option<String>,
    body: Vec<u8>,
    /// Every byte the pod read for this request, head and body.
    raw: Vec<u8>,
}

impl Seen {
    fn body_text(&self) -> String {
        String::from_utf8(self.body.clone()).unwrap()
    }
}

/// A loopback pod that answers its script in order, one connection per answer, then stops
/// listening, so a connection past its script is refused.
struct Pod {
    authority: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Pod {
    fn start(script: Vec<Answer>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let authority = listener.local_addr().unwrap().to_string();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recording = Arc::clone(&seen);
        let stopping = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stopping);
        let thread = thread::spawn(move || {
            for answer in script {
                let Ok((mut connection, _)) = listener.accept() else {
                    return;
                };
                if stopped.load(Ordering::SeqCst) {
                    return;
                }
                connection.set_read_timeout(Some(WAIT)).unwrap();
                connection.set_write_timeout(Some(WAIT)).unwrap();
                if let Some(request) = read_request(&mut connection) {
                    recording.lock().unwrap().push(request);
                }
                answer_with(&mut connection, answer);
            }
        });
        Self {
            authority,
            seen,
            stopping,
            thread: Some(thread),
        }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for Pod {
    fn drop(&mut self) {
        // A pod whose script was not used up is still blocked in `accept`: mark it stopping,
        // then unblock it with one connection it will not answer.
        self.stopping.store(true, Ordering::SeqCst);
        drop(TcpStream::connect(&self.authority));
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn header_of(head: &str, name: &str) -> Option<String> {
    head.split("\r\n").skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().to_string())
    })
}

/// Reads one request head and its `content-length` body.
fn read_request(connection: &mut TcpStream) -> Option<Seen> {
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
    let head = String::from_utf8(buffer[..end].to_vec()).ok()?;
    let mut body = buffer[end + 4..].to_vec();
    let content_length = header_of(&head, "content-length");
    let wanted: usize = content_length
        .as_deref()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    while body.len() < wanted {
        let read = connection.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    let raw = [&buffer[..end + 4], body.as_slice()].concat();
    let mut start = head.split("\r\n").next()?.split(' ');
    Some(Seen {
        method: start.next()?.to_string(),
        path: start.next()?.to_string(),
        host: header_of(&head, "host"),
        content_length,
        transfer_encoding: header_of(&head, "transfer-encoding"),
        authorization: header_of(&head, "authorization"),
        body,
        raw,
    })
}

fn answer_with(connection: &mut TcpStream, answer: Answer) {
    let Answer::Reply {
        status,
        content_type,
        chunks,
        framing,
        hold_head,
        hold_chunks,
    } = answer
    else {
        drop(connection.shutdown(Shutdown::Both));
        return;
    };
    if let Some(hold) = hold_head {
        // A sender that is gone releases the hold as surely as one that sent.
        let _released = hold.recv_timeout(WAIT);
    }
    let length = match framing {
        Framing::Length => {
            let total: usize = chunks.iter().map(Vec::len).sum();
            format!("content-length: {total}\r\n")
        }
        Framing::Chunked => "transfer-encoding: chunked\r\n".to_string(),
        Framing::Close => String::new(),
    };
    let head = format!(
        "HTTP/1.1 {status} Fixture\r\ncontent-type: {content_type}\r\n{length}connection: close\r\n\r\n"
    );
    if connection.write_all(head.as_bytes()).is_err() {
        return;
    }
    for (index, chunk) in chunks.iter().enumerate() {
        if index > 0
            && let Some(hold) = &hold_chunks
        {
            let _released = hold.recv_timeout(WAIT);
        }
        let framed = match framing {
            Framing::Chunked => {
                let mut framed = format!("{:x}\r\n", chunk.len()).into_bytes();
                framed.extend_from_slice(chunk);
                framed.extend_from_slice(b"\r\n");
                framed
            }
            Framing::Length | Framing::Close => chunk.clone(),
        };
        if connection.write_all(&framed).is_err() {
            return;
        }
        drop(connection.flush());
    }
    if matches!(framing, Framing::Chunked) {
        drop(connection.write_all(b"0\r\n\r\n"));
    }
    drop(connection.shutdown(Shutdown::Both));
}

// --- The target source ------------------------------------------------------------------------

/// A pod as the source hands it out: listening, or refusing every connection.
#[derive(Clone)]
struct Slot {
    authority: String,
    refuses: bool,
    bearer: Option<Arc<TargetBearer>>,
    /// The header the bearer is sent in instead of `authorization` (an `api-key` endpoint).
    header: Option<CredentialHeader>,
    /// How many of the connections this slot's targets open, counted across them all, are
    /// refused before any is opened.
    refuse_first: Arc<AtomicUsize>,
}

/// The fake target source: hands out the current slot, and moves to the next one when the
/// gateway reports the current one failed. Its first `cold` acquisitions report the model still
/// starting past the hold budget (row W6), as a pool whose pod has not served yet does.
struct Source {
    slots: Vec<Slot>,
    current: Mutex<usize>,
    acquired: Mutex<Vec<String>>,
    invalidated: Mutex<Vec<(String, String)>>,
    released: Arc<AtomicUsize>,
    cold: AtomicUsize,
}

impl Source {
    fn new(slots: Vec<Slot>) -> Arc<Self> {
        Self::cold(slots, 0)
    }

    fn cold(slots: Vec<Slot>, cold: usize) -> Arc<Self> {
        Arc::new(Self {
            slots,
            current: Mutex::new(0),
            acquired: Mutex::new(Vec::new()),
            invalidated: Mutex::new(Vec::new()),
            released: Arc::new(AtomicUsize::new(0)),
            cold: AtomicUsize::new(cold),
        })
    }

    fn of(pods: &[&Pod]) -> Arc<Self> {
        Self::new(pods.iter().map(|pod| listening(pod)).collect())
    }

    fn acquired(&self) -> Vec<String> {
        self.acquired.lock().unwrap().clone()
    }

    fn invalidated(&self) -> Vec<(String, String)> {
        self.invalidated.lock().unwrap().clone()
    }

    fn released(&self) -> usize {
        self.released.load(Ordering::SeqCst)
    }

    fn make_current(&self, index: usize) {
        *self.current.lock().unwrap() = index;
    }

    /// Waits until `count` targets have been released, or the deadline passes.
    fn released_reaches(&self, count: usize) -> bool {
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            if self.released() >= count {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }
}

fn listening(pod: &Pod) -> Slot {
    Slot {
        authority: pod.authority.clone(),
        refuses: false,
        bearer: None,
        header: None,
        refuse_first: Arc::new(AtomicUsize::new(0)),
    }
}

fn refusing(name: &str) -> Slot {
    Slot {
        authority: format!("{name}.invalid:8000"),
        refuses: true,
        bearer: None,
        header: None,
        refuse_first: Arc::new(AtomicUsize::new(0)),
    }
}

impl RelayTargets for Source {
    fn acquire(&self, alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        self.acquired.lock().unwrap().push(alias.to_string());
        if self
            .cold
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(TargetRefusal::ColdStart);
        }
        let slot = self
            .slots
            .get(*self.current.lock().unwrap())
            .ok_or(TargetRefusal::Unavailable)?
            .clone();
        Ok(Box::new(Target {
            slot,
            released: Arc::clone(&self.released),
        }))
    }

    fn invalidate(&self, alias: &str, authority: &str) {
        self.invalidated
            .lock()
            .unwrap()
            .push((alias.to_string(), authority.to_string()));
        let mut current = self.current.lock().unwrap();
        // Only the current endpoint is dropped; a stale report changes nothing.
        if self
            .slots
            .get(*current)
            .is_some_and(|slot| slot.authority == authority)
        {
            *current += 1;
        }
    }
}

struct Target {
    slot: Slot,
    released: Arc<AtomicUsize>,
}

impl RelayTarget for Target {
    fn authority(&self) -> &str {
        &self.slot.authority
    }

    fn bearer(&self) -> Option<&TargetBearer> {
        self.slot.bearer.as_deref()
    }

    fn credential_header(&self) -> Option<&CredentialHeader> {
        self.slot.header.as_ref()
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        let refused_first = self
            .slot
            .refuse_first
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        if self.slot.refuses || refused_first {
            return Err(io::Error::from(io::ErrorKind::ConnectionRefused));
        }
        let stream = TcpStream::connect(&self.slot.authority)?;
        stream.set_read_timeout(Some(WAIT))?;
        stream.set_write_timeout(Some(WAIT))?;
        Ok(Box::new(stream))
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        self.released.fetch_add(1, Ordering::SeqCst);
    }
}

// --- The gateway --------------------------------------------------------------------------------

struct Relayed {
    handle: Option<GatewayHandle>,
    addr: SocketAddr,
}

impl Relayed {
    fn start(models: Vec<RelayModel>, source: &Arc<Source>) -> Self {
        Self::compose(models, source, true)
    }

    fn compose(models: Vec<RelayModel>, source: &Arc<Source>, ready: bool) -> Self {
        let verifier = Arc::new(
            SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
        );
        let targets: Arc<dyn RelayTargets> = Arc::clone(source) as Arc<dyn RelayTargets>;
        let relay = Relay::new(models, targets).unwrap();
        let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
        config.read_timeout = WAIT;
        let handle = Gateway::bind_with_relay(
            config,
            verifier,
            RouteInventory::new(Vec::new()).unwrap(),
            relay,
        )
        .unwrap();
        if ready {
            handle.mark_ready();
        }
        let addr = handle.local_addr();
        Self {
            handle: Some(handle),
            addr,
        }
    }

    fn post(&self, path: &str, body: &[u8]) -> Answered {
        send(self.addr, &request("POST", path, Some(OWNER), body))
    }

    fn post_text(&self, path: &str, body: &str) -> Answered {
        self.post(path, body.as_bytes())
    }
}

impl Drop for Relayed {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.shutdown();
        }
    }
}

// --- The client ---------------------------------------------------------------------------------

fn request(method: &str, path: &str, bearer: Option<&str>, body: &[u8]) -> Vec<u8> {
    let auth = bearer.map_or(String::new(), |token| {
        format!("authorization: Bearer {token}\r\n")
    });
    let mut raw = format!(
        "{method} {path} HTTP/1.1\r\nhost: gateway\r\n{auth}content-type: application/json\r\ncontent-length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);
    raw
}

#[derive(Debug)]
struct Answered {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Answered {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn text(&self) -> String {
        String::from_utf8(self.body.clone()).unwrap()
    }

    fn code(&self) -> Option<String> {
        let text = String::from_utf8_lossy(&self.body);
        let marker = "\"code\":\"";
        let start = text.find(marker)? + marker.len();
        let end = text[start..].find('"')?;
        Some(text[start..start + end].to_string())
    }

    fn refused(&self) -> (u16, Option<String>) {
        (self.status, self.code())
    }
}

/// Writes the whole request, then reads the whole answer. A write the gateway cut short is not
/// a lost answer: whatever arrived is read.
fn send(addr: SocketAddr, raw: &[u8]) -> Answered {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream.set_write_timeout(Some(WAIT)).unwrap();
    drop(stream.write_all(raw));
    drop(stream.flush());
    let mut received = Vec::new();
    drop(stream.read_to_end(&mut received));
    parse(&received).expect("the gateway wrote no complete answer")
}

/// Splits an answer at its head and decodes its body from whatever framing it carries.
fn parse(raw: &[u8]) -> Option<Answered> {
    let end = find(raw, b"\r\n\r\n")?;
    let head = String::from_utf8(raw[..end].to_vec()).ok()?;
    let status = head.split(' ').nth(1)?.parse().ok()?;
    let headers: Vec<(String, String)> = head
        .split("\r\n")
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let rest = &raw[end + 4..];
    let chunked = headers
        .iter()
        .any(|(key, value)| key == "transfer-encoding" && value.eq_ignore_ascii_case("chunked"));
    let body = if chunked {
        let (body, complete) = dechunk(rest);
        if !complete {
            return None;
        }
        body
    } else if let Some((_, length)) = headers.iter().find(|(key, _)| key == "content-length") {
        let length: usize = length.parse().ok()?;
        if rest.len() < length {
            return None;
        }
        rest[..length].to_vec()
    } else {
        rest.to_vec()
    };
    Some(Answered {
        status,
        headers,
        body,
    })
}

/// Decodes as much of a chunked body as has arrived, and whether its last chunk arrived.
fn dechunk(mut rest: &[u8]) -> (Vec<u8>, bool) {
    let mut body = Vec::new();
    loop {
        let Some(line_end) = find(rest, b"\r\n") else {
            return (body, false);
        };
        let size_text = String::from_utf8_lossy(&rest[..line_end]);
        let size_text = size_text.split(';').next().unwrap_or("").trim().to_string();
        let Ok(size) = usize::from_str_radix(&size_text, 16) else {
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

/// An answer read while it arrives, for the cases about streaming.
struct Reading {
    stream: TcpStream,
    raw: Vec<u8>,
}

impl Reading {
    fn open(addr: SocketAddr, path: &str, body: &str) -> Self {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        stream
            .write_all(&request("POST", path, Some(OWNER), body.as_bytes()))
            .unwrap();
        Self {
            stream,
            raw: Vec::new(),
        }
    }

    /// The body decoded so far.
    fn body_so_far(&self) -> Vec<u8> {
        let Some(end) = find(&self.raw, b"\r\n\r\n") else {
            return Vec::new();
        };
        let head = String::from_utf8_lossy(&self.raw[..end]).to_ascii_lowercase();
        let rest = &self.raw[end + 4..];
        if head.contains("transfer-encoding: chunked") {
            dechunk(rest).0
        } else {
            rest.to_vec()
        }
    }

    /// Reads until the decoded body ends with `expected`, or the deadline passes.
    fn until_body_is(&mut self, expected: &[u8]) -> bool {
        let deadline = Instant::now() + WAIT;
        let mut chunk = [0_u8; 4096];
        while Instant::now() < deadline {
            if self.body_so_far() == expected {
                return true;
            }
            match self.stream.read(&mut chunk) {
                Ok(0) => return self.body_so_far() == expected,
                Ok(read) => self.raw.extend_from_slice(&chunk[..read]),
                Err(_) => {}
            }
        }
        false
    }

    fn finish(mut self) -> Answered {
        self.stream.set_read_timeout(Some(WAIT)).unwrap();
        drop(self.stream.read_to_end(&mut self.raw));
        parse(&self.raw).expect("the gateway wrote no complete answer")
    }
}

fn pad_to(prefix: &str, total: usize) -> Vec<u8> {
    // `{"model":"code","pad":"aaa…"}`: the prefix, then the padding, then `"}`.
    let opening = format!("{prefix},\"pad\":\"");
    let mut body = opening.into_bytes();
    let fill = total - body.len() - 2;
    body.resize(body.len() + fill, b'a');
    body.extend_from_slice(b"\"}");
    assert_eq!(body.len(), total);
    body
}

// --- R6, R7, R8: the three wires ------------------------------------------------------------

/// Relays one plain answer and one event stream on `wire` and checks both reached the target's
/// same path with the model rewritten, and the client unchanged.
fn relays_on(path: &str, request_body: &str, rewritten: &str, events: &[&str]) {
    let pod = Pod::start(vec![ok_json("{\"id\":\"plain\"}"), stream(events)]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![code_model()], &source);

    let plain = gateway.post_text(path, request_body);
    assert_eq!(plain.status, 200, "{plain:?}");
    assert_eq!(plain.text(), "{\"id\":\"plain\"}");
    assert_eq!(plain.header("content-type"), Some("application/json"));

    let streamed = gateway.post_text(path, request_body);
    assert_eq!(streamed.status, 200, "{streamed:?}");
    assert_eq!(streamed.text(), events.concat());
    assert_eq!(streamed.header("content-type"), Some("text/event-stream"));

    let seen = pod.seen();
    assert_eq!(seen.len(), 2);
    for request in &seen {
        assert_eq!(request.method, "POST");
        assert_eq!(
            request.path, path,
            "the target is asked on the wire's own path"
        );
        assert_eq!(request.host.as_deref(), Some(pod.authority.as_str()));
        assert_eq!(request.body_text(), rewritten);
        assert_eq!(
            request.content_length.as_deref(),
            Some(rewritten.len().to_string().as_str())
        );
    }
    assert_eq!(source.acquired(), vec!["code", "code"]);
    assert_eq!(source.invalidated(), Vec::<(String, String)>::new());
    assert!(source.released_reaches(2));
}

#[test]
fn r6_chat_completions_are_relayed_to_the_chat_path_streaming_and_not() {
    relays_on(
        "/v1/chat/completions",
        "{\"model\":\"code\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}",
        "{\"model\":\"served-code\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}",
        &[
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: [DONE]\n\n",
        ],
    );
}

#[test]
fn r7_responses_are_relayed_to_the_responses_path_streaming_and_not() {
    relays_on(
        "/v1/responses",
        "{\"model\":\"code\",\"input\":\"hi\"}",
        "{\"model\":\"served-code\",\"input\":\"hi\"}",
        &[
            "event: response.created\ndata: {\"type\":\"response.created\"}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n",
        ],
    );
}

#[test]
fn r8_messages_are_relayed_to_the_messages_path_streaming_and_not() {
    relays_on(
        "/v1/messages",
        "{\"model\":\"code\",\"max_tokens\":16,\"messages\":[]}",
        "{\"model\":\"served-code\",\"max_tokens\":16,\"messages\":[]}",
        &[
            "event: message_start\ndata: {\"type\":\"message_start\"}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ],
    );
}

#[test]
fn r6_a_relay_needs_the_owner_credential_and_asks_for_no_target_without_it() {
    let pod = Pod::start(vec![ok_json("{}")]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![code_model()], &source);
    let body = b"{\"model\":\"code\"}";
    for path in ["/v1/chat/completions", "/v1/responses", "/v1/messages"] {
        let absent = send(gateway.addr, &request("POST", path, None, body));
        assert_eq!(
            absent.refused(),
            (401, Some("credential-absent".into())),
            "{path}"
        );
        let wrong = send(
            gateway.addr,
            &request(
                "POST",
                path,
                Some("not-the-owner-token-bbbbbbbbbbbbbbbbbbbb"),
                body,
            ),
        );
        assert_eq!(
            wrong.refused(),
            (401, Some("credential-rejected".into())),
            "{path}"
        );
    }
    assert_eq!(source.acquired(), Vec::<String>::new());
    assert_eq!(pod.seen().len(), 0);
}

#[test]
fn r6_a_relay_before_readiness_is_unavailable_and_another_method_is_not_allowed() {
    let pod = Pod::start(vec![ok_json("{}")]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::compose(vec![code_model()], &source, false);
    let unready = gateway.post_text("/v1/chat/completions", "{\"model\":\"code\"}");
    assert_eq!(unready.refused(), (503, Some("unavailable".into())));

    gateway.handle.as_ref().unwrap().mark_ready();
    for method in ["GET", "PUT", "DELETE"] {
        let other = send(
            gateway.addr,
            &request(method, "/v1/messages", Some(OWNER), b""),
        );
        assert_eq!(
            other.refused(),
            (405, Some("method-not-allowed".into())),
            "{method}"
        );
        assert_eq!(other.header("allow"), Some("POST"), "{method}");
    }
    assert_eq!(source.acquired(), Vec::<String>::new());
    assert_eq!(pod.seen().len(), 0);
}

#[test]
fn r6_a_model_the_source_has_no_target_for_is_target_unavailable() {
    let source = Source::new(Vec::new());
    let gateway = Relayed::start(vec![code_model()], &source);
    let answer = gateway.post_text("/v1/chat/completions", "{\"model\":\"code\"}");
    assert_eq!(answer.refused(), (503, Some("target-unavailable".into())));
    assert_eq!(
        answer.text(),
        "{\"error\":{\"code\":\"target-unavailable\",\"message\":\"no model target is available\"}}"
    );
    assert_eq!(source.acquired(), vec!["code"]);
}

// --- W6: a cold start past the hold budget ---------------------------------------------------------

const COLD_START: &str = "{\"error\":{\"code\":\"model-cold-start\",\"message\":\"the model is still starting; ask again after the retry-after delay\"}}";

#[test]
fn w6_a_model_still_starting_past_its_hold_budget_is_model_cold_start_with_retry_after_30() {
    let pod = Pod::start(vec![ok_json("{\"id\":\"warm\"}")]);
    let source = Source::cold(vec![listening(&pod)], 1);
    let gateway = Relayed::start(vec![code_model()], &source);
    let cold = gateway.post_text("/v1/chat/completions", "{\"model\":\"code\"}");
    assert_eq!(cold.refused(), (503, Some("model-cold-start".into())));
    assert_eq!(cold.text(), COLD_START);
    assert_eq!(cold.header("retry-after"), Some("30"));
    assert_eq!(cold.header("cache-control"), Some("no-store"));
    // The pod serves now: the next request reaches it, and its answer carries no retry-after.
    let warm = gateway.post_text("/v1/responses", "{\"model\":\"code\"}");
    assert_eq!(warm.status, 200, "{}", warm.text());
    assert_eq!(warm.text(), "{\"id\":\"warm\"}");
    assert_eq!(warm.header("retry-after"), None);
    assert_eq!(source.acquired(), vec!["code", "code"]);
    assert!(
        source.invalidated().is_empty(),
        "a cold start is no failure"
    );
    assert_eq!(pod.seen().len(), 1);
}

#[test]
fn w6_only_a_cold_start_carries_retry_after() {
    // No target at all is `target-unavailable`: the model is down, not starting, so the client
    // is not told when to come back.
    let source = Source::new(Vec::new());
    let gateway = Relayed::start(vec![code_model()], &source);
    let answer = gateway.post_text("/v1/chat/completions", "{\"model\":\"code\"}");
    assert_eq!(answer.refused(), (503, Some("target-unavailable".into())));
    assert_eq!(answer.header("retry-after"), None);
    // A request refused before a target is asked for never reports a cold start, even from a
    // source that would.
    let cold = Source::cold(Vec::new(), 1);
    let gateway = Relayed::start(vec![code_model()], &cold);
    let unknown = gateway.post_text("/v1/chat/completions", "{\"model\":\"other\"}");
    assert_eq!(unknown.refused(), (404, Some("model-unknown".into())));
    assert_eq!(unknown.header("retry-after"), None);
    assert!(cold.acquired().is_empty());
}

// --- W1: the body bound ----------------------------------------------------------------------

#[test]
fn w1_a_body_of_exactly_32_mib_is_relayed_whole() {
    let pod = Pod::start(vec![ok_json("{\"id\":\"big\"}")]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![code_model()], &source);
    let body = pad_to("{\"model\":\"code\"", BODY_BOUND);

    let answer = gateway.post("/v1/chat/completions", &body);
    assert_eq!(answer.status, 200, "{:?}", answer.code());

    let seen = pod.seen();
    assert_eq!(seen.len(), 1);
    let expected = pad_to("{\"model\":\"served-code\"", BODY_BOUND + 7);
    assert_eq!(seen[0].body.len(), expected.len());
    assert!(
        seen[0].body.as_slice().eq(expected.as_slice()),
        "the padded body changed beyond its model"
    );
}

#[test]
fn w1_a_body_declared_one_byte_over_32_mib_is_refused_before_it_is_read() {
    let pod = Pod::start(vec![ok_json("{}")]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![code_model()], &source);
    for path in ["/v1/chat/completions", "/v1/responses", "/v1/messages"] {
        // Only the head: no byte of the declared body is ever sent.
        let head = format!(
            "POST {path} HTTP/1.1\r\nauthorization: Bearer {OWNER}\r\ncontent-length: {}\r\n\r\n",
            BODY_BOUND + 1
        );
        let answer = send(gateway.addr, head.as_bytes());
        assert_eq!(
            answer.refused(),
            (413, Some("body-too-large".into())),
            "{path}"
        );
        assert_eq!(
            answer.text(),
            "{\"error\":{\"code\":\"body-too-large\",\"message\":\"the request body exceeds its byte bound\"}}"
        );
    }
    assert_eq!(source.acquired(), Vec::<String>::new());
    assert_eq!(pod.seen().len(), 0);
}

#[test]
fn w1_a_chunked_body_of_exactly_32_mib_is_decoded_and_relayed_with_a_length() {
    let pod = Pod::start(vec![ok_json("{}")]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![code_model()], &source);
    let body = pad_to("{\"model\":\"code\"", BODY_BOUND);
    let (first, second) = body.split_at(BODY_BOUND / 2);
    let mut raw = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nauthorization: Bearer {OWNER}\r\ntransfer-encoding: chunked\r\n\r\n"
    )
    .into_bytes();
    for part in [first, second] {
        raw.extend_from_slice(format!("{:x}\r\n", part.len()).as_bytes());
        raw.extend_from_slice(part);
        raw.extend_from_slice(b"\r\n");
    }
    raw.extend_from_slice(b"0\r\n\r\n");

    let answer = send(gateway.addr, &raw);
    assert_eq!(answer.status, 200, "{:?}", answer.code());
    let seen = pod.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].transfer_encoding, None);
    assert_eq!(
        seen[0].content_length.as_deref(),
        Some((BODY_BOUND + 7).to_string().as_str())
    );
    let expected = pad_to("{\"model\":\"served-code\"", BODY_BOUND + 7);
    assert!(
        seen[0].body.as_slice().eq(expected.as_slice()),
        "the decoded body changed beyond its model"
    );
}

#[test]
fn w1_a_chunked_body_is_refused_the_moment_it_passes_32_mib() {
    let pod = Pod::start(vec![ok_json("{}")]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![code_model()], &source);
    // Exactly the bound in one chunk, then a chunk head announcing one more byte, which is
    // never sent: the refusal can only come from the decoded size passing the bound.
    let mut raw = format!(
        "POST /v1/messages HTTP/1.1\r\nauthorization: Bearer {OWNER}\r\ntransfer-encoding: chunked\r\n\r\n{BODY_BOUND:x}\r\n"
    )
    .into_bytes();
    raw.extend_from_slice(&pad_to("{\"model\":\"code\"", BODY_BOUND));
    raw.extend_from_slice(b"\r\n1\r\n");

    let answer = send(gateway.addr, &raw);
    assert_eq!(answer.refused(), (413, Some("body-too-large".into())));
    assert_eq!(source.acquired(), Vec::<String>::new());
    assert_eq!(pod.seen().len(), 0);
}

// --- W2: the model rewrite -------------------------------------------------------------------

#[test]
fn w2_only_the_top_level_model_is_rewritten_on_every_wire_and_no_other_byte_changes() {
    let cases = [
        (
            "/v1/chat/completions",
            "{\"messages\":[{\"role\":\"user\",\"content\":\"model\"}],\"model\":\"code\",\"metadata\":{\"model\":\"code\"}}",
            "{\"messages\":[{\"role\":\"user\",\"content\":\"model\"}],\"model\":\"served-code\",\"metadata\":{\"model\":\"code\"}}",
        ),
        (
            "/v1/responses",
            "{ \"input\" : \"x\" ,\n  \"model\" : \"code\" }",
            "{ \"input\" : \"x\" ,\n  \"model\" : \"served-code\" }",
        ),
        (
            "/v1/messages",
            "{\"model\":\"code\",\"system\":\"the \\\"model\\\" is code\"}",
            "{\"model\":\"served-code\",\"system\":\"the \\\"model\\\" is code\"}",
        ),
        (
            "/v1/chat/completions",
            "{\"model\":\"co\\u0064e\",\"n\":1}",
            "{\"model\":\"served-code\",\"n\":1}",
        ),
    ];
    let pod = Pod::start(cases.iter().map(|_| ok_json("{}")).collect());
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![code_model()], &source);
    for (path, sent, _) in cases {
        let answer = gateway.post_text(path, sent);
        assert_eq!(answer.status, 200, "{path}: {:?}", answer.code());
    }
    let seen = pod.seen();
    assert_eq!(seen.len(), cases.len());
    for (request, (path, _, rewritten)) in seen.iter().zip(cases) {
        assert_eq!(request.path, path);
        assert_eq!(request.body_text(), rewritten);
    }
}

#[test]
fn w2_the_upstream_name_is_written_as_a_json_string() {
    let pod = Pod::start(vec![ok_json("{}")]);
    let source = Source::of(&[&pod]);
    let model = RelayModel::new(label("code"), label("served\"code\\x"), all_wires()).unwrap();
    let gateway = Relayed::start(vec![model], &source);
    let answer = gateway.post_text("/v1/chat/completions", "{\"model\":\"code\"}");
    assert_eq!(answer.status, 200, "{:?}", answer.code());
    assert_eq!(
        pod.seen()[0].body_text(),
        "{\"model\":\"served\\\"code\\\\x\"}"
    );
}

// --- W3: typed refusals ----------------------------------------------------------------------

/// Sends each body and expects the same refusal for all of them, with no target asked for.
fn refuses_all(bodies: &[&[u8]], status: u16, code: &str, message: &str) {
    let pod = Pod::start(vec![ok_json("{}")]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![code_model()], &source);
    for body in bodies {
        for path in ["/v1/chat/completions", "/v1/responses", "/v1/messages"] {
            let answer = gateway.post(path, body);
            assert_eq!(
                answer.refused(),
                (status, Some(code.to_string())),
                "{path} {}",
                String::from_utf8_lossy(body)
            );
            assert_eq!(
                answer.text(),
                format!("{{\"error\":{{\"code\":\"{code}\",\"message\":\"{message}\"}}}}")
            );
            assert_eq!(answer.header("cache-control"), Some("no-store"));
        }
    }
    assert_eq!(
        source.acquired(),
        Vec::<String>::new(),
        "a refused request woke a target"
    );
    assert_eq!(pod.seen().len(), 0);
}

#[test]
fn w3_a_body_that_is_not_one_json_object_is_body_not_json() {
    refuses_all(
        &[
            b"",
            b"not json",
            b"[{\"model\":\"code\"}]",
            b"\"code\"",
            b"{\"model\":\"code\"",
            b"{\"model\":\"code\"} trailing",
            b"{\"model\":\"code\",}",
            b"{\"model\":\"co\xffde\"}",
            b"{\"model\":\"code\",\"model\":\"code\"}",
        ],
        400,
        "body-not-json",
        "the request body is not one JSON object",
    );
}

#[test]
fn w3_an_object_without_a_top_level_model_string_is_model_absent() {
    refuses_all(
        &[
            b"{}",
            b"{\"messages\":[]}",
            b"{\"model\":7}",
            b"{\"model\":null}",
            b"{\"model\":[\"code\"]}",
            b"{\"metadata\":{\"model\":\"code\"}}",
        ],
        400,
        "model-absent",
        "the request body names no model",
    );
}

#[test]
fn w3_a_model_nobody_serves_is_model_unknown_with_404() {
    refuses_all(
        &[
            b"{\"model\":\"other\"}",
            b"{\"model\":\"served-code\"}",
            b"{\"model\":\"Code\"}",
            b"{\"model\":\"\"}",
        ],
        404,
        "model-unknown",
        "no such model",
    );
}

// --- W4: the messages wire's effort mapping --------------------------------------------------

fn rewrites(path: &str, cases: &[(&str, &str)]) {
    let pod = Pod::start(cases.iter().map(|_| ok_json("{}")).collect());
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![code_model()], &source);
    for (sent, _) in cases {
        let answer = gateway.post_text(path, sent);
        assert_eq!(answer.status, 200, "{sent}: {:?}", answer.code());
    }
    let seen: Vec<String> = pod.seen().iter().map(Seen::body_text).collect();
    let expected: Vec<&str> = cases.iter().map(|(_, rewritten)| *rewritten).collect();
    assert_eq!(seen, expected);
}

#[test]
fn w4_on_the_messages_wire_effort_high_reaches_the_target_as_xhigh() {
    rewrites(
        "/v1/messages",
        &[
            (
                "{\"model\":\"code\",\"output_config\":{\"effort\":\"high\"},\"chat_template_kwargs\":{\"reasoning_effort\":\"high\"}}",
                "{\"model\":\"served-code\",\"output_config\":{\"effort\":\"xhigh\"},\"chat_template_kwargs\":{\"reasoning_effort\":\"xhigh\"}}",
            ),
            (
                "{\"output_config\":{\"other\":1,\"effort\":\"high\"},\"model\":\"code\"}",
                "{\"output_config\":{\"other\":1,\"effort\":\"xhigh\"},\"model\":\"served-code\"}",
            ),
            (
                "{\"model\":\"code\",\"chat_template_kwargs\":{\"reasoning_effort\":\"high\"}}",
                "{\"model\":\"served-code\",\"chat_template_kwargs\":{\"reasoning_effort\":\"xhigh\"}}",
            ),
        ],
    );
}

#[test]
fn w4_any_other_effort_on_the_messages_wire_passes_unchanged() {
    rewrites(
        "/v1/messages",
        &[
            (
                "{\"model\":\"code\",\"output_config\":{\"effort\":\"medium\"},\"chat_template_kwargs\":{\"reasoning_effort\":\"low\"}}",
                "{\"model\":\"served-code\",\"output_config\":{\"effort\":\"medium\"},\"chat_template_kwargs\":{\"reasoning_effort\":\"low\"}}",
            ),
            (
                "{\"model\":\"code\",\"output_config\":{\"effort\":\"HIGH\"},\"chat_template_kwargs\":{\"reasoning_effort\":\" high\"}}",
                "{\"model\":\"served-code\",\"output_config\":{\"effort\":\"HIGH\"},\"chat_template_kwargs\":{\"reasoning_effort\":\" high\"}}",
            ),
            (
                "{\"model\":\"code\",\"effort\":\"high\",\"metadata\":{\"output_config\":{\"effort\":\"high\"}}}",
                "{\"model\":\"served-code\",\"effort\":\"high\",\"metadata\":{\"output_config\":{\"effort\":\"high\"}}}",
            ),
            (
                "{\"model\":\"code\",\"output_config\":{\"effort\":[\"high\"]}}",
                "{\"model\":\"served-code\",\"output_config\":{\"effort\":[\"high\"]}}",
            ),
        ],
    );
}

#[test]
fn w4_on_the_chat_and_responses_wires_effort_high_passes_unchanged() {
    let messages_fields = "\"output_config\":{\"effort\":\"high\"},\"chat_template_kwargs\":{\"reasoning_effort\":\"high\"}";
    rewrites(
        "/v1/chat/completions",
        &[(
            &format!("{{\"model\":\"code\",\"reasoning_effort\":\"high\",{messages_fields}}}"),
            &format!(
                "{{\"model\":\"served-code\",\"reasoning_effort\":\"high\",{messages_fields}}}"
            ),
        )],
    );
    rewrites(
        "/v1/responses",
        &[(
            &format!(
                "{{\"model\":\"code\",\"reasoning\":{{\"effort\":\"high\"}},{messages_fields}}}"
            ),
            &format!(
                "{{\"model\":\"served-code\",\"reasoning\":{{\"effort\":\"high\"}},{messages_fields}}}"
            ),
        )],
    );
}

// --- W5: bytes and streams relayed as they arrive --------------------------------------------

#[test]
fn w5_an_event_stream_reaches_the_client_event_by_event_and_holds_its_target_to_the_end() {
    let events = [
        "data: {\"n\":1}\n\n",
        "data: {\"n\":2}\n\n",
        "data: [DONE]\n\n",
    ];
    let (release, held): (Sender<()>, Receiver<()>) = mpsc::channel();
    let pod = Pod::start(vec![Answer::Reply {
        status: 200,
        content_type: "text/event-stream; charset=utf-8",
        chunks: events
            .iter()
            .map(|event| event.as_bytes().to_vec())
            .collect(),
        framing: Framing::Chunked,
        hold_head: None,
        hold_chunks: Some(held),
    }]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![code_model()], &source);

    let mut reading = Reading::open(
        gateway.addr,
        "/v1/chat/completions",
        "{\"model\":\"code\",\"stream\":true}",
    );
    let mut so_far = String::new();
    for (index, event) in events.iter().enumerate() {
        if index > 0 {
            release.send(()).unwrap();
        }
        so_far.push_str(event);
        assert!(
            reading.until_body_is(so_far.as_bytes()),
            "event {index} did not reach the client before the pod sent the next; got {:?}",
            String::from_utf8_lossy(&reading.body_so_far())
        );
        if index + 1 < events.len() {
            assert_eq!(
                source.released(),
                0,
                "the target was released while its stream was still open"
            );
        }
    }
    let answer = reading.finish();
    assert_eq!(answer.status, 200);
    assert_eq!(answer.text(), events.concat());
    assert_eq!(
        answer.header("content-type"),
        Some("text/event-stream; charset=utf-8")
    );
    assert_eq!(answer.header("cache-control"), Some("no-store"));
    assert!(source.released_reaches(1));
}

#[test]
fn w5_the_targets_status_content_type_and_bytes_reach_the_client_unchanged() {
    let binary: Vec<u8> = (0..=255_u8).chain([0, 255, 13, 10]).collect();
    let script = vec![
        reply(200, "application/octet-stream", &binary),
        Answer::Reply {
            status: 201,
            content_type: "application/json; charset=utf-8",
            chunks: vec![b"{\"id\":".to_vec(), b"\"split\"}".to_vec()],
            framing: Framing::Chunked,
            hold_head: None,
            hold_chunks: None,
        },
        Answer::Reply {
            status: 200,
            content_type: "text/plain",
            chunks: vec![b"closed ".to_vec(), b"delimited".to_vec()],
            framing: Framing::Close,
            hold_head: None,
            hold_chunks: None,
        },
    ];
    let pod = Pod::start(script);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![code_model()], &source);

    let raw = gateway.post_text("/v1/chat/completions", "{\"model\":\"code\"}");
    assert_eq!(raw.status, 200);
    assert_eq!(raw.body, binary);
    assert_eq!(raw.header("content-type"), Some("application/octet-stream"));
    assert_eq!(raw.header("cache-control"), Some("no-store"));

    let created = gateway.post_text("/v1/responses", "{\"model\":\"code\"}");
    assert_eq!(created.status, 201);
    assert_eq!(created.text(), "{\"id\":\"split\"}");
    assert_eq!(
        created.header("content-type"),
        Some("application/json; charset=utf-8")
    );
    assert_eq!(created.header("cache-control"), Some("no-store"));

    let closed = gateway.post_text("/v1/messages", "{\"model\":\"code\"}");
    assert_eq!(closed.status, 200);
    assert_eq!(closed.text(), "closed delimited");
    assert_eq!(closed.header("cache-control"), Some("no-store"));
    assert!(source.released_reaches(3));
}

// --- W7: a failed target is dropped ----------------------------------------------------------

#[test]
fn w7_an_upstream_502_503_or_504_drops_that_endpoint_and_the_next_request_reaches_a_replacement() {
    let failing: Vec<Pod> = [502_u16, 503, 504]
        .into_iter()
        .map(|status| Pod::start(vec![reply(status, "text/plain", b"proxy says no")]))
        .collect();
    let replacement = Pod::start(vec![ok_json("{\"id\":\"replacement\"}")]);
    let mut pods: Vec<&Pod> = failing.iter().collect();
    pods.push(&replacement);
    let source = Source::of(&pods);
    let gateway = Relayed::start(vec![code_model()], &source);

    for (pod, path) in failing
        .iter()
        .zip(["/v1/chat/completions", "/v1/responses", "/v1/messages"])
    {
        let answer = gateway.post_text(path, "{\"model\":\"code\"}");
        assert_eq!(
            answer.refused(),
            (502, Some("upstream-failed".into())),
            "{path}"
        );
        assert_eq!(
            answer.text(),
            UPSTREAM_FAILED,
            "the pod's own text is not relayed"
        );
        assert_eq!(pod.seen().len(), 1);
    }
    let answer = gateway.post_text("/v1/chat/completions", "{\"model\":\"code\"}");
    assert_eq!(answer.status, 200);
    assert_eq!(answer.text(), "{\"id\":\"replacement\"}");

    let reported: Vec<(String, String)> = failing
        .iter()
        .map(|pod| ("code".to_string(), pod.authority.clone()))
        .collect();
    assert_eq!(source.invalidated(), reported);
    assert_eq!(source.acquired().len(), 4);
    assert!(source.released_reaches(4));
}

#[test]
fn w7_a_refused_or_dropped_connection_drops_that_endpoint_and_answers_upstream_failed() {
    let dropping = Pod::start(vec![Answer::Close]);
    let replacement = Pod::start(vec![ok_json("{\"id\":\"replacement\"}")]);
    let refused = refusing("pod-refused");
    let source = Source::new(vec![
        refused.clone(),
        listening(&dropping),
        listening(&replacement),
    ]);
    let gateway = Relayed::start(vec![code_model()], &source);

    for _ in 0..2 {
        let answer = gateway.post_text("/v1/chat/completions", "{\"model\":\"code\"}");
        assert_eq!(answer.refused(), (502, Some("upstream-failed".into())));
        assert_eq!(answer.text(), UPSTREAM_FAILED);
    }
    let answer = gateway.post_text("/v1/chat/completions", "{\"model\":\"code\"}");
    assert_eq!(answer.status, 200);
    assert_eq!(answer.text(), "{\"id\":\"replacement\"}");

    assert_eq!(
        source.invalidated(),
        vec![
            ("code".to_string(), refused.authority),
            ("code".to_string(), dropping.authority.clone()),
        ]
    );
    assert_eq!(
        dropping.seen().len(),
        1,
        "the dropping pod did read the request"
    );
    assert!(source.released_reaches(3));
}

#[test]
fn w7_the_failure_names_the_endpoint_that_served_it_so_a_newer_one_is_kept() {
    let (answer_now, held) = mpsc::channel();
    let stale = Pod::start(vec![Answer::Reply {
        status: 503,
        content_type: "text/plain",
        chunks: vec![b"gone".to_vec()],
        framing: Framing::Length,
        hold_head: Some(held),
        hold_chunks: None,
    }]);
    let newer = Pod::start(vec![ok_json("{\"id\":\"newer\"}")]);
    let source = Source::of(&[&stale, &newer]);
    let gateway = Relayed::start(vec![code_model()], &source);

    let reading = Reading::open(gateway.addr, "/v1/chat/completions", "{\"model\":\"code\"}");
    // The request holds the stale pod; meanwhile the source has moved on to a newer one.
    let deadline = Instant::now() + WAIT;
    while stale.seen().is_empty() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(stale.seen().len(), 1);
    source.make_current(1);
    answer_now.send(()).unwrap();
    let failed = reading.finish();
    assert_eq!(failed.refused(), (502, Some("upstream-failed".into())));
    assert_eq!(
        source.invalidated(),
        vec![("code".to_string(), stale.authority.clone())]
    );

    let answer = gateway.post_text("/v1/chat/completions", "{\"model\":\"code\"}");
    assert_eq!(
        answer.text(),
        "{\"id\":\"newer\"}",
        "the newer endpoint was kept"
    );
}

#[test]
fn w7_a_model_error_is_relayed_with_its_body_and_keeps_the_endpoint() {
    let pod = Pod::start(vec![
        reply(
            500,
            "application/json",
            b"{\"type\":\"error\",\"error\":{\"type\":\"internal_error\"}}",
        ),
        reply(
            400,
            "application/json",
            b"{\"error\":{\"message\":\"bad effort\"}}",
        ),
        reply(429, "application/json", b"{\"error\":\"slow down\"}"),
        ok_json("{\"id\":\"same-pod\"}"),
    ]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![code_model()], &source);
    let expected = [
        (
            500,
            "{\"type\":\"error\",\"error\":{\"type\":\"internal_error\"}}",
        ),
        (400, "{\"error\":{\"message\":\"bad effort\"}}"),
        (429, "{\"error\":\"slow down\"}"),
        (200, "{\"id\":\"same-pod\"}"),
    ];
    for (status, body) in expected {
        let answer = gateway.post_text("/v1/messages", "{\"model\":\"code\"}");
        assert_eq!(answer.status, status);
        assert_eq!(answer.text(), body);
    }
    assert_eq!(source.invalidated(), Vec::<(String, String)>::new());
    assert_eq!(pod.seen().len(), 4);
}

// --- K11: each model's wire set ----------------------------------------------------------------

#[test]
fn k11_a_wire_the_model_does_not_declare_is_refused_without_asking_for_a_target() {
    let pod = Pod::start(vec![
        ok_json("{\"id\":\"chat\"}"),
        ok_json("{\"id\":\"msg\"}"),
    ]);
    let source = Source::of(&[&pod]);
    let chat_only =
        RelayModel::new(label("chat-only"), label("served-chat"), vec![Wire::Chat]).unwrap();
    let no_chat = RelayModel::new(
        label("no-chat"),
        label("served-no-chat"),
        vec![Wire::Messages, Wire::Responses],
    )
    .unwrap();
    let gateway = Relayed::start(vec![chat_only, no_chat], &source);

    for (path, model) in [
        ("/v1/messages", "chat-only"),
        ("/v1/responses", "chat-only"),
        ("/v1/chat/completions", "no-chat"),
    ] {
        let answer = gateway.post_text(path, &format!("{{\"model\":\"{model}\"}}"));
        assert_eq!(
            answer.refused(),
            (400, Some("wire-not-served".into())),
            "{path}"
        );
        assert_eq!(
            answer.text(),
            "{\"error\":{\"code\":\"wire-not-served\",\"message\":\"the model is not served on this wire\"}}"
        );
    }
    assert_eq!(
        source.acquired(),
        Vec::<String>::new(),
        "an undeclared wire woke a target"
    );

    let chat = gateway.post_text("/v1/chat/completions", "{\"model\":\"chat-only\"}");
    assert_eq!(chat.text(), "{\"id\":\"chat\"}");
    let messages = gateway.post_text("/v1/messages", "{\"model\":\"no-chat\"}");
    assert_eq!(messages.text(), "{\"id\":\"msg\"}");
    assert_eq!(source.acquired(), vec!["chat-only", "no-chat"]);
    let bodies: Vec<String> = pod.seen().iter().map(Seen::body_text).collect();
    assert_eq!(
        bodies,
        vec![
            "{\"model\":\"served-chat\"}",
            "{\"model\":\"served-no-chat\"}"
        ]
    );
}

#[test]
fn k11_a_model_declares_a_non_empty_set_of_distinct_wires() {
    let model = |wires: Vec<Wire>| RelayModel::new(label("code"), label("served-code"), wires);
    assert_eq!(model(Vec::new()).err(), Some(RelayError::NoWire));
    assert_eq!(
        model(vec![Wire::Chat, Wire::Messages, Wire::Chat]).err(),
        Some(RelayError::RepeatedWire)
    );
    // Every non-empty subset of the three wires, in any order, is a valid wire set.
    let subsets: [&[Wire]; 7] = [
        &[Wire::Chat],
        &[Wire::Responses],
        &[Wire::Messages],
        &[Wire::Chat, Wire::Responses],
        &[Wire::Messages, Wire::Chat],
        &[Wire::Responses, Wire::Messages],
        &[Wire::Messages, Wire::Responses, Wire::Chat],
    ];
    for subset in subsets {
        assert!(model(subset.to_vec()).is_ok(), "{subset:?}");
    }
}

#[test]
fn k11_two_models_under_one_alias_are_refused() {
    let source = Source::new(Vec::new());
    let targets: Arc<dyn RelayTargets> = source;
    let first = RelayModel::new(label("code"), label("served-code"), vec![Wire::Chat]).unwrap();
    let second = RelayModel::new(label("code"), label("other-code"), vec![Wire::Messages]).unwrap();
    assert_eq!(
        Relay::new(vec![first, second], targets).err(),
        Some(RelayError::DuplicateModel)
    );
}

// --- B8: the target's own credential, never the owner's ---------------------------------------

const VLLM_KEY: &str = "vllm-key-code-0123456789abcdef";

#[test]
fn b8_a_target_with_a_bearer_receives_it_and_never_the_owners_credential() {
    let pod = Pod::start(vec![
        ok_json("{\"id\":\"one\"}"),
        stream(&["data: [DONE]\n\n"]),
    ]);
    let mut slot = listening(&pod);
    slot.bearer = Some(Arc::new(
        TargetBearer::new(VLLM_KEY.as_bytes().to_vec()).unwrap(),
    ));
    let source = Source::new(vec![slot]);
    let gateway = Relayed::start(vec![code_model()], &source);
    for (path, body) in [
        ("/v1/chat/completions", "{\"model\":\"code\"}"),
        ("/v1/messages", "{\"model\":\"code\",\"stream\":true}"),
    ] {
        assert_eq!(gateway.post_text(path, body).status, 200, "{path}");
    }
    let seen = pod.seen();
    assert_eq!(seen.len(), 2);
    for request in &seen {
        assert_eq!(
            request.authorization.as_deref(),
            Some(format!("Bearer {VLLM_KEY}").as_str()),
            "{}",
            request.path
        );
        assert!(
            find(&request.raw, OWNER.as_bytes()).is_none(),
            "the owner's credential reached the pod on {}",
            request.path
        );
    }
}

#[test]
fn b8_a_target_without_a_bearer_receives_no_authorization_at_all() {
    let pod = Pod::start(vec![ok_json("{\"id\":\"one\"}")]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![code_model()], &source);
    assert_eq!(
        gateway
            .post_text("/v1/chat/completions", "{\"model\":\"code\"}")
            .status,
        200
    );
    let seen = pod.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].authorization, None);
    assert!(find(&seen[0].raw, OWNER.as_bytes()).is_none());
}

#[test]
fn b8_a_bearer_follows_the_owner_material_rules() {
    assert!(TargetBearer::new(b"k".to_vec()).is_ok());
    assert!(TargetBearer::new(vec![b'k'; 4096]).is_ok());
    for (material, rule) in [
        (Vec::new(), TokenError::Empty),
        (vec![b'k'; 4097], TokenError::TooLarge),
        (b"vllm key".to_vec(), TokenError::NotPrintableAscii),
        (
            b"vllm\r\nx-injected: 1".to_vec(),
            TokenError::NotPrintableAscii,
        ),
    ] {
        assert_eq!(TargetBearer::new(material).err(), Some(rule));
    }
    let bearer = TargetBearer::new(VLLM_KEY.as_bytes().to_vec()).unwrap();
    assert!(!format!("{bearer:?}").contains(VLLM_KEY));
}

// --- Hosted endpoints: a credential sent in a header the target names -------------------------

/// story:hosted-endpoints, criterion 2 at the head the gateway writes: a target naming a
/// credential header (an `api-key` endpoint) receives its key as that header's value and no
/// `authorization`, on every wire, and never the owner's credential. The control is
/// `b8_a_target_with_a_bearer_receives_it_and_never_the_owners_credential`.
#[test]
fn hosted_a_target_naming_a_credential_header_receives_its_key_there_and_no_authorization() {
    let pod = Pod::start(vec![
        ok_json("{\"id\":\"one\"}"),
        stream(&["data: [DONE]\n\n"]),
    ]);
    let mut slot = listening(&pod);
    slot.bearer = Some(Arc::new(
        TargetBearer::new(VLLM_KEY.as_bytes().to_vec()).unwrap(),
    ));
    slot.header = Some(CredentialHeader::new("X-Api-Key").unwrap());
    let source = Source::new(vec![slot]);
    let gateway = Relayed::start(vec![code_model()], &source);
    for (path, body) in [
        ("/v1/chat/completions", "{\"model\":\"code\"}"),
        ("/v1/messages", "{\"model\":\"code\",\"stream\":true}"),
    ] {
        assert_eq!(gateway.post_text(path, body).status, 200, "{path}");
    }
    let seen = pod.seen();
    assert_eq!(seen.len(), 2);
    for request in &seen {
        let head = String::from_utf8_lossy(&request.raw).into_owned();
        let head = head.split("\r\n\r\n").next().unwrap();
        assert_eq!(request.authorization, None, "{head}");
        assert_eq!(
            header_of(head, "x-api-key").as_deref(),
            Some(VLLM_KEY),
            "{head}"
        );
        assert!(
            head.contains(&format!("\r\nx-api-key: {VLLM_KEY}\r\n")),
            "{head}"
        );
        assert!(find(&request.raw, OWNER.as_bytes()).is_none(), "{head}");
    }
}

/// A credential header is one RFC 9110 token of at most `credential-header-bytes` (64), held
/// lowercase, and never a header the gateway writes itself or one that frames a message: it
/// can neither end its line nor smuggle a second framing or `authorization`.
#[test]
fn hosted_a_credential_header_is_one_token_and_never_one_the_gateway_writes() {
    for name in [
        "x-api-key",
        "api-key",
        "X-Goog-Api-Key",
        "a",
        &"k".repeat(64),
        "x!#$%&'*+.^_`|~1",
    ] {
        let header =
            CredentialHeader::new(name).unwrap_or_else(|error| panic!("{name:?}: {error}"));
        assert_eq!(header.as_str(), name.to_ascii_lowercase());
    }
    for (name, rule) in [
        (String::new(), CredentialHeaderError::Length),
        ("k".repeat(65), CredentialHeaderError::Length),
        ("x api key".to_owned(), CredentialHeaderError::NotAToken),
        ("x-api-key:".to_owned(), CredentialHeaderError::NotAToken),
        (
            "x-api-key\r\nx-injected".to_owned(),
            CredentialHeaderError::NotAToken,
        ),
        ("x-äpi".to_owned(), CredentialHeaderError::NotAToken),
        ("Authorization".to_owned(), CredentialHeaderError::Reserved),
        ("host".to_owned(), CredentialHeaderError::Reserved),
        ("Content-Length".to_owned(), CredentialHeaderError::Reserved),
        (
            "transfer-encoding".to_owned(),
            CredentialHeaderError::Reserved,
        ),
        ("content-type".to_owned(), CredentialHeaderError::Reserved),
        ("connection".to_owned(), CredentialHeaderError::Reserved),
        (
            "proxy-authorization".to_owned(),
            CredentialHeaderError::Reserved,
        ),
    ] {
        assert_eq!(CredentialHeader::new(&name).err(), Some(rule), "{name:?}");
    }
}

// --- Tool calling: a tool request to a model that cannot parse tool calls --------------------

const TOOLS_NOT_SERVED: &str = "{\"error\":{\"code\":\"tools-not-served\",\"message\":\"the model does not serve tool calls\"}}";

#[test]
fn tools_a_tool_request_to_a_model_without_tool_calling_is_refused_without_asking_for_a_target() {
    let pod = Pod::start(vec![ok_json("{}")]);
    let source = Source::of(&[&pod]);
    let declared = RelayModel::new(label("text"), label("served-text"), all_wires())
        .unwrap()
        .with_tool_calling(ToolCalling::Absent);
    // A model that says nothing about tool calling takes `Absent`.
    let undeclared =
        RelayModel::new(label("undeclared"), label("served-undeclared"), all_wires()).unwrap();
    assert_eq!(undeclared.tool_calling(), ToolCalling::Absent);
    let gateway = Relayed::start(vec![declared, undeclared], &source);

    for (path, body) in [
        (
            "/v1/chat/completions",
            "{\"model\":\"text\",\"tools\":[{\"type\":\"function\"}]}",
        ),
        (
            "/v1/responses",
            "{\"tools\" : [ {} ] , \"model\":\"text\",\"input\":\"hi\"}",
        ),
        (
            "/v1/messages",
            "{\"model\":\"text\",\"tools\":[{\"name\":\"shell\"}],\"messages\":[]}",
        ),
        // A second top-level `tools` that is non-empty is refused, whichever the target reads.
        (
            "/v1/chat/completions",
            "{\"model\":\"text\",\"tools\":[],\"tools\":[{}]}",
        ),
        (
            "/v1/chat/completions",
            "{\"model\":\"undeclared\",\"tools\":[{}]}",
        ),
    ] {
        let answer = gateway.post_text(path, body);
        assert_eq!(
            answer.refused(),
            (400, Some("tools-not-served".into())),
            "{path} {body}"
        );
        assert_eq!(answer.text(), TOOLS_NOT_SERVED);
    }
    assert_eq!(
        source.acquired(),
        Vec::<String>::new(),
        "a tool request to a model without tool calling woke a target"
    );
    assert!(pod.seen().is_empty());
}

#[test]
fn tools_a_tool_request_to_a_model_with_tool_calling_parsed_is_relayed_unchanged() {
    let pod = Pod::start(vec![ok_json("{\"id\":\"tool\"}")]);
    let source = Source::of(&[&pod]);
    let model = RelayModel::new(label("code"), label("served-code"), all_wires())
        .unwrap()
        .with_tool_calling(ToolCalling::Parsed);
    assert_eq!(model.tool_calling(), ToolCalling::Parsed);
    let gateway = Relayed::start(vec![model], &source);
    let answer = gateway.post_text(
        "/v1/chat/completions",
        "{\"model\":\"code\",\"tools\":[{\"type\":\"function\"}]}",
    );
    assert_eq!(answer.text(), "{\"id\":\"tool\"}");
    assert_eq!(source.acquired(), vec!["code"]);
    let bodies: Vec<String> = pod.seen().iter().map(Seen::body_text).collect();
    assert_eq!(
        bodies,
        vec!["{\"model\":\"served-code\",\"tools\":[{\"type\":\"function\"}]}"]
    );
}

#[test]
fn tools_a_request_offering_no_tool_to_a_model_without_tool_calling_is_relayed_unchanged() {
    let bodies = [
        // No `tools` key.
        "{\"model\":\"text\",\"messages\":[]}",
        // An empty array offers no tool.
        "{\"model\":\"text\",\"tools\": [ ]}",
        // Not an array: the target decides what it means.
        "{\"model\":\"text\",\"tools\":null}",
        "{\"model\":\"text\",\"tools\":{\"a\":1}}",
        // Only the top level counts.
        "{\"model\":\"text\",\"metadata\":{\"tools\":[{}]},\"messages\":[{\"tools\":[1]}]}",
    ];
    let pod = Pod::start(bodies.iter().map(|_| ok_json("{}")).collect());
    let source = Source::of(&[&pod]);
    let model = RelayModel::new(label("text"), label("served-text"), all_wires())
        .unwrap()
        .with_tool_calling(ToolCalling::Absent);
    let gateway = Relayed::start(vec![model], &source);
    for body in bodies {
        let answer = gateway.post_text("/v1/chat/completions", body);
        assert_eq!(answer.status, 200, "{body}: {}", answer.text());
    }
    assert_eq!(source.acquired().len(), bodies.len());
    let seen: Vec<String> = pod.seen().iter().map(Seen::body_text).collect();
    let expected: Vec<String> = bodies
        .iter()
        .map(|body| body.replacen("\"text\"", "\"served-text\"", 1))
        .collect();
    assert_eq!(seen, expected);
}

// --- W4: an effort of `max` ---------------------------------------------------------------------

#[test]
fn w4_on_the_messages_wire_effort_max_reaches_the_target_as_xhigh() {
    rewrites(
        "/v1/messages",
        &[
            (
                "{\"model\":\"code\",\"output_config\":{\"effort\":\"max\"},\"chat_template_kwargs\":{\"reasoning_effort\":\"max\"}}",
                "{\"model\":\"served-code\",\"output_config\":{\"effort\":\"xhigh\"},\"chat_template_kwargs\":{\"reasoning_effort\":\"xhigh\"}}",
            ),
            (
                "{\"output_config\":{\"format\":{\"type\":\"json_schema\"},\"effort\":\"m\\u0061x\"},\"model\":\"code\"}",
                "{\"output_config\":{\"format\":{\"type\":\"json_schema\"},\"effort\":\"xhigh\"},\"model\":\"served-code\"}",
            ),
            (
                "{\"model\":\"code\",\"output_config\":{\"effort\":\"high\"},\"chat_template_kwargs\":{\"reasoning_effort\":\"max\"}}",
                "{\"model\":\"served-code\",\"output_config\":{\"effort\":\"xhigh\"},\"chat_template_kwargs\":{\"reasoning_effort\":\"xhigh\"}}",
            ),
        ],
    );
}

#[test]
fn w4_low_medium_xhigh_and_any_other_effort_reach_the_target_unchanged() {
    rewrites(
        "/v1/messages",
        &[
            (
                "{\"model\":\"code\",\"output_config\":{\"effort\":\"low\"}}",
                "{\"model\":\"served-code\",\"output_config\":{\"effort\":\"low\"}}",
            ),
            (
                "{\"model\":\"code\",\"output_config\":{\"effort\":\"medium\"}}",
                "{\"model\":\"served-code\",\"output_config\":{\"effort\":\"medium\"}}",
            ),
            (
                "{\"model\":\"code\",\"output_config\":{\"effort\":\"xhigh\"},\"chat_template_kwargs\":{\"reasoning_effort\":\"xhigh\"}}",
                "{\"model\":\"served-code\",\"output_config\":{\"effort\":\"xhigh\"},\"chat_template_kwargs\":{\"reasoning_effort\":\"xhigh\"}}",
            ),
            (
                "{\"model\":\"code\",\"output_config\":{\"effort\":\"MAX\"},\"chat_template_kwargs\":{\"reasoning_effort\":\"maximum\"},\"effort\":\"max\"}",
                "{\"model\":\"served-code\",\"output_config\":{\"effort\":\"MAX\"},\"chat_template_kwargs\":{\"reasoning_effort\":\"maximum\"},\"effort\":\"max\"}",
            ),
        ],
    );
}

#[test]
fn w4_on_the_chat_and_responses_wires_effort_max_passes_unchanged() {
    let messages_fields = "\"output_config\":{\"effort\":\"max\"},\"chat_template_kwargs\":{\"reasoning_effort\":\"max\"}";
    rewrites(
        "/v1/chat/completions",
        &[(
            &format!("{{\"model\":\"code\",\"reasoning_effort\":\"max\",{messages_fields}}}"),
            &format!(
                "{{\"model\":\"served-code\",\"reasoning_effort\":\"max\",{messages_fields}}}"
            ),
        )],
    );
    rewrites(
        "/v1/responses",
        &[(
            &format!(
                "{{\"model\":\"code\",\"reasoning\":{{\"effort\":\"max\"}},{messages_fields}}}"
            ),
            &format!(
                "{{\"model\":\"served-code\",\"reasoning\":{{\"effort\":\"max\"}},{messages_fields}}}"
            ),
        )],
    );
}

// --- Claude Code on the messages wire -----------------------------------------------------------
//
// The fixtures under `tests/fixtures/claude-code/` are authored from Claude Code's published
// gateway protocol and from vLLM v0.27.1's source, not captured; each is headed by its sources
// and `tests/fixtures/claude-code/README.md` describes their format.

/// The name the pinned vLLM serves the model under, as its answers in the fixtures carry it.
const SERVED_QWEN: &str = "Qwen/Qwen3.8-27B-FP8";

const TOOL_TURN_REQUEST: &str = include_str!("fixtures/claude-code/1-tool-turn.request.txt");
const TOOL_TURN_RESPONSE: &str = include_str!("fixtures/claude-code/1-tool-turn.response.txt");
const TOOL_RESULT_REQUEST: &str = include_str!("fixtures/claude-code/2-tool-result.request.txt");
const TOOL_RESULT_RESPONSE: &str = include_str!("fixtures/claude-code/2-tool-result.response.txt");
const STRUCTURED_REQUEST: &str =
    include_str!("fixtures/claude-code/3-structured-output.request.txt");
const STRUCTURED_RESPONSE: &str =
    include_str!("fixtures/claude-code/3-structured-output.response.txt");

/// A fixture without its provenance header: every leading line that starts with `#`.
fn fixture(text: &'static str) -> &'static str {
    let mut rest = text;
    while rest.starts_with('#') {
        rest = rest.split_once('\n').map_or("", |(_, after)| after);
    }
    rest
}

/// A request fixture as Claude Code sends it.
struct ClaudeCodeRequest {
    path: &'static str,
    headers: Vec<&'static str>,
    body: &'static str,
}

impl ClaudeCodeRequest {
    fn parse(text: &'static str) -> Self {
        let (head, body) = fixture(text).split_once("\n\n").unwrap();
        let mut lines = head.lines();
        let path = lines
            .next()
            .and_then(|line| line.strip_prefix("POST "))
            .and_then(|line| line.strip_suffix(" HTTP/1.1"))
            .unwrap();
        Self {
            path,
            headers: lines.collect(),
            body,
        }
    }

    /// The request on the wire, with the owner's credential and the body's length added.
    fn raw(&self) -> Vec<u8> {
        let mut raw = format!(
            "POST {} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\n",
            self.path
        );
        for line in &self.headers {
            raw.push_str(line);
            raw.push_str("\r\n");
        }
        raw.push_str("content-length: ");
        raw.push_str(&self.body.len().to_string());
        raw.push_str("\r\n\r\n");
        let mut raw = raw.into_bytes();
        raw.extend_from_slice(self.body.as_bytes());
        raw
    }
}

/// The model Claude Code reaches: alias `code`, served as the pinned vLLM names it, with tool
/// calling parsed.
fn claude_code_model() -> RelayModel {
    RelayModel::new(label("code"), label(SERVED_QWEN), vec![Wire::Messages])
        .unwrap()
        .with_tool_calling(ToolCalling::Parsed)
}

/// Every header name of a request head a pod received, lowercase, in order.
fn header_names(raw: &[u8]) -> Vec<String> {
    let end = find(raw, b"\r\n\r\n").unwrap();
    String::from_utf8(raw[..end].to_vec())
        .unwrap()
        .split("\r\n")
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, _)| name.trim().to_ascii_lowercase())
        .collect()
}

/// Relays each request fixture to a pod answering the matching response fixture, and checks
/// the pod received the fixture's body with only `model` and `max_effort` rewritten, under the
/// gateway's own head, and the client received the response fixture byte for byte.
fn relays_claude_code(exchanges: &[(&'static str, &'static str, &'static str)]) {
    let pod = Pod::start(
        exchanges
            .iter()
            .map(|(_, response, content_type)| {
                let body = fixture(response);
                if *content_type == "text/event-stream" {
                    stream(&body.split_inclusive("\n\n").collect::<Vec<_>>())
                } else {
                    reply(200, content_type, body.as_bytes())
                }
            })
            .collect(),
    );
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![claude_code_model()], &source);
    let mut expected_bodies = Vec::new();
    for (request, response, content_type) in exchanges {
        let request = ClaudeCodeRequest::parse(request);
        assert!(request.path.ends_with("?beta=true"), "{}", request.path);
        for name in ["anthropic-version", "anthropic-beta"] {
            assert!(
                request
                    .headers
                    .iter()
                    .any(|line| line.starts_with(&format!("{name}: "))),
                "the fixture sends {name}"
            );
        }
        let answer = send(gateway.addr, &request.raw());
        assert_eq!(answer.status, 200, "{:?}", answer.code());
        assert_eq!(answer.header("content-type"), Some(*content_type));
        assert_eq!(
            answer.text(),
            fixture(response),
            "the client receives the target's answer byte for byte"
        );
        expected_bodies.push(
            request
                .body
                .replacen(
                    "\"model\": \"code\"",
                    &format!("\"model\": \"{SERVED_QWEN}\""),
                    1,
                )
                .replacen("\"effort\": \"max\"", "\"effort\": \"xhigh\"", 1),
        );
    }
    let seen = pod.seen();
    assert_eq!(seen.len(), exchanges.len());
    for (received, expected) in seen.iter().zip(&expected_bodies) {
        assert_eq!(received.method, "POST");
        assert_eq!(
            received.path, "/v1/messages",
            "the query is discarded and the target asked on the wire's path"
        );
        assert_eq!(&received.body_text(), expected);
        assert_eq!(
            header_names(&received.raw),
            ["host", "content-type", "content-length", "connection"],
            "no header of Claude Code's reaches the target: not anthropic-version, \
             anthropic-beta or x-claude-code-session-id"
        );
    }
    assert_eq!(source.acquired().len(), exchanges.len());
}

#[test]
fn claude_code_a_tool_turn_and_its_tool_result_reach_the_pod_mapped_and_stream_back_unchanged() {
    for request in [TOOL_TURN_REQUEST, TOOL_RESULT_REQUEST] {
        let body = ClaudeCodeRequest::parse(request).body;
        for part in [
            "\"thinking\": {\"type\": \"adaptive\"}",
            "\"output_config\": {\"effort\": \"max\"}",
            "\"cache_control\": {\"type\": \"ephemeral\"}",
            "\"tools\": [",
        ] {
            assert!(body.contains(part), "the fixture carries {part}");
        }
    }
    assert!(
        ClaudeCodeRequest::parse(TOOL_RESULT_REQUEST)
            .body
            .contains("\"type\": \"tool_result\"")
    );
    for part in [
        "\"type\":\"thinking\"",
        "\"type\":\"signature_delta\"",
        "\"type\":\"tool_use\"",
        "\"stop_reason\":\"tool_use\"",
    ] {
        assert!(
            fixture(TOOL_TURN_RESPONSE).contains(part),
            "the stream carries {part}"
        );
    }
    relays_claude_code(&[
        (TOOL_TURN_REQUEST, TOOL_TURN_RESPONSE, "text/event-stream"),
        (
            TOOL_RESULT_REQUEST,
            TOOL_RESULT_RESPONSE,
            "text/event-stream",
        ),
    ]);
}

#[test]
fn claude_code_adaptive_thinking_reaches_the_pod_unchanged() {
    let pod = Pod::start(vec![ok_json("{}")]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![claude_code_model()], &source);
    let body = "{\"model\":\"code\",\"thinking\":{\"type\":\"adaptive\"},\"messages\":[]}";
    assert_eq!(gateway.post_text("/v1/messages", body).status, 200);
    let bodies: Vec<String> = pod.seen().iter().map(Seen::body_text).collect();
    assert_eq!(
        bodies,
        [format!(
            "{{\"model\":\"{SERVED_QWEN}\",\"thinking\":{{\"type\":\"adaptive\"}},\"messages\":[]}}"
        )]
    );
}

#[test]
fn claude_code_structured_output_reaches_the_pod_with_its_format_unchanged() {
    let format = "\"format\": {\n      \"type\": \"json_schema\",\n      \"schema\": {";
    assert!(
        ClaudeCodeRequest::parse(STRUCTURED_REQUEST)
            .body
            .contains(format)
    );
    relays_claude_code(&[(STRUCTURED_REQUEST, STRUCTURED_RESPONSE, "application/json")]);
}

#[test]
fn claude_code_the_startup_probe_head_api_hello_is_refused_and_asks_for_no_target() {
    let pod = Pod::start(Vec::new());
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![claude_code_model()], &source);
    let head_of = |raw: &str| {
        let mut stream = TcpStream::connect(gateway.addr).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        let mut received = Vec::new();
        drop(stream.read_to_end(&mut received));
        let end = find(&received, b"\r\n\r\n").unwrap();
        assert_eq!(received.len(), end + 4, "an answer to HEAD has no body");
        String::from_utf8(received[..end].to_vec()).unwrap()
    };
    let anonymous = head_of(
        "HEAD /api/hello HTTP/1.1\r\nhost: gateway\r\nanthropic-version: 2023-06-01\r\n\r\n",
    );
    assert!(anonymous.starts_with("HTTP/1.1 401 "), "{anonymous}");
    assert_eq!(
        header_of(&anonymous, "www-authenticate").as_deref(),
        Some("Bearer")
    );
    let owner = head_of(&format!(
        "HEAD /api/hello HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\n\r\n"
    ));
    assert!(owner.starts_with("HTTP/1.1 404 "), "{owner}");
    // The bodies a GET would carry name the codes.
    let get = |bearer| send(gateway.addr, &request("GET", "/api/hello", bearer, b""));
    assert_eq!(
        get(None).refused(),
        (401, Some("credential-absent".to_string()))
    );
    assert_eq!(
        get(Some(OWNER)).refused(),
        (404, Some("path-unknown".to_string()))
    );
    assert!(source.acquired().is_empty());
    assert!(pod.seen().is_empty());
}

// --- A model's declared effort (story:effort-default) ----------------------------------------

/// Claude Code's first turn with its effort removed: `output_config` gone (`missing`), or left
/// as an empty object (`empty`).
fn tool_turn_without_effort() -> (ClaudeCodeRequest, ClaudeCodeRequest) {
    let named = "  \"output_config\": {\"effort\": \"max\"},\n";
    let request = ClaudeCodeRequest::parse(TOOL_TURN_REQUEST);
    assert!(request.body.contains(named), "the fixture names effort max");
    let missing = request.body.replacen(named, "", 1);
    let empty = request
        .body
        .replacen(named, "  \"output_config\": {},\n", 1);
    let leak = |body: String| -> &'static str { Box::leak(body.into_boxed_str()) };
    let with = |body| ClaudeCodeRequest {
        path: request.path,
        headers: request.headers.clone(),
        body: leak(body),
    };
    (with(missing), with(empty))
}

/// Criterion 2: on the messages wire, Claude Code's turn naming no effort, sent to a model that
/// declares one, reaches the pod with `output_config.effort` equal to it, once per declarable
/// effort: a missing `output_config` is added as the last top-level member, an empty one gains
/// the key, and every other byte is unchanged.
#[test]
fn effort_a_messages_request_naming_none_reaches_the_pod_with_the_models_declared_effort() {
    let (missing, empty) = tool_turn_without_effort();
    for effort in ReasoningEffort::ALL {
        let pod = Pod::start(vec![ok_json("{}"), ok_json("{}")]);
        let source = Source::of(&[&pod]);
        let gateway = Relayed::start(
            vec![claude_code_model().with_reasoning_effort(effort)],
            &source,
        );
        for request in [&missing, &empty] {
            let answer = send(gateway.addr, &request.raw());
            assert_eq!(answer.status, 200, "{effort:?}: {:?}", answer.code());
        }
        let served = |body: &str| {
            body.replacen(
                "\"model\": \"code\"",
                &format!("\"model\": \"{SERVED_QWEN}\""),
                1,
            )
        };
        let value = effort.as_str();
        let added = served(missing.body).replacen(
            "\n  ]\n}\n",
            &format!("\n  ],\"output_config\":{{\"effort\":\"{value}\"}}\n}}\n"),
            1,
        );
        let gained = served(empty.body).replacen(
            "\"output_config\": {}",
            &format!("\"output_config\": {{\"effort\":\"{value}\"}}"),
            1,
        );
        assert_ne!(added, served(missing.body));
        let bodies: Vec<String> = pod.seen().iter().map(Seen::body_text).collect();
        assert_eq!(bodies, [added, gained], "{effort:?}");
    }
}

/// Criterion 3: a request that names an effort in either key, whatever its value, keeps the W4
/// mapping and never the declared effort. A top-level `output_config` that is not an object
/// gains nothing; one below the top level names no effort, so the top level gains one; a
/// top-level `output_config` named twice gains the key in the last.
#[test]
fn effort_a_declared_effort_never_replaces_a_named_one() {
    let pod = Pod::start((0..9).map(|_| ok_json("{}")).collect());
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(
        vec![code_model().with_reasoning_effort(ReasoningEffort::Low)],
        &source,
    );
    let cases = [
        (
            r#"{"model":"code","output_config":{"effort":"max"}}"#,
            r#"{"model":"served-code","output_config":{"effort":"xhigh"}}"#,
        ),
        (
            r#"{"model":"code","output_config":{"effort":"medium"}}"#,
            r#"{"model":"served-code","output_config":{"effort":"medium"}}"#,
        ),
        (
            r#"{"model":"code","chat_template_kwargs":{"reasoning_effort":"high"}}"#,
            r#"{"model":"served-code","chat_template_kwargs":{"reasoning_effort":"xhigh"}}"#,
        ),
        (
            r#"{"model":"code","chat_template_kwargs":{"reasoning_effort":"xhigh"}}"#,
            r#"{"model":"served-code","chat_template_kwargs":{"reasoning_effort":"xhigh"}}"#,
        ),
        (
            r#"{"model":"code","output_config":{"effort":null}}"#,
            r#"{"model":"served-code","output_config":{"effort":null}}"#,
        ),
        (
            r#"{"model":"code","output_config":"none"}"#,
            r#"{"model":"served-code","output_config":"none"}"#,
        ),
        (
            r#"{"model":"code","x":{"output_config":{"effort":"high"}}}"#,
            r#"{"model":"served-code","x":{"output_config":{"effort":"high"}},"output_config":{"effort":"low"}}"#,
        ),
        (
            r#"{"output_config":{"a":1},"model":"code","output_config":{"b":2}}"#,
            r#"{"output_config":{"a":1},"model":"served-code","output_config":{"b":2,"effort":"low"}}"#,
        ),
        (
            "{ \"model\" : \"code\" , \"output_config\" : { } }",
            "{ \"model\" : \"served-code\" , \"output_config\" : { \"effort\":\"low\"} }",
        ),
    ];
    for (sent, _) in &cases {
        assert_eq!(
            gateway.post_text("/v1/messages", sent).status,
            200,
            "{sent}"
        );
    }
    let seen: Vec<String> = pod.seen().iter().map(Seen::body_text).collect();
    let expected: Vec<&str> = cases.iter().map(|(_, rewritten)| *rewritten).collect();
    assert_eq!(seen, expected);
}

/// Criterion 3: a model that declares no effort relays Claude Code's turn naming none with
/// only `model` rewritten.
#[test]
fn effort_a_model_declaring_none_relays_a_request_naming_none_unchanged() {
    let (missing, _) = tool_turn_without_effort();
    let pod = Pod::start(vec![ok_json("{}")]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![claude_code_model()], &source);
    assert_eq!(send(gateway.addr, &missing.raw()).status, 200);
    let bodies: Vec<String> = pod.seen().iter().map(Seen::body_text).collect();
    assert_eq!(
        bodies,
        [missing.body.replacen(
            "\"model\": \"code\"",
            &format!("\"model\": \"{SERVED_QWEN}\""),
            1
        )]
    );
}

/// Criterion 3: on the chat and responses wires a declared effort is never given.
#[test]
fn effort_on_the_chat_and_responses_wires_a_declared_effort_is_not_given() {
    let pod = Pod::start(vec![ok_json("{}"), ok_json("{}")]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(
        vec![code_model().with_reasoning_effort(ReasoningEffort::Medium)],
        &source,
    );
    let chat = gateway.post_text("/v1/chat/completions", r#"{"model":"code","messages":[]}"#);
    let responses = gateway.post_text("/v1/responses", r#"{"model":"code","input":"hi"}"#);
    assert_eq!((chat.status, responses.status), (200, 200));
    let bodies: Vec<String> = pod.seen().iter().map(Seen::body_text).collect();
    assert_eq!(
        bodies,
        [
            r#"{"model":"served-code","messages":[]}"#,
            r#"{"model":"served-code","input":"hi"}"#
        ]
    );
}

// --- The output clamp (story:max-tokens-clamp) -------------------------------------------------
//
// The count's answer is `tests/fixtures/claude-code/4-count-tokens.response.txt`, authored from
// vLLM v0.27.1's `/v1/messages/count_tokens` route.

const COUNT_TOKENS: &str = include_str!("fixtures/claude-code/4-count-tokens.response.txt");
/// The window the consumer's vLLM ran with (`--max-model-len 65536`).
const WINDOW: u64 = 65_536;
const CONTEXT_EXCEEDED: &str = "{\"error\":{\"code\":\"context-exceeded\",\"message\":\"prompt is too long: the input leaves no room for output in the model context window\"}}";

/// The count's answer, as vLLM writes it.
fn count_answer() -> Answer {
    ok_json(fixture(COUNT_TOKENS).trim_end())
}

/// The count the fixture answers.
fn counted() -> u64 {
    fixture(COUNT_TOKENS)
        .trim_end()
        .strip_prefix("{\"input_tokens\":")
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap()
        .parse()
        .unwrap()
}

/// Claude Code's tool-result turn asking for `max_tokens`.
fn tool_result_asking(max_tokens: u64) -> ClaudeCodeRequest {
    let request = ClaudeCodeRequest::parse(TOOL_RESULT_REQUEST);
    let named = "\"max_tokens\": 32000,";
    assert!(request.body.contains(named), "the fixture asks for 32000");
    let body = request
        .body
        .replacen(named, &format!("\"max_tokens\": {max_tokens},"), 1);
    ClaudeCodeRequest {
        path: request.path,
        headers: request.headers,
        body: Box::leak(body.into_boxed_str()),
    }
}

/// A Claude Code body as the relay rewrites it: the model's served name, effort `max` as
/// `xhigh`, and `max_tokens` set to `max_tokens`.
fn served_tool_result(body: &str, max_tokens: u64) -> String {
    let body = body
        .replacen(
            "\"model\": \"code\"",
            &format!("\"model\": \"{SERVED_QWEN}\""),
            1,
        )
        .replacen("\"effort\": \"max\"", "\"effort\": \"xhigh\"", 1);
    let at = body.find("\"max_tokens\": ").unwrap() + "\"max_tokens\": ".len();
    let end = at + body[at..].find(',').unwrap();
    format!("{}{max_tokens}{}", &body[..at], &body[end..])
}

/// A slot carrying the model's vLLM key.
fn keyed(pod: &Pod) -> Slot {
    let mut slot = listening(pod);
    slot.bearer = Some(Arc::new(
        TargetBearer::new(VLLM_KEY.as_bytes().to_vec()).unwrap(),
    ));
    slot
}

/// Criterion 2: before relaying, the gateway asks the same target for the count, with the body
/// it is about to relay, under its own head and the target's credential. A `max_tokens` one
/// above the room `w - n` the window leaves is lowered to it; one at it and one below it reach
/// the pod unchanged. The fixture's own 32000 is the case above.
#[test]
fn clamp_a_max_tokens_above_the_room_left_is_lowered_to_it_and_one_at_or_below_is_unchanged() {
    let room = WINDOW - counted();
    assert_eq!(
        room + 1,
        32_000,
        "the fixture's count leaves one token fewer than it asks"
    );
    for (asked, relayed) in [(room - 1, room - 1), (room, room), (room + 1, room)] {
        let pod = Pod::start(vec![count_answer(), ok_json("{}")]);
        let source = Source::new(vec![keyed(&pod)]);
        let gateway = Relayed::start(
            vec![claude_code_model().with_max_model_len(WINDOW)],
            &source,
        );
        let request = tool_result_asking(asked);
        let answer = send(gateway.addr, &request.raw());
        assert_eq!(answer.status, 200, "{asked}: {:?}", answer.code());
        let seen = pod.seen();
        assert_eq!(seen.len(), 2, "{asked}: a count, then the request");
        let (count, relay) = (&seen[0], &seen[1]);
        assert_eq!(
            (count.method.as_str(), count.path.as_str()),
            ("POST", "/v1/messages/count_tokens")
        );
        assert_eq!(count.body_text(), served_tool_result(request.body, asked));
        assert_eq!(
            (relay.method.as_str(), relay.path.as_str()),
            ("POST", "/v1/messages")
        );
        assert_eq!(
            relay.body_text(),
            served_tool_result(request.body, relayed),
            "{asked}"
        );
        for received in [count, relay] {
            assert_eq!(
                received.authorization.as_deref(),
                Some(format!("Bearer {VLLM_KEY}").as_str())
            );
            assert_eq!(
                header_names(&received.raw),
                [
                    "host",
                    "authorization",
                    "content-type",
                    "content-length",
                    "connection"
                ],
                "the count's head is the relay's own: no header of Claude Code's"
            );
            assert!(find(&received.raw, OWNER.as_bytes()).is_none());
        }
        assert_eq!(
            source.acquired().len(),
            1,
            "one target serves the count and the relay"
        );
        assert!(source.invalidated().is_empty());
    }
}

/// Criterion 3: an input that leaves no room (`w - n < 1`) is `context-exceeded` (400) and is
/// never relayed; its message carries the phrase Claude Code compacts on. One token of room is
/// relayed, asking for that one token.
#[test]
fn clamp_a_prompt_that_leaves_no_room_is_context_exceeded_and_never_relayed() {
    let counted = counted();
    for window in [counted - 1, counted] {
        let pod = Pod::start(vec![count_answer(), ok_json("{}")]);
        let source = Source::of(&[&pod]);
        let gateway = Relayed::start(
            vec![claude_code_model().with_max_model_len(window)],
            &source,
        );
        let answer = send(gateway.addr, &tool_result_asking(32_000).raw());
        assert_eq!(
            answer.refused(),
            (400, Some("context-exceeded".to_string())),
            "{window}"
        );
        assert_eq!(answer.text(), CONTEXT_EXCEEDED);
        assert!(answer.text().contains("prompt is too long"));
        let paths: Vec<String> = pod.seen().iter().map(|seen| seen.path.clone()).collect();
        assert_eq!(paths, ["/v1/messages/count_tokens"], "{window}");
        assert!(source.invalidated().is_empty());
        assert!(source.released_reaches(1), "the target is released");
    }
    let pod = Pod::start(vec![count_answer(), ok_json("{}")]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(
        vec![claude_code_model().with_max_model_len(counted + 1)],
        &source,
    );
    let request = tool_result_asking(32_000);
    assert_eq!(send(gateway.addr, &request.raw()).status, 200);
    let seen = pod.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1].body_text(), served_tool_result(request.body, 1));
}

/// Criterion 4: a count that fails relays the request unchanged, reports no target failed, and
/// is answered as before: a refused connection, one closed without an answer, a status outside
/// 2xx (a 503 included, which a relay would report), and an answer without one non-negative
/// integer `input_tokens`, or longer than the head bound.
#[test]
fn clamp_a_count_that_fails_relays_the_request_unchanged() {
    let padded = format!("{{\"input_tokens\":10,\"pad\":\"{}\"}}", "a".repeat(9_000));
    let failures: Vec<(&str, Option<Answer>)> = vec![
        ("refused", None),
        ("closed", Some(Answer::Close)),
        (
            "500",
            Some(reply(500, "application/json", b"{\"input_tokens\":10}")),
        ),
        (
            "503",
            Some(reply(503, "application/json", b"{\"input_tokens\":10}")),
        ),
        ("absent", Some(ok_json("{\"tokens\":10}"))),
        ("negative", Some(ok_json("{\"input_tokens\":-1}"))),
        ("fraction", Some(ok_json("{\"input_tokens\":10.0}"))),
        ("exponent", Some(ok_json("{\"input_tokens\":1e1}"))),
        ("string", Some(ok_json("{\"input_tokens\":\"10\"}"))),
        ("null", Some(ok_json("{\"input_tokens\":null}"))),
        (
            "twice",
            Some(ok_json("{\"input_tokens\":10,\"input_tokens\":10}")),
        ),
        ("nested", Some(ok_json("{\"usage\":{\"input_tokens\":10}}"))),
        ("array", Some(ok_json("[{\"input_tokens\":10}]"))),
        ("not json", Some(ok_json("input_tokens: 10"))),
        ("over the head bound", Some(ok_json(&padded))),
    ];
    let body = "{\"model\":\"code\",\"max_tokens\":32000,\"messages\":[]}";
    let relayed = "{\"model\":\"served-code\",\"max_tokens\":32000,\"messages\":[]}";
    for (name, count) in failures {
        let refused = count.is_none();
        let mut script: Vec<Answer> = count.into_iter().collect();
        script.push(ok_json("{}"));
        let pod = Pod::start(script);
        let slot = listening(&pod);
        if refused {
            slot.refuse_first.store(1, Ordering::SeqCst);
        }
        let source = Source::new(vec![slot]);
        // Counted at 10, the room would be 90.
        let gateway = Relayed::start(vec![code_model().with_max_model_len(100)], &source);
        let answer = gateway.post_text("/v1/messages", body);
        assert_eq!(answer.status, 200, "{name}: {:?}", answer.code());
        assert_eq!(answer.text(), "{}", "{name}");
        let seen = pod.seen();
        let last = seen.last().unwrap();
        assert_eq!(last.path, "/v1/messages", "{name}");
        assert_eq!(last.body_text(), relayed, "{name}: relayed unchanged");
        assert_eq!(seen.len(), if refused { 1 } else { 2 }, "{name}");
        assert!(
            source.invalidated().is_empty(),
            "{name}: no target is reported failed"
        );
        assert_eq!(source.acquired().len(), 1, "{name}");
    }
}

/// Criteria 1 and 5: no count is asked for a model without a window, for a body without one
/// top-level integer `max_tokens`, or on the chat and responses wires; each is relayed with
/// only its `model` rewritten.
#[test]
fn clamp_no_count_is_asked_without_a_window_a_top_level_integer_max_tokens_or_the_messages_wire() {
    let windowless = RelayModel::new(label("open"), label("served-open"), all_wires()).unwrap();
    let cases = [
        (
            "/v1/messages",
            "{\"model\":\"open\",\"max_tokens\":32000,\"messages\":[]}",
        ),
        ("/v1/messages", "{\"model\":\"code\",\"messages\":[]}"),
        (
            "/v1/messages",
            "{\"model\":\"code\",\"max_tokens\":1.5,\"messages\":[]}",
        ),
        (
            "/v1/messages",
            "{\"model\":\"code\",\"max_tokens\":1e3,\"messages\":[]}",
        ),
        (
            "/v1/messages",
            "{\"model\":\"code\",\"max_tokens\":-1,\"messages\":[]}",
        ),
        (
            "/v1/messages",
            "{\"model\":\"code\",\"max_tokens\":\"500\",\"messages\":[]}",
        ),
        (
            "/v1/messages",
            "{\"model\":\"code\",\"max_tokens\":null,\"messages\":[]}",
        ),
        (
            "/v1/messages",
            "{\"model\":\"code\",\"x\":{\"max_tokens\":500},\"messages\":[]}",
        ),
        (
            "/v1/messages",
            "{\"model\":\"code\",\"max_tokens\":500,\"max_tokens\":500}",
        ),
        (
            "/v1/chat/completions",
            "{\"model\":\"code\",\"max_tokens\":500,\"messages\":[]}",
        ),
        (
            "/v1/responses",
            "{\"model\":\"code\",\"max_tokens\":500,\"input\":\"hi\"}",
        ),
    ];
    let pod = Pod::start(cases.iter().map(|_| ok_json("{}")).collect());
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(
        vec![code_model().with_max_model_len(100), windowless],
        &source,
    );
    for (path, body) in cases {
        assert_eq!(gateway.post_text(path, body).status, 200, "{path} {body}");
    }
    let seen: Vec<(String, String)> = pod
        .seen()
        .iter()
        .map(|seen| (seen.path.clone(), seen.body_text()))
        .collect();
    let expected: Vec<(String, String)> = cases
        .iter()
        .map(|(path, body)| {
            let served = body
                .replacen("\"model\":\"code\"", "\"model\":\"served-code\"", 1)
                .replacen("\"model\":\"open\"", "\"model\":\"served-open\"", 1);
            ((*path).to_owned(), served)
        })
        .collect();
    assert_eq!(seen, expected);
}

/// Criterion 1: the window is the model's `max_model_len` when it declares one, else its
/// `context_window`: a model declaring both is clamped to the first, one declaring only a
/// context window to it.
#[test]
fn clamp_the_window_is_the_max_model_len_else_the_context_window() {
    let pod = Pod::start(vec![
        ok_json("{\"input_tokens\":900}"),
        ok_json("{}"),
        ok_json("{\"input_tokens\":900}"),
        ok_json("{}"),
    ]);
    let source = Source::of(&[&pod]);
    let both = code_model()
        .with_max_model_len(1_000)
        .with_context_window(65_536);
    let window = RelayModel::new(label("window"), label("served-window"), all_wires())
        .unwrap()
        .with_context_window(1_000);
    let gateway = Relayed::start(vec![both, window], &source);
    for alias in ["code", "window"] {
        let body = format!("{{\"model\":\"{alias}\",\"max_tokens\":32000,\"messages\":[]}}");
        assert_eq!(
            gateway.post_text("/v1/messages", &body).status,
            200,
            "{alias}"
        );
    }
    let relayed: Vec<String> = pod
        .seen()
        .iter()
        .filter(|seen| seen.path == "/v1/messages")
        .map(Seen::body_text)
        .collect();
    assert_eq!(
        relayed,
        [
            "{\"model\":\"served-code\",\"max_tokens\":100,\"messages\":[]}",
            "{\"model\":\"served-window\",\"max_tokens\":100,\"messages\":[]}",
        ]
    );
}

/// Criterion 4: the count runs only on a target `acquire` handed out, so a request refused at
/// `acquire` asks no pod anything.
#[test]
fn clamp_a_request_acquire_refuses_asks_no_pod_for_a_count() {
    let pod = Pod::start(vec![count_answer(), ok_json("{}")]);
    let cold = Source::cold(vec![listening(&pod)], 1);
    let gateway = Relayed::start(vec![claude_code_model().with_max_model_len(WINDOW)], &cold);
    let answer = send(gateway.addr, &tool_result_asking(32_000).raw());
    assert_eq!(
        answer.refused(),
        (503, Some("model-cold-start".to_string()))
    );
    let none = Source::new(Vec::new());
    let gateway = Relayed::start(vec![claude_code_model().with_max_model_len(WINDOW)], &none);
    let answer = send(gateway.addr, &tool_result_asking(32_000).raw());
    assert_eq!(
        answer.refused(),
        (503, Some("target-unavailable".to_string()))
    );
    assert!(pod.seen().is_empty());
}

// --- Adversarial: a declared effort's insertion on bodies the unit's cases do not reach --------

/// Insertion against escaped key spellings, whitespace before a closing brace, nested objects
/// as the last member of `output_config`, non-object `output_config` and `chat_template_kwargs`
/// values, braces inside strings, and a duplicate `output_config` whose first names the effort.
/// Each expected body is written by hand from `docs/gateway.md` "The relay", step 6.
#[test]
fn effort_adv_insertion_keeps_every_body_valid_and_follows_step_6() {
    let cases: &[(&str, &str)] = &[
        (
            r#"{"model":"code","output_config":{}}"#,
            r#"{"model":"served-code","output_config":{"effort":"low"}}"#,
        ),
        (
            r#"{"model":"code","output_config":{"effort":"high"}}"#,
            r#"{"model":"served-code","output_config":{"effort":"xhigh"}}"#,
        ),
        (
            r#"{"model":"code","chat_template_kwargs":{"reasoning_effort":"medium"}}"#,
            r#"{"model":"served-code","chat_template_kwargs":{"reasoning_effort":"medium"}}"#,
        ),
        (
            "{\"model\":\"code\",\"output_config\":{\"a\":1 \n\t}\r\n}\n",
            "{\"model\":\"served-code\",\"output_config\":{\"a\":1,\"effort\":\"low\" \n\t}\r\n}\n",
        ),
        (
            "{\"model\":\"code\"  \n}  \n",
            "{\"model\":\"served-code\",\"output_config\":{\"effort\":\"low\"}  \n}  \n",
        ),
        (
            r#"{"model":"code","output_config":{"format":{"schema":{"x":[{}]}}}}"#,
            r#"{"model":"served-code","output_config":{"format":{"schema":{"x":[{}]}},"effort":"low"}}"#,
        ),
        (
            r#"{"model":"code","output_config":{"x":{"effort":"high"}}}"#,
            r#"{"model":"served-code","output_config":{"x":{"effort":"high"},"effort":"low"}}"#,
        ),
        (
            r#"{"model":"code","output_config":{"s":"}\"{"}}"#,
            r#"{"model":"served-code","output_config":{"s":"}\"{","effort":"low"}}"#,
        ),
        (
            r#"{"model":"code","output_config":{"ü":"é"}}"#,
            r#"{"model":"served-code","output_config":{"ü":"é","effort":"low"}}"#,
        ),
        (
            r#"{"model":"code","output_config":[]}"#,
            r#"{"model":"served-code","output_config":[]}"#,
        ),
        (
            r#"{"model":"code","output_config":null}"#,
            r#"{"model":"served-code","output_config":null}"#,
        ),
        (
            r#"{"model":"code","output_config":0}"#,
            r#"{"model":"served-code","output_config":0}"#,
        ),
        (
            r#"{"model":"code","output_config":true}"#,
            r#"{"model":"served-code","output_config":true}"#,
        ),
        (
            r#"{"model":"code","chat_template_kwargs":"reasoning_effort"}"#,
            r#"{"model":"served-code","chat_template_kwargs":"reasoning_effort","output_config":{"effort":"low"}}"#,
        ),
        (
            r#"{"model":"code","chat_template_kwargs":null}"#,
            r#"{"model":"served-code","chat_template_kwargs":null,"output_config":{"effort":"low"}}"#,
        ),
        (
            r#"{"model":"code","chat_template_kwargs":[{"reasoning_effort":"high"}]}"#,
            r#"{"model":"served-code","chat_template_kwargs":[{"reasoning_effort":"high"}],"output_config":{"effort":"low"}}"#,
        ),
        (
            r#"{"model":"code","output_config":{"effort":"medium"},"output_config":{}}"#,
            r#"{"model":"served-code","output_config":{"effort":"medium"},"output_config":{}}"#,
        ),
    ];
    let pod = Pod::start(cases.iter().map(|_| ok_json("{}")).collect());
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(
        vec![code_model().with_reasoning_effort(ReasoningEffort::Low)],
        &source,
    );
    for (sent, _) in cases {
        let answer = gateway.post_text("/v1/messages", sent);
        assert_eq!(answer.status, 200, "{sent}: {:?}", answer.code());
    }
    let seen = pod.seen();
    let bodies: Vec<String> = seen.iter().map(Seen::body_text).collect();
    let expected: Vec<&str> = cases.iter().map(|(_, rewritten)| *rewritten).collect();
    assert_eq!(bodies, expected);
    for request in &seen {
        assert_eq!(
            request.content_length.as_deref(),
            Some(request.body.len().to_string().as_str())
        );
    }
}

/// A messages body of exactly 32 MiB naming no effort is relayed whole with the declared effort
/// added past the bound, and the content-length the target is sent counts the added member.
#[test]
fn effort_adv_a_32_mib_body_naming_none_grows_past_the_bound_with_a_matching_length() {
    let pod = Pod::start(vec![ok_json("{}")]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(
        vec![code_model().with_reasoning_effort(ReasoningEffort::Low)],
        &source,
    );
    let body = pad_to("{\"model\":\"code\"", BODY_BOUND);
    let answer = gateway.post("/v1/messages", &body);
    assert_eq!(answer.status, 200, "{:?}", answer.code());
    let member = b",\"output_config\":{\"effort\":\"low\"}";
    let mut expected = pad_to("{\"model\":\"served-code\"", BODY_BOUND + 7);
    let close = expected.pop();
    assert_eq!(close, Some(b'}'));
    expected.extend_from_slice(member);
    expected.push(b'}');
    let seen = pod.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].content_length.as_deref(),
        Some((BODY_BOUND + 7 + member.len()).to_string().as_str())
    );
    assert!(
        seen[0].body.as_slice().eq(expected.as_slice()),
        "the 32 MiB body changed beyond its model and the added effort"
    );
}

/// A chunked messages body naming no effort reaches the target decoded, with a length counting
/// the added effort and no transfer-encoding.
#[test]
fn effort_adv_a_chunked_body_naming_none_gains_the_effort_and_a_matching_length() {
    let pod = Pod::start(vec![ok_json("{}")]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(
        vec![code_model().with_reasoning_effort(ReasoningEffort::Xhigh)],
        &source,
    );
    let body = b"{\"model\":\"code\",\"output_config\":{\"format\":{}} }";
    let (first, second) = body.split_at(30);
    let mut raw = format!(
        "POST /v1/messages HTTP/1.1\r\nauthorization: Bearer {OWNER}\r\ntransfer-encoding: chunked\r\n\r\n"
    )
    .into_bytes();
    for part in [first, second] {
        raw.extend_from_slice(format!("{:x}\r\n", part.len()).as_bytes());
        raw.extend_from_slice(part);
        raw.extend_from_slice(b"\r\n");
    }
    raw.extend_from_slice(b"0\r\n\r\n");
    let answer = send(gateway.addr, &raw);
    assert_eq!(answer.status, 200, "{:?}", answer.code());
    let expected =
        "{\"model\":\"served-code\",\"output_config\":{\"format\":{},\"effort\":\"xhigh\"} }";
    let seen = pod.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].transfer_encoding, None);
    assert_eq!(
        seen[0].content_length.as_deref(),
        Some(expected.len().to_string().as_str())
    );
    assert_eq!(seen[0].body_text(), expected);
}

/// The chat and responses wires stay byte-identical for a 32 MiB body naming no effort sent to
/// a model that declares one.
#[test]
fn effort_adv_chat_and_responses_bodies_at_the_bound_gain_no_effort() {
    let pod = Pod::start(vec![ok_json("{}"), ok_json("{}")]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(
        vec![code_model().with_reasoning_effort(ReasoningEffort::Medium)],
        &source,
    );
    let body = pad_to("{\"model\":\"code\"", BODY_BOUND);
    for path in ["/v1/chat/completions", "/v1/responses"] {
        assert_eq!(gateway.post(path, &body).status, 200, "{path}");
    }
    let expected = pad_to("{\"model\":\"served-code\"", BODY_BOUND + 7);
    for seen in pod.seen() {
        assert!(
            seen.body.as_slice().eq(expected.as_slice()),
            "{}",
            seen.path
        );
    }
}

// --- Adversarial cases for the output clamp (story:max-tokens-clamp) ----------------------------
//
// Each drives the relay from what vLLM v0.27.1 sends and accepts, read from its source
// (`vllm/entrypoints/anthropic/{api_router,protocol,serving}.py`, tag v0.27.1).

/// A relayed request's body and the `content-length` it was sent with agree.
fn length_matches(seen: &Seen) {
    assert_eq!(
        seen.content_length.as_deref(),
        Some(seen.body.len().to_string().as_str()),
        "{}",
        seen.path
    );
}

/// vLLM v0.27.1 answers the count with `model_dump(exclude_none=True)` of
/// `AnthropicCountTokensResponse`, which carries `context_management` beside `input_tokens`
/// (`serving.py:1020-1025`, `api_router.py:133`); the fixture's compact `{"input_tokens":n}` is
/// not what a pod writes. The real shape must decide the clamp just the same.
#[test]
fn adversary_mtc_the_count_vllm_really_writes_with_context_management_is_read() {
    let pod = Pod::start(vec![
        ok_json("{\"input_tokens\":900,\"context_management\":{\"original_input_tokens\":900}}"),
        ok_json("{}"),
    ]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![code_model().with_max_model_len(1_000)], &source);
    let answer = gateway.post_text(
        "/v1/messages",
        "{\"model\":\"code\",\"max_tokens\":32000,\"messages\":[]}",
    );
    assert_eq!(answer.status, 200, "{:?}", answer.code());
    let seen = pod.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen[1].body_text(),
        "{\"model\":\"served-code\",\"max_tokens\":100,\"messages\":[]}"
    );
    length_matches(&seen[1]);
}

/// A key spelled with an escape is the same key to vLLM's JSON parser, so
/// `"max_tokens"` is the request's `max_tokens` and is lowered in place, its spelling and
/// every other byte kept, and the `content-length` follows.
#[test]
fn adversary_mtc_an_escaped_max_tokens_key_is_counted_and_lowered_in_place() {
    let pod = Pod::start(vec![ok_json("{\"input_tokens\":900}"), ok_json("{}")]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![code_model().with_max_model_len(1_000)], &source);
    let answer = gateway.post_text(
        "/v1/messages",
        "{\"model\":\"code\", \"max\\u005ftokens\" :\n 32000 \n,\"messages\":[]}",
    );
    assert_eq!(answer.status, 200, "{:?}", answer.code());
    let seen = pod.seen();
    assert_eq!(seen.len(), 2, "a count, then the request");
    assert_eq!(seen[0].path, "/v1/messages/count_tokens");
    assert_eq!(
        seen[1].body_text(),
        "{\"model\":\"served-code\", \"max\\u005ftokens\" :\n 100 \n,\"messages\":[]}"
    );
    length_matches(&seen[0]);
    length_matches(&seen[1]);
}

/// Both rewrites in one body: the declared effort inserted before, after or beside
/// `max_tokens`, effort `max` mapped to `xhigh`, and the clamp, each byte-exact. The count is
/// asked with the body as step 6 wrote it, before the clamp.
#[test]
fn adversary_mtc_the_declared_effort_and_the_clamp_rewrite_one_body_byte_exact() {
    let cases = [
        (
            r#"{"output_config":{},"max_tokens":32000,"model":"code","messages":[]}"#,
            r#"{"output_config":{"effort":"low"},"max_tokens":32000,"model":"served-code","messages":[]}"#,
            r#"{"output_config":{"effort":"low"},"max_tokens":100,"model":"served-code","messages":[]}"#,
        ),
        (
            r#"{"model":"code","messages":[],"max_tokens":32000}"#,
            r#"{"model":"served-code","messages":[],"max_tokens":32000,"output_config":{"effort":"low"}}"#,
            r#"{"model":"served-code","messages":[],"max_tokens":100,"output_config":{"effort":"low"}}"#,
        ),
        (
            r#"{"output_config":{"format":{"type":"json_schema"}},"model":"code","max_tokens":32000 }"#,
            r#"{"output_config":{"format":{"type":"json_schema"},"effort":"low"},"model":"served-code","max_tokens":32000 }"#,
            r#"{"output_config":{"format":{"type":"json_schema"},"effort":"low"},"model":"served-code","max_tokens":100 }"#,
        ),
        (
            r#"{"output_config":{"effort":"max"},"chat_template_kwargs":{"reasoning_effort":"max"},"max_tokens":32000,"model":"code"}"#,
            r#"{"output_config":{"effort":"xhigh"},"chat_template_kwargs":{"reasoning_effort":"xhigh"},"max_tokens":32000,"model":"served-code"}"#,
            r#"{"output_config":{"effort":"xhigh"},"chat_template_kwargs":{"reasoning_effort":"xhigh"},"max_tokens":100,"model":"served-code"}"#,
        ),
    ];
    let pod = Pod::start(
        cases
            .iter()
            .flat_map(|_| [ok_json("{\"input_tokens\":900}"), ok_json("{}")])
            .collect(),
    );
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(
        vec![
            code_model()
                .with_max_model_len(1_000)
                .with_reasoning_effort(ReasoningEffort::Low),
        ],
        &source,
    );
    for (sent, _, _) in &cases {
        assert_eq!(
            gateway.post_text("/v1/messages", sent).status,
            200,
            "{sent}"
        );
    }
    let seen = pod.seen();
    assert_eq!(seen.len(), cases.len() * 2);
    for (index, (sent, counted, relayed)) in cases.iter().enumerate() {
        let (count, relay) = (&seen[index * 2], &seen[index * 2 + 1]);
        assert_eq!(count.path, "/v1/messages/count_tokens", "{sent}");
        assert_eq!(count.body_text(), *counted, "{sent}");
        assert_eq!(relay.path, "/v1/messages", "{sent}");
        assert_eq!(relay.body_text(), *relayed, "{sent}");
        length_matches(count);
        length_matches(relay);
    }
}

/// A count answered chunked, in pieces that split the number, or framed by the pod closing
/// the connection, decides the clamp as one framed by `content-length` does.
#[test]
fn adversary_mtc_a_chunked_or_close_framed_count_is_read_whole() {
    let framed = |framing| Answer::Reply {
        status: 200,
        content_type: "application/json",
        chunks: vec![
            b"{\"input_".to_vec(),
            b"tokens\":9".to_vec(),
            b"00}".to_vec(),
        ],
        framing,
        hold_head: None,
        hold_chunks: None,
    };
    let pod = Pod::start(vec![
        framed(Framing::Chunked),
        ok_json("{}"),
        framed(Framing::Close),
        ok_json("{}"),
    ]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![code_model().with_max_model_len(1_000)], &source);
    for _ in 0..2 {
        let answer = gateway.post_text(
            "/v1/messages",
            "{\"model\":\"code\",\"max_tokens\":32000,\"messages\":[]}",
        );
        assert_eq!(answer.status, 200, "{:?}", answer.code());
    }
    let relayed: Vec<String> = pod
        .seen()
        .iter()
        .filter(|seen| seen.path == "/v1/messages")
        .map(Seen::body_text)
        .collect();
    assert_eq!(
        relayed,
        [
            "{\"model\":\"served-code\",\"max_tokens\":100,\"messages\":[]}",
            "{\"model\":\"served-code\",\"max_tokens\":100,\"messages\":[]}",
        ]
    );
}

/// A streamed request is counted (vLLM's count ignores `stream`) and lowered, and its answer
/// still streams to the client chunk by chunk.
#[test]
fn adversary_mtc_a_streamed_request_is_counted_lowered_and_still_streamed() {
    let pod = Pod::start(vec![
        ok_json("{\"input_tokens\":900}"),
        stream(&["event: message_start\ndata: {}\n\n", "data: [DONE]\n\n"]),
    ]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![code_model().with_max_model_len(1_000)], &source);
    let answer = gateway.post_text(
        "/v1/messages",
        "{\"model\":\"code\",\"stream\":true,\"max_tokens\":32000,\"messages\":[]}",
    );
    assert_eq!(answer.status, 200, "{:?}", answer.code());
    assert_eq!(answer.header("content-type"), Some("text/event-stream"));
    assert_eq!(
        answer.text(),
        "event: message_start\ndata: {}\n\ndata: [DONE]\n\n"
    );
    let seen = pod.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen[1].body_text(),
        "{\"model\":\"served-code\",\"stream\":true,\"max_tokens\":100,\"messages\":[]}"
    );
}

/// The count of an `api-key` endpoint carries its key in the endpoint's own header and no
/// `authorization`, exactly as the relayed request does, and never the owner's credential.
#[test]
fn adversary_mtc_the_count_of_a_credential_header_endpoint_carries_its_key_there() {
    let pod = Pod::start(vec![ok_json("{\"input_tokens\":900}"), ok_json("{}")]);
    let mut slot = listening(&pod);
    slot.bearer = Some(Arc::new(
        TargetBearer::new(VLLM_KEY.as_bytes().to_vec()).unwrap(),
    ));
    slot.header = Some(CredentialHeader::new("X-Api-Key").unwrap());
    let source = Source::new(vec![slot]);
    let gateway = Relayed::start(vec![code_model().with_context_window(1_000)], &source);
    let answer = gateway.post_text(
        "/v1/messages",
        "{\"model\":\"code\",\"max_tokens\":32000,\"messages\":[]}",
    );
    assert_eq!(answer.status, 200, "{:?}", answer.code());
    let seen = pod.seen();
    assert_eq!(seen.len(), 2);
    for request in &seen {
        let text = String::from_utf8_lossy(&request.raw).into_owned();
        let head = text.split("\r\n\r\n").next().unwrap();
        assert_eq!(request.authorization, None, "{head}");
        assert!(
            head.contains(&format!("\r\nx-api-key: {VLLM_KEY}\r\n")),
            "{head}"
        );
        assert!(find(&request.raw, OWNER.as_bytes()).is_none(), "{head}");
    }
    assert_eq!(
        seen[1].body_text(),
        "{\"model\":\"served-code\",\"max_tokens\":100,\"messages\":[]}"
    );
}

/// A body of exactly the 32 MiB bound is counted whole and relayed whole with only its
/// `model` and `max_tokens` changed.
#[test]
fn adversary_mtc_a_body_at_the_32_mib_bound_is_counted_and_lowered_whole() {
    let pod = Pod::start(vec![ok_json("{\"input_tokens\":900}"), ok_json("{}")]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![code_model().with_max_model_len(1_000)], &source);
    let body = pad_to("{\"model\":\"code\",\"max_tokens\":32000", BODY_BOUND);
    let answer = gateway.post("/v1/messages", &body);
    assert_eq!(answer.status, 200, "{:?}", answer.code());
    let seen = pod.seen();
    assert_eq!(seen.len(), 2);
    let counted = pad_to(
        "{\"model\":\"served-code\",\"max_tokens\":32000",
        BODY_BOUND + 7,
    );
    let relayed = pad_to(
        "{\"model\":\"served-code\",\"max_tokens\":100",
        BODY_BOUND + 7 - 2,
    );
    assert!(
        seen[0].body.as_slice().eq(counted.as_slice()),
        "the count's body"
    );
    assert!(
        seen[1].body.as_slice().eq(relayed.as_slice()),
        "the relayed body"
    );
    length_matches(&seen[0]);
    length_matches(&seen[1]);
}

/// The count is counted in no series: a clamped request and one whose count answered 500 are
/// two inference requests, neither an upstream failure nor a status failure.
#[test]
fn adversary_mtc_the_count_feeds_no_metric_series() {
    let pod = Pod::start(vec![
        ok_json("{\"input_tokens\":900}"),
        ok_json("{}"),
        reply(500, "application/json", b"{\"error\":\"boom\"}"),
        ok_json("{}"),
    ]);
    let source = Source::of(&[&pod]);
    let gateway = Relayed::start(vec![code_model().with_max_model_len(1_000)], &source);
    for _ in 0..2 {
        let answer = gateway.post_text(
            "/v1/messages",
            "{\"model\":\"code\",\"max_tokens\":32000,\"messages\":[]}",
        );
        assert_eq!(answer.status, 200, "{:?}", answer.code());
    }
    let scrape = send(gateway.addr, &request("GET", "/metrics", Some(OWNER), b"")).text();
    for line in [
        "\nllmgw_inference_requests_total 2\n",
        "\nllmgw_upstream_failures_total 0\n",
        "\nllmgw_endpoint_invalidations_total 0\n",
        "\nllmgw_route_requests_total{model=\"code\",wire=\"messages\"} 2\n",
        "\nllmgw_route_refusals_total{model=\"code\",wire=\"messages\"} 0\n",
        "\nllmgw_route_upstream_status_failures_total{model=\"code\",wire=\"messages\"} 0\n",
    ] {
        assert!(scrape.contains(line), "{line:?} not in\n{scrape}");
    }
}

// --- Usage details (story:usage-details-pass-through) ------------------------------------------
//
// The fixtures under `tests/fixtures/usage/` are authored from vLLM v0.27.1's usage models, not
// captured; each is headed by its sources and `tests/fixtures/usage/README.md` describes them.
// Every usage block carries `future_usage_field`, a field no server defines, so a relay that
// parsed and re-wrote usage would drop it.

const USAGE_CHAT: &str = include_str!("fixtures/usage/chat.response.txt");
const USAGE_CHAT_STREAM: &str = include_str!("fixtures/usage/chat-stream.response.txt");
const USAGE_MESSAGES: &str = include_str!("fixtures/usage/messages.response.txt");
const USAGE_MESSAGES_STREAM: &str = include_str!("fixtures/usage/messages-stream.response.txt");
const UNKNOWN_USAGE_FIELD: &str = "\"future_usage_field\":{\"kept\":true}";

/// Relays `request` on `path` to a pod answering the usage fixture `response`, and checks the
/// client received the fixture byte for byte under the pod's content type, and the pod the
/// request with only `model` rewritten. The model declares no window, so nothing is counted.
fn relays_usage(path: &str, request: &str, response: &'static str, carries: &[&str]) {
    let body = fixture(response);
    for part in carries.iter().chain(&[UNKNOWN_USAGE_FIELD]) {
        assert!(body.contains(part), "the fixture carries {part}");
    }
    let streamed = request.contains("\"stream\":true");
    let answer = if streamed {
        stream(&body.split_inclusive("\n\n").collect::<Vec<_>>())
    } else {
        reply(200, "application/json", body.as_bytes())
    };
    let pod = Pod::start(vec![answer]);
    let source = Source::of(&[&pod]);
    let model = RelayModel::new(
        label("code"),
        label(SERVED_QWEN),
        vec![Wire::Chat, Wire::Messages],
    )
    .unwrap();
    let gateway = Relayed::start(vec![model], &source);

    let answered = gateway.post_text(path, request);
    assert_eq!(answered.status, 200, "{:?}", answered.code());
    assert_eq!(
        answered.header("content-type"),
        Some(if streamed {
            "text/event-stream"
        } else {
            "application/json"
        })
    );
    assert!(
        answered.body.as_slice() == body.as_bytes(),
        "the client receives the target's answer byte for byte, usage included:\n{}",
        answered.text()
    );
    let bodies: Vec<String> = pod.seen().iter().map(Seen::body_text).collect();
    assert_eq!(
        bodies,
        [request.replacen(
            "\"model\":\"code\"",
            &format!("\"model\":\"{SERVED_QWEN}\""),
            1
        )]
    );
    assert_eq!(source.acquired(), ["code"]);
    assert!(source.released_reaches(1));
}

#[test]
fn usage_a_chat_answer_reaches_the_client_with_its_cached_tokens_and_an_unknown_field() {
    relays_usage(
        "/v1/chat/completions",
        "{\"model\":\"code\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}",
        USAGE_CHAT,
        &["\"prompt_tokens_details\":{\"cached_tokens\":1024,"],
    );
}

#[test]
fn usage_a_chat_stream_reaches_the_client_with_its_final_usage_chunk_unchanged() {
    relays_usage(
        "/v1/chat/completions",
        "{\"model\":\"code\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}],\"stream\":true,\"stream_options\":{\"include_usage\":true}}",
        USAGE_CHAT_STREAM,
        &[
            "\"choices\":[],\"usage\":{",
            "\"prompt_tokens_details\":{\"cached_tokens\":1024,",
            "data: [DONE]\n\n",
        ],
    );
}

#[test]
fn usage_a_messages_answer_reaches_the_client_with_its_cache_tokens_and_an_unknown_field() {
    relays_usage(
        "/v1/messages",
        "{\"model\":\"code\",\"max_tokens\":16,\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}",
        USAGE_MESSAGES,
        &[
            "\"cache_creation_input_tokens\":192",
            "\"cache_read_input_tokens\":1024",
        ],
    );
}

#[test]
fn usage_a_messages_stream_reaches_the_client_with_its_start_and_delta_usage_unchanged() {
    relays_usage(
        "/v1/messages",
        "{\"model\":\"code\",\"max_tokens\":16,\"stream\":true,\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}",
        USAGE_MESSAGES_STREAM,
        &[
            "\"type\":\"message_start\"",
            "\"usage\":{\"input_tokens\":1234,\"output_tokens\":0,\"future_usage_field\"",
            "\"type\":\"message_delta\"",
            "\"cache_creation_input_tokens\":192",
            "\"cache_read_input_tokens\":1024",
        ],
    );
}
