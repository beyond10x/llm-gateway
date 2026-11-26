//! The fake upstream of these tests, and what another process on the host can do to it.
//!
//! A loopback port is open to every process on the machine, and cargo-nextest runs each test in
//! a process of its own, many at once: a gateway of another test whose target held the same port
//! before, or a test that probes a port it chose, may connect to an upstream that is not its own.
//! So an [`Upstream`] answers only its own test's calls. Each upstream has a token of its own,
//! which its test's targets send as the static header [`HEADER`] ([`Upstream::headers`],
//! [`Upstream::target`]); a connection whose request does not carry it is closed unanswered and
//! takes no reply. It serves each connection on a thread of its own, and holds its listener until
//! it is dropped, so the connection that wakes its accept loop reaches this upstream and no other
//! process's listener.

use llm_gateway::{RelayStream, RelayTarget, StaticHeader};
use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// How long the upstream waits on one read or write of a connection, and a foreign request on
/// its answer.
const WAIT: Duration = Duration::from_secs(60);

/// The header each request of a test's calls carries to its upstream, valued per upstream.
pub const HEADER: &str = "x-test-upstream";

/// What the upstream does with one connection of its test's calls, after it read the request.
/// The connection is shut down when it returns.
pub type Reply = Box<dyn FnOnce(&mut TcpStream) + Send>;

/// What the upstream does with every call past its script.
type Rest = Arc<dyn Fn(&mut TcpStream) + Send + Sync>;

/// The replies left, and every request of the test's calls the upstream read.
struct Script {
    replies: VecDeque<Reply>,
    rest: Option<Rest>,
    seen: Vec<Vec<u8>>,
}

/// A loopback upstream that answers its test's calls with its replies in the order their
/// requests arrive. A call past its script is closed unanswered, unless the upstream answers
/// every call ([`Upstream::answering`], [`Upstream::scripted_then`]).
pub struct Upstream {
    authority: String,
    header: StaticHeader,
    script: Arc<Mutex<Script>>,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Upstream {
    /// Answers the test's calls with `replies`, in order.
    pub fn scripted(replies: Vec<Reply>) -> Self {
        Self::start(replies, None)
    }

    /// Answers every call of the test with `reply`.
    pub fn answering(reply: impl Fn(&mut TcpStream) + Send + Sync + 'static) -> Self {
        Self::start(Vec::new(), Some(Arc::new(reply)))
    }

    /// Answers the test's calls with `replies`, in order, then every later one with `rest`.
    pub fn scripted_then(
        replies: Vec<Reply>,
        rest: impl Fn(&mut TcpStream) + Send + Sync + 'static,
    ) -> Self {
        Self::start(replies, Some(Arc::new(rest)))
    }

    fn start(replies: Vec<Reply>, rest: Option<Rest>) -> Self {
        static STARTED: AtomicUsize = AtomicUsize::new(0);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let authority = listener.local_addr().unwrap().to_string();
        let token = format!(
            "{}-{}",
            std::process::id(),
            STARTED.fetch_add(1, Ordering::SeqCst)
        );
        let header = StaticHeader::new(HEADER, &token).unwrap();
        let script = Arc::new(Mutex::new(Script {
            replies: VecDeque::from(replies),
            rest,
            seen: Vec::new(),
        }));
        let stopping = Arc::new(AtomicBool::new(false));
        let thread = {
            let (script, stopping) = (Arc::clone(&script), Arc::clone(&stopping));
            thread::spawn(move || {
                for connection in listener.incoming() {
                    if stopping.load(Ordering::SeqCst) {
                        return;
                    }
                    let Ok(connection) = connection else {
                        continue;
                    };
                    let (token, script) = (token.clone(), Arc::clone(&script));
                    thread::spawn(move || serve(connection, &token, &script));
                }
            })
        };
        Self {
            authority,
            header,
            script,
            stopping,
            thread: Some(thread),
        }
    }

    /// The upstream's `host:port` on loopback.
    pub fn authority(&self) -> &str {
        &self.authority
    }

    /// The static headers a target of this upstream sends: [`HEADER`] with this upstream's token.
    pub fn headers(&self) -> Vec<StaticHeader> {
        vec![self.header.clone()]
    }

    /// Every request of the test's calls the upstream read, head and body, in the order they
    /// arrived, each without its [`HEADER`] line: the bytes the gateway would have sent a target
    /// without static headers.
    pub fn seen(&self) -> Vec<Vec<u8>> {
        self.script.lock().unwrap().seen.clone()
    }

    /// A target on this upstream whose connections time out after `wait`.
    pub fn target(&self, wait: Duration) -> Target {
        Target {
            authority: self.authority.clone(),
            headers: self.headers(),
            wait,
        }
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        // The accept loop holds the listener until it sees `stopping`, so this connection
        // reaches this upstream and no other process's listener.
        self.stopping.store(true, Ordering::SeqCst);
        drop(TcpStream::connect(&self.authority));
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

/// Answers one connection: the next reply when its request is a call of this upstream's test,
/// else nothing.
fn serve(mut connection: TcpStream, token: &str, script: &Mutex<Script>) {
    if connection.set_read_timeout(Some(WAIT)).is_err()
        || connection.set_write_timeout(Some(WAIT)).is_err()
    {
        return;
    }
    let Some(request) = read_request(&mut connection) else {
        return;
    };
    if let Some(kept) = without_token(&request, token) {
        let (reply, rest) = {
            let mut script = script.lock().unwrap();
            script.seen.push(kept);
            let reply = script.replies.pop_front();
            let rest = reply.is_none().then(|| script.rest.clone()).flatten();
            (reply, rest)
        };
        if let Some(reply) = reply {
            reply(&mut connection);
        } else if let Some(rest) = rest {
            rest(&mut connection);
        }
    }
    drop(connection.flush());
    drop(connection.shutdown(Shutdown::Both));
}

/// `request` without its [`HEADER`] line, when its head carries one valued `token`.
fn without_token(request: &[u8], token: &str) -> Option<Vec<u8>> {
    let end = find(request, b"\r\n\r\n")?;
    let mut start = find(request, b"\r\n")? + 2;
    while start < end {
        let stop = start + find(&request[start..], b"\r\n")?;
        let line = String::from_utf8_lossy(&request[start..stop]);
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case(HEADER)
            && value.trim() == token
        {
            return Some([&request[..start], &request[stop + 2..]].concat());
        }
        start = stop + 2;
    }
    None
}

/// The head of a raw request, up to its blank line, as text.
pub fn head_of(raw: &[u8]) -> String {
    let end = find(raw, b"\r\n\r\n").unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).into_owned()
}

/// The body of a raw request, after its blank line.
pub fn body_of(raw: &[u8]) -> &[u8] {
    find(raw, b"\r\n\r\n").map_or(&[], |end| &raw[end + 4..])
}

/// A `200` answer of the JSON body `{}`, framed by its length.
pub const OK: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}";

/// A reply that writes `raw` as it is.
pub fn bytes(raw: impl Into<Vec<u8>>) -> Reply {
    let raw = raw.into();
    Box::new(move |connection| drop(connection.write_all(&raw)))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// One read of a connection. An interrupted read is retried: a socket with a read timeout
/// answers a signal, or a stop and continue of the test process, with `EINTR` whatever
/// `SA_RESTART` says (`man 7 signal`), and an upstream that took it for the end of the request
/// would close with the body unread and reset the gateway's write.
fn read_some(connection: &mut TcpStream, chunk: &mut [u8]) -> Option<usize> {
    loop {
        match connection.read(chunk) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            read => return read.ok(),
        }
    }
}

/// Reads one request head and its `content-length` body, raw. `None` when the head did not
/// arrive whole.
fn read_request(connection: &mut TcpStream) -> Option<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut chunk = vec![0_u8; 1 << 16];
    let end = loop {
        if let Some(end) = find(&buffer, b"\r\n\r\n") {
            break end;
        }
        let read = read_some(connection, &mut chunk)?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let wanted: usize = head_of(&buffer[..end + 4])
        .split("\r\n")
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .unwrap_or(0);
    while buffer.len() < end + 4 + wanted {
        match read_some(connection, &mut chunk) {
            Some(0) | None => break,
            Some(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    }
    Some(buffer)
}

/// A target on one [`Upstream`] that sends its [`HEADER`].
#[derive(Clone)]
pub struct Target {
    authority: String,
    headers: Vec<StaticHeader>,
    wait: Duration,
}

impl RelayTarget for Target {
    fn authority(&self) -> &str {
        &self.authority
    }

    fn headers(&self) -> &[StaticHeader] {
        &self.headers
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        let stream = TcpStream::connect(&self.authority)?;
        stream.set_read_timeout(Some(self.wait))?;
        stream.set_write_timeout(Some(self.wait))?;
        Ok(Box::new(stream))
    }
}

/// A request to `authority` from no call of the test that owns it, as a gateway of another test
/// process sends one, read to its end: every byte it was answered.
pub fn foreign_request(authority: &str) -> Vec<u8> {
    let mut connection = TcpStream::connect(authority).unwrap();
    connection.set_read_timeout(Some(WAIT)).unwrap();
    connection
        .write_all(
            format!("POST /v1/chat/completions HTTP/1.1\r\nhost: {authority}\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}")
                .as_bytes(),
        )
        .unwrap();
    let mut answer = Vec::new();
    drop(connection.read_to_end(&mut answer));
    answer
}

/// The port of `authority`, bound by this process as another one would bind it once it is free,
/// tried for `within`; `None` while something else still holds it.
pub fn take_port(authority: &str, within: Duration) -> Option<TcpListener> {
    let deadline = Instant::now() + within;
    loop {
        if let Ok(listener) = TcpListener::bind(authority) {
            return Some(listener);
        }
        if Instant::now() > deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(5));
    }
}
