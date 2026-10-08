//! The relay of the three model-call wires (story:wire-relay), one test per behaviour, each
//! named after the row of `docs/llmgw-capability-matrix.md` it closes: R6, R7, R8, W1, W2, W3,
//! W4, W5, W6, W7, K11 and B8. The specification is the `Relay` command of
//! `spec/domains/gateway.yaml`.
//!
//! Every target is a loopback fixture pod in this file: a `TcpListener` on `127.0.0.1:0` that
//! answers a fixed script. The gateway never connects anywhere itself; it asks the injected
//! [`RelayTargets`] for a target and the target opens the connection. Nothing leaves the
//! machine, and no provider is involved.

use llm_gateway::{
    Gateway, GatewayConfig, GatewayHandle, Label, OwnerToken, Relay, RelayError, RelayModel,
    RelayStream, RelayTarget, RelayTargets, RouteInventory, SharedSecretVerifier, TargetBearer,
    TargetRefusal, TokenError, Wire,
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
    }
}

fn refusing(name: &str) -> Slot {
    Slot {
        authority: format!("{name}.invalid:8000"),
        refuses: true,
        bearer: None,
    }
}

impl RelayTargets for Source {
    fn acquire(&self, alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        self.acquired.lock().unwrap().push(alias.to_string());
        if self
            .cold
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| left.checked_sub(1))
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

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        if self.slot.refuses {
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
    assert!(source.invalidated().is_empty(), "a cold start is no failure");
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
