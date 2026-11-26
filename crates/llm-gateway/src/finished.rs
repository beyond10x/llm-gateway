//! One finished-call value per model call, and a bounded copy of its bytes when the relay
//! declares one (story:finished-call-facts; `FinishedCall`, `CallBytes` and `CallBytesBound` in
//! `spec/domains/telemetry.yaml`, docs/gateway.md "Finished calls").
//!
//! A value leaves the crate only through [`FinishedCalls`], which the embedding implements, as it
//! implements [`crate::UsageRecords`]. It is built at the one place a call ends, where its usage
//! record is made, and handed over once, on the connection's thread, before the connection is
//! closed. The fields map one to one onto agentspec's `GatewayFacts`; the copy onto its
//! `WireCall` request and answer.

use crate::{error::RefusalCode, relay::Wire};

/// Which harness named a call's session, read as the dashboard labels it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Harness {
    /// `x-claude-code-session-id`, or a `claude-cli` user agent.
    ClaudeCode,
    /// A `codex…` user agent, or `session-id`, `session_id` or `x-opencode-session` sent without
    /// a user agent.
    Codex,
    /// Every other client.
    Other,
}

impl Harness {
    /// The harness of a dashboard label (`claude-code`, `codex`, or anything else).
    pub(crate) fn of_label(label: &str) -> Self {
        match label {
            "claude-code" => Self::ClaudeCode,
            "codex" => Self::Codex,
            _ => Self::Other,
        }
    }
}

/// How a finished call ended, in this order of precedence: refused, ended in its wire's terminal
/// error, cut short by its upstream, left by its client, complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CallEnding {
    /// The answer reached the client whole, whatever its status.
    Complete,
    /// The client went away before the answer's last byte, or before any answer was written.
    ClientDisconnected,
    /// A read of the answer's body failed after its head was written.
    UpstreamCut,
    /// The answer, its head written, ended in its wire's terminal error: told by a translator,
    /// or read from its bytes.
    WireError,
    /// The client was answered this refusal (`upstream-failed` included).
    Refused(RefusalCode),
}

/// What the gateway knows of one finished model call (`llm-gateway.telemetry.FinishedCall`).
/// Each field is agentspec's `GatewayFacts` field of the same name, but `declared_cost_usd`
/// (its `declared_cost_microdollars`, converted by the embedding: the crate does no arithmetic on
/// money) and `ending` (its `incomplete`); `wire` is its `WireCall`'s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishedCall {
    /// The id every answer to the call carried as `x-llm-gateway-call` ([`crate::UsageRecord`]'s
    /// `call`).
    pub call_id: String,
    /// The session's hash, 12 lower-case hex digits, from the reader the dashboard uses.
    pub session_hash: String,
    pub harness: Harness,
    /// The path the client called: its wire's path.
    pub path: String,
    /// The wire the client spoke.
    pub wire: Wire,
    /// The relayed model's alias the body named; `None` when it named none.
    pub route: Option<String>,
    /// The authority of the target the call was handed; `None` when none was.
    pub upstream: Option<String>,
    /// Attempts at the target: each attempt of a passthrough call, 1 on the byte relay and the
    /// translated path once a target was handed out, 0 when none was.
    pub attempts: u32,
    /// Milliseconds from the end of the request head until the target's connection was open.
    pub queue_wait_ms: Option<u64>,
    /// Milliseconds from the end of the request head until the first answer body byte reached
    /// the client.
    pub ttft_ms: Option<u64>,
    /// The usage record's `cost_usd`, the same decimal string: present exactly when it is.
    pub declared_cost_usd: Option<String>,
    pub ending: CallEnding,
    /// Milliseconds since the Unix epoch at the end of the request head.
    pub started_ms: u64,
    /// Milliseconds since the Unix epoch when the call's last byte or refusal was written.
    pub finished_ms: u64,
}

/// A copied answer, as the client received it, de-chunked and, when it carried a content coding,
/// decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerBytes {
    /// The body bytes, de-chunked and decoded.
    Body(Vec<u8>),
    /// The `data:` payload of each server-sent event the client received whole, in order.
    Events(Vec<Vec<u8>>),
}

/// The copy of one call's bytes, handed beside its [`FinishedCall`] when the relay was declared
/// [`crate::Relay::with_call_bytes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallBytes {
    /// The request body and the answer together were at most the bound. `answer` is `None` when
    /// no answer head of the target's was written to the client.
    Copied {
        request: Vec<u8>,
        answer: Option<AnswerBytes>,
    },
    /// They were over the bound: no bytes, both sizes, the answer's decoded.
    OverBound {
        request_bytes: u64,
        answer_bytes: u64,
    },
    /// The answer carried a content coding the relay's [`crate::BodyDecoder`] did not decode
    /// whole: no bytes, the request's size, the answer's size as the client received it
    /// (encoded), and the coding (`content-encoding` as the target sent it).
    Undecodable {
        request_bytes: u64,
        answer_bytes: u64,
        coding: String,
    },
}

/// Where the gateway hands each [`FinishedCall`], with its [`CallBytes`] when the relay copies
/// them. The embedding implements it; it is called on the connection's own thread before the
/// connection is closed, so it should return at once.
pub trait FinishedCalls: Send + Sync {
    fn finished(&self, call: &FinishedCall, bytes: Option<&CallBytes>);
}

/// Hands values to nobody: what a relay composed without [`crate::Relay::with_finished_calls`]
/// uses.
pub(crate) struct NoFinishedCalls;

impl FinishedCalls for NoFinishedCalls {
    fn finished(&self, _call: &FinishedCall, _bytes: Option<&CallBytes>) {}
}

/// How many times a copy grew a buffer of its own, in this process: what the allocation test
/// reads.
#[cfg(test)]
pub(crate) static COPY_ALLOCATIONS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Notes a buffer of a copy whose allocation went from `before` to `after` bytes: what the
/// allocation test counts.
fn grew(before: usize, after: usize) {
    #[cfg(test)]
    if before != after {
        COPY_ALLOCATIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    #[cfg(not(test))]
    let _ = (before, after);
}

/// Appends `bytes` to `buffer`, noting a grown allocation.
fn append(buffer: &mut Vec<u8>, bytes: &[u8]) {
    let before = buffer.capacity();
    buffer.extend_from_slice(bytes);
    grew(before, buffer.capacity());
}

/// Reads server-sent events as their bytes arrive, split anywhere (the event stream format of
/// the HTML standard): the `data` of each event a blank line ends, several `data` lines of one
/// event joined by a line feed. A line ends at CR, LF or CRLF; a line starting `:` is a
/// comment; every field but `data` is passed over; an event with no `data` field is none.
#[derive(Debug, Default)]
pub struct EventReader {
    line: Vec<u8>,
    data: Vec<u8>,
    /// The event being read has a `data` field, though perhaps an empty one.
    has_data: bool,
    /// The last byte was a CR, so an LF right after it ends no further line.
    after_cr: bool,
    events: Vec<Vec<u8>>,
}

impl EventReader {
    /// Reads `bytes`, the next bytes of the stream, split anywhere.
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut start = 0;
        for (at, &byte) in bytes.iter().enumerate() {
            if self.after_cr && byte == b'\n' {
                self.after_cr = false;
                start = at + 1;
                continue;
            }
            self.after_cr = false;
            if byte == b'\r' || byte == b'\n' {
                append(&mut self.line, &bytes[start..at]);
                self.end_line();
                self.after_cr = byte == b'\r';
                start = at + 1;
            }
        }
        append(&mut self.line, &bytes[start..]);
    }

    /// The `data` of every event ended since the last take, in order.
    pub fn take(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.events)
    }

    fn end_line(&mut self) {
        if self.line.is_empty() {
            if self.has_data {
                let before = self.events.capacity();
                self.events.push(std::mem::take(&mut self.data));
                grew(before, self.events.capacity());
            }
            self.has_data = false;
            return;
        }
        if self.line[0] != b':' {
            let (field, value) = match self.line.iter().position(|&byte| byte == b':') {
                Some(colon) => (&self.line[..colon], &self.line[colon + 1..]),
                None => (&self.line[..], &[][..]),
            };
            if field == b"data" {
                let value = value.strip_prefix(b" ").unwrap_or(value);
                if self.has_data {
                    append(&mut self.data, b"\n");
                }
                append(&mut self.data, value);
                self.has_data = true;
            }
        }
        self.line.clear();
    }
}

/// A copied answer while it arrives.
#[derive(Debug)]
enum Copying {
    Body(Vec<u8>),
    Events(EventReader),
}

/// The bytes a copy holds while the call is within its bound.
#[derive(Debug, Default)]
struct Held {
    request: Vec<u8>,
    /// `None` until an answer head was written to the client.
    answer: Option<Copying>,
}

/// An answer with a content coding while it arrives: its bytes are counted as the client
/// received them, and copied only once the relay hands them decoded ([`Tap::decoded`]).
#[derive(Debug)]
struct Coded {
    coding: String,
    received: u64,
}

/// The copy of one call's bytes while it runs: the request body as read, and the answer body as
/// the client received it (decoded when it carried a content coding), until the two together
/// pass the bound, when the copy is dropped and only their sizes are counted.
#[derive(Debug)]
pub(crate) struct Tap {
    bound: usize,
    request_bytes: u64,
    answer_bytes: u64,
    /// `None` once the call passed the bound.
    held: Option<Held>,
    /// The answer's content coding, when it carried one and was not yet handed decoded.
    coded: Option<Coded>,
}

impl Tap {
    pub(crate) fn new(bound: usize) -> Self {
        Self {
            bound,
            request_bytes: 0,
            answer_bytes: 0,
            held: Some(Held::default()),
            coded: None,
        }
    }

    /// The answer's content coding was `coding`, and `decoded` is what the relay's decoder made
    /// of the bytes the client received, `None` when there was none to ask or it knew no such
    /// coding. A whole decode is copied as the answer; any other is
    /// [`CallBytes::Undecodable`].
    pub(crate) fn decoded(&mut self, decoded: Option<&crate::relay::Decoded>) {
        let Some(coded) = self.coded.take() else {
            return;
        };
        match decoded {
            Some(decoded) if decoded.complete => self.answer(&decoded.bytes),
            _ => {
                self.held = None;
                self.coded = Some(coded);
            }
        }
    }

    /// Whether the bytes counted so far pass the bound; the copy is dropped when they do.
    fn over(&mut self) -> bool {
        let counted = self.request_bytes.saturating_add(self.answer_bytes);
        if counted > u64::try_from(self.bound).unwrap_or(u64::MAX) {
            self.held = None;
        }
        self.held.is_none()
    }

    /// The request body as the gateway read it.
    pub(crate) fn request(&mut self, body: &[u8]) {
        self.request_bytes = u64::try_from(body.len()).unwrap_or(u64::MAX);
        if self.over() {
            return;
        }
        if let Some(held) = &mut self.held {
            append(&mut held.request, body);
        }
    }

    /// An answer head was written to the client; `events` says its body is a server-sent event
    /// stream, and `coding` names its content coding when it carries one.
    pub(crate) fn head(&mut self, events: bool, coding: Option<&str>) {
        if let Some(held) = &mut self.held {
            held.answer = Some(if events {
                Copying::Events(EventReader::default())
            } else {
                Copying::Body(Vec::new())
            });
        }
        self.coded = coding.map(|coding| Coded {
            coding: coding.to_owned(),
            received: 0,
        });
    }

    /// Answer body bytes the client received. Those of an answer with a content coding are only
    /// counted until the relay hands them decoded.
    pub(crate) fn answer(&mut self, bytes: &[u8]) {
        if let Some(coded) = &mut self.coded {
            coded.received = coded
                .received
                .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
            return;
        }
        self.answer_bytes = self
            .answer_bytes
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        if self.over() {
            return;
        }
        match self.held.as_mut().and_then(|held| held.answer.as_mut()) {
            Some(Copying::Body(body)) => append(body, bytes),
            Some(Copying::Events(reader)) => reader.feed(bytes),
            None => {}
        }
    }

    /// The copy, both sizes when it passed the bound, or both sizes and the coding when its
    /// answer was not decoded whole. An answer with a coding the relay never handed decoded (the
    /// call ended before it could be) is not decoded whole.
    pub(crate) fn finish(self) -> CallBytes {
        if let Some(coded) = self.coded {
            return CallBytes::Undecodable {
                request_bytes: self.request_bytes,
                answer_bytes: coded.received,
                coding: coded.coding,
            };
        }
        match self.held {
            Some(held) => CallBytes::Copied {
                request: held.request,
                answer: held.answer.map(|answer| match answer {
                    Copying::Body(body) => AnswerBytes::Body(body),
                    Copying::Events(reader) => AnswerBytes::Events(reader.events),
                }),
            },
            None => CallBytes::OverBound {
                request_bytes: self.request_bytes,
                answer_bytes: self.answer_bytes,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AnswerBytes, COPY_ALLOCATIONS, CallBytes, Tap};
    use crate::{
        Gateway, GatewayConfig, IngressAuth, Label, NoOwnerVerifier, Relay, RelayModel,
        RelayStream, RelayTarget, RelayTargets, RouteInventory, TargetRefusal, Wire,
    };
    use std::{
        io::{self, Cursor, Read, Write},
        net::TcpStream,
        sync::{Arc, atomic::Ordering},
        time::Duration,
    };

    /// A recorded stream: named events, a comment, a two-line data field, an event with no
    /// data, CRLF and lone CR line ends, and a last event the stream never ended.
    const STREAM: &[u8] = b"event: a\ndata: {\"n\":1}\n\n: a comment\ndata:{\"n\":2}\ndata: x\n\nevent: ping\n\nevent: c\r\ndata: {\"n\":3}\r\n\r\nretry: 5\rdata: 4\r\rdata: never ended\n";

    fn expected() -> Vec<Vec<u8>> {
        vec![
            b"{\"n\":1}".to_vec(),
            b"{\"n\":2}\nx".to_vec(),
            b"{\"n\":3}".to_vec(),
            b"4".to_vec(),
        ]
    }

    fn copied(pieces: &[&[u8]], events: bool) -> CallBytes {
        let mut tap = Tap::new(1 << 20);
        tap.request(b"{}");
        tap.head(events, None);
        for piece in pieces {
            tap.answer(piece);
        }
        tap.finish()
    }

    #[test]
    fn a_streams_copy_is_its_data_payloads_however_its_bytes_are_split() {
        let events = |answer| CallBytes::Copied {
            request: b"{}".to_vec(),
            answer: Some(AnswerBytes::Events(answer)),
        };
        assert_eq!(copied(&[STREAM], true), events(expected()));
        for at in 0..=STREAM.len() {
            assert_eq!(
                copied(&[&STREAM[..at], &STREAM[at..]], true),
                events(expected()),
                "split at {at}"
            );
        }
        let bytes: Vec<&[u8]> = STREAM.chunks(1).collect();
        assert_eq!(copied(&bytes, true), events(expected()));
        // Not an event stream: the bytes as they came.
        assert_eq!(
            copied(&[&STREAM[..7], &STREAM[7..]], false),
            CallBytes::Copied {
                request: b"{}".to_vec(),
                answer: Some(AnswerBytes::Body(STREAM.to_vec())),
            }
        );
    }

    // --- The allocation count of the copy path ------------------------------------------------

    struct Canned(Cursor<Vec<u8>>);

    impl Read for Canned {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.0.read(buffer)
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

    impl RelayStream for Canned {}

    struct Answers(Vec<Vec<u8>>);

    struct Answer(Vec<u8>);

    impl RelayTarget for Answer {
        fn authority(&self) -> &'static str {
            "canned.invalid:80"
        }

        fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
            Ok(Box::new(Canned(Cursor::new(self.0.clone()))))
        }
    }

    impl RelayTargets for Answers {
        fn acquire(&self, alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
            let answer = usize::from(alias != "json");
            Ok(Box::new(Answer(self.0[answer].clone())))
        }

        fn invalidate(&self, _alias: &str, _authority: &str) {}
    }

    /// A JSON call and a streamed call through a gateway whose relay copies with `call_bytes`.
    fn two_calls(call_bytes: Option<usize>) {
        let json =
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}"
                .to_vec();
        let mut stream =
            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: ".to_vec();
        stream.extend_from_slice(format!("{}\r\n\r\n", STREAM.len()).as_bytes());
        stream.extend_from_slice(STREAM);
        let models = ["json", "stream"]
            .map(|alias| {
                RelayModel::new(
                    Label::new(alias).unwrap(),
                    Label::new("served").unwrap(),
                    vec![Wire::Messages],
                )
                .unwrap()
            })
            .to_vec();
        let mut relay = Relay::new(models, Arc::new(Answers(vec![json, stream]))).unwrap();
        if let Some(bound) = call_bytes {
            relay = relay.with_call_bytes(bound);
        }
        let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
        config.ingress_auth = IngressAuth::None;
        let handle = Gateway::bind_with_relay(
            config,
            Arc::new(NoOwnerVerifier),
            RouteInventory::new(Vec::new()).unwrap(),
            relay,
        )
        .unwrap();
        handle.mark_ready();
        for alias in ["json", "stream"] {
            let body = format!("{{\"model\":\"{alias}\"}}");
            let mut client = TcpStream::connect(handle.local_addr()).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            write!(
                client,
                "POST /v1/messages HTTP/1.1\r\nhost: gateway\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            let mut answer = Vec::new();
            drop(client.read_to_end(&mut answer));
            assert!(answer.starts_with(b"HTTP/1.1 200 "), "{alias}");
        }
        handle.shutdown();
    }

    /// Without `with_call_bytes` the copy path allocates nothing; with it, it does. The count is
    /// the process's: cargo-nextest, the crate's runner, runs each test in a process of its own.
    #[test]
    fn the_copy_path_allocates_nothing_without_call_bytes() {
        two_calls(None);
        assert_eq!(COPY_ALLOCATIONS.load(Ordering::SeqCst), 0);
        two_calls(Some(1 << 20));
        assert!(COPY_ALLOCATIONS.load(Ordering::SeqCst) > 0);
    }
}
