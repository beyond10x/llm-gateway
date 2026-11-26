//! One finished-call value per model call, and a bounded copy of its bytes when the relay
//! declares one (story:finished-call-facts; `FinishedCall`, `CallBytes` and `CallBytesBound` in
//! `spec/domains/telemetry.yaml`, docs/gateway.md "Finished calls").
//!
//! A value leaves the crate only through [`FinishedCalls`], which the embedding implements, as it
//! implements [`crate::UsageRecords`]. It is built at the one place a call ends, where its usage
//! record is made, and handed over once, on the connection's thread, before the connection is
//! closed. The fields map one to one onto sessionspec's `GatewayFacts`; the copy onto its
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
/// Each field is sessionspec's `GatewayFacts` field of the same name, but `declared_cost_usd`
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

/// A copied answer, as the client received it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerBytes {
    /// The body bytes, de-chunked.
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
    /// They were over the bound: no bytes, both sizes.
    OverBound {
        request_bytes: u64,
        answer_bytes: u64,
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

/// The copy of one call's bytes while it runs.
#[derive(Debug)]
pub(crate) struct Tap {
    bound: usize,
}

impl Tap {
    pub(crate) fn new(bound: usize) -> Self {
        Self { bound }
    }

    /// The request body as the gateway read it.
    pub(crate) fn request(&mut self, _body: &[u8]) {}

    /// An answer head was written to the client; `events` says its body is a server-sent event
    /// stream without a content coding.
    pub(crate) fn head(&mut self, _events: bool) {}

    /// Answer body bytes the client received.
    pub(crate) fn answer(&mut self, _bytes: &[u8]) {}

    /// The copy, or both sizes when it passed the bound.
    pub(crate) fn finish(self) -> CallBytes {
        let _ = self.bound;
        CallBytes::Copied {
            request: Vec::new(),
            answer: None,
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
        tap.head(events);
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
            let answer = if alias == "json" { 0 } else { 1 };
            Ok(Box::new(Answer(self.0[answer].clone())))
        }

        fn invalidate(&self, _alias: &str, _authority: &str) {}
    }

    /// A JSON call and a streamed call through a gateway whose relay copies with `call_bytes`.
    fn two_calls(call_bytes: Option<usize>) {
        let json = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}".to_vec();
        let mut stream = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: "
            .to_vec();
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
