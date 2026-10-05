//! Relaying the three model-call wires to a target the embedding selects (story:wire-relay).
//!
//! The gateway reads and checks the request, rewrites `model` (and on the messages wire the
//! reasoning effort), asks the injected [`RelayTargets`] for a target, sends the request over
//! the connection that target opens, and relays the answer as it arrives. It never opens a
//! connection itself: the target does, so the transport — plain TCP to a loopback fixture, TLS
//! to a pod proxy — and its timeouts belong to the embedding. The specification is the `Relay`
//! command of `spec/domains/gateway.yaml`.

use crate::{
    body,
    error::{RefusalCode, RelayError},
    inventory::Label,
};
use std::{
    collections::BTreeMap,
    fmt,
    io::{self, Read, Write},
    net::TcpStream,
    sync::Arc,
    time::{Duration, Instant},
};

/// The largest request body the relay reads: 32 MiB, as llmgw (`src/lib.rs:35`).
pub(crate) const MAX_REQUEST_BODY_BYTES: u64 = 33_554_432;
/// The size of one internal read while relaying. Not a limit on anything.
const RELAY_CHUNK: usize = 16_384;

/// A wire the gateway relays. Each has its own path, and the target is asked on the same path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Wire {
    /// `POST /v1/chat/completions`, `OpenAI` chat completions.
    Chat,
    /// `POST /v1/responses`, `OpenAI` responses (the Codex wire).
    Responses,
    /// `POST /v1/messages`, Anthropic messages.
    Messages,
}

impl Wire {
    /// Every wire.
    pub const ALL: [Self; 3] = [Self::Chat, Self::Responses, Self::Messages];

    /// The path the wire is served on, and asked for at the target.
    pub const fn path(self) -> &'static str {
        match self {
            Self::Chat => "/v1/chat/completions",
            Self::Responses => "/v1/responses",
            Self::Messages => "/v1/messages",
        }
    }

    /// The wire's name in a deployment document: `chat`, `responses` or `messages`.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Responses => "responses",
            Self::Messages => "messages",
        }
    }

    pub(crate) fn from_path(path: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|wire| wire.path() == path)
    }
}

/// One connection to a target: whatever the embedding opened, read and written as bytes.
pub trait RelayStream: Read + Write + Send {}

impl<T: Read + Write + Send> RelayStream for T {}

/// One target, held for as long as a relayed answer streams and released when dropped.
pub trait RelayTarget: Send {
    /// The target's `host[:port]`. It is sent as `host`, and it is what the gateway names back
    /// in [`RelayTargets::invalidate`] when a request through this target fails.
    fn authority(&self) -> &str;

    /// Opens one connection to the target. Its read and write timeouts are the target's.
    ///
    /// # Errors
    /// The connection's own error; the gateway treats any as a transport failure.
    fn connect(&self) -> io::Result<Box<dyn RelayStream>>;
}

/// Where a model's requests go. The embedding implements this over its pool.
pub trait RelayTargets: Send + Sync {
    /// The target serving `alias` now, starting one when none is live, or `None` when none can
    /// be had.
    fn acquire(&self, alias: &str) -> Option<Box<dyn RelayTarget>>;

    /// A request for `alias` through the target at `authority` failed (row W7): drop that
    /// endpoint if it is still the current one, so the next acquisition starts a replacement.
    fn invalidate(&self, alias: &str, authority: &str);
}

/// A model the gateway relays: its alias, the model name its target expects, and its wires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayModel {
    alias: Label,
    upstream_model: Label,
    wires: Vec<Wire>,
}

impl RelayModel {
    /// # Errors
    /// [`RelayError::NoWire`] for an empty wire set, [`RelayError::RepeatedWire`] for a wire
    /// named twice (row K11).
    pub fn new(alias: Label, upstream_model: Label, wires: Vec<Wire>) -> Result<Self, RelayError> {
        if wires.is_empty() {
            return Err(RelayError::NoWire);
        }
        for (index, wire) in wires.iter().enumerate() {
            if wires[..index].contains(wire) {
                return Err(RelayError::RepeatedWire);
            }
        }
        Ok(Self {
            alias,
            upstream_model,
            wires,
        })
    }

    pub fn alias(&self) -> &Label {
        &self.alias
    }

    pub fn upstream_model(&self) -> &Label {
        &self.upstream_model
    }

    pub fn wires(&self) -> &[Wire] {
        &self.wires
    }
}

/// The relayed models and the target source, composed once.
pub struct Relay {
    models: BTreeMap<String, RelayModel>,
    targets: Arc<dyn RelayTargets>,
}

impl Relay {
    /// # Errors
    /// [`RelayError::DuplicateModel`] for two models with one alias.
    pub fn new(
        models: Vec<RelayModel>,
        targets: Arc<dyn RelayTargets>,
    ) -> Result<Self, RelayError> {
        let mut indexed = BTreeMap::new();
        for model in models {
            if indexed
                .insert(model.alias.as_str().to_string(), model)
                .is_some()
            {
                return Err(RelayError::DuplicateModel);
            }
        }
        Ok(Self {
            models: indexed,
            targets,
        })
    }
}

impl fmt::Debug for Relay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Relay")
            .field("models", &self.models.values().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// How the client frames its request body, decided from the head alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Framing {
    Length(u64),
    Chunked,
}

/// A relay the head has admitted.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Admitted {
    pub(crate) wire: Wire,
    pub(crate) framing: Framing,
    pub(crate) expects_continue: bool,
}

/// Decides the body framing from the head (row W1).
///
/// # Errors
/// [`RefusalCode::RequestMalformed`] for a framing this gateway does not read, and
/// [`RefusalCode::BodyTooLarge`] for a declared length over the bound.
pub(crate) fn framing(
    content_length: Option<&str>,
    transfer_encoding: Option<&str>,
) -> Result<Framing, RefusalCode> {
    match (content_length, transfer_encoding) {
        (None, None) => Ok(Framing::Length(0)),
        (None, Some(coding)) if coding.eq_ignore_ascii_case("chunked") => Ok(Framing::Chunked),
        (Some(length), None)
            if !length.is_empty() && length.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            match length.parse::<u64>() {
                Ok(length) if length <= MAX_REQUEST_BODY_BYTES => Ok(Framing::Length(length)),
                _ => Err(RefusalCode::BodyTooLarge),
            }
        }
        _ => Err(RefusalCode::RequestMalformed),
    }
}

/// Bytes read from one side of a relay, through a small buffer.
struct Buffered<'a> {
    source: Source<'a>,
    buffer: Vec<u8>,
    start: usize,
}

enum Source<'a> {
    /// The client, read under one deadline for the whole body.
    Client(&'a mut TcpStream, Instant),
    /// The target, read under the connection's own timeouts.
    Target(&'a mut dyn RelayStream),
}

impl Buffered<'_> {
    fn available(&self) -> &[u8] {
        &self.buffer[self.start..]
    }

    /// Reads more. `Ok(false)` is the end of the stream.
    fn fill(&mut self) -> io::Result<bool> {
        if self.start == self.buffer.len() {
            self.buffer.clear();
            self.start = 0;
        }
        let mut chunk = [0_u8; RELAY_CHUNK];
        let read = match &mut self.source {
            Source::Client(stream, deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                stream.set_read_timeout(Some(left))?;
                stream.read(&mut chunk)?
            }
            Source::Target(stream) => stream.read(&mut chunk)?,
        };
        self.buffer.extend_from_slice(&chunk[..read]);
        Ok(read > 0)
    }

    /// Up to `limit` buffered bytes, reading first when none are buffered.
    fn take(&mut self, limit: usize) -> io::Result<Vec<u8>> {
        if self.available().is_empty() && !self.fill()? {
            return Ok(Vec::new());
        }
        let count = limit.min(self.available().len());
        let taken = self.available()[..count].to_vec();
        self.start += count;
        Ok(taken)
    }

    /// One line without its CRLF, of at most `limit` bytes.
    fn line(&mut self, limit: usize) -> io::Result<String> {
        loop {
            if let Some(end) = self.available().windows(2).position(|pair| pair == b"\r\n") {
                let line = String::from_utf8(self.available()[..end].to_vec())
                    .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
                self.start += end + 2;
                return Ok(line);
            }
            if self.available().len() >= limit + 2 || !self.fill()? {
                return Err(io::ErrorKind::InvalidData.into());
            }
        }
    }

    fn expect_crlf(&mut self) -> io::Result<()> {
        if self.line(0)?.is_empty() {
            Ok(())
        } else {
            Err(io::ErrorKind::InvalidData.into())
        }
    }
}

/// A chunk-size line: hex digits and an ignored extension.
fn chunk_size(line: &str) -> io::Result<u64> {
    let digits = line.split(';').next().unwrap_or_default().trim();
    if digits.is_empty() || digits.len() > 16 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    u64::from_str_radix(digits, 16).map_err(|_| io::ErrorKind::InvalidData.into())
}

/// Reads the whole client body under one deadline, refusing it the moment it passes the bound.
fn read_body(
    input: &mut Buffered<'_>,
    framing: Framing,
    line_limit: usize,
) -> Result<Vec<u8>, RefusalCode> {
    let malformed = |_| RefusalCode::RequestMalformed;
    let mut body = Vec::new();
    match framing {
        Framing::Length(length) => {
            let length = usize::try_from(length).map_err(|_| RefusalCode::BodyTooLarge)?;
            while body.len() < length {
                let piece = input.take(length - body.len()).map_err(malformed)?;
                if piece.is_empty() {
                    return Err(RefusalCode::RequestMalformed);
                }
                body.extend_from_slice(&piece);
            }
        }
        Framing::Chunked => loop {
            let size =
                chunk_size(&input.line(line_limit).map_err(malformed)?).map_err(malformed)?;
            if size == 0 {
                // Trailer fields are read and discarded up to the empty line.
                while !input.line(line_limit).map_err(malformed)?.is_empty() {}
                break;
            }
            let total = u64::try_from(body.len()).unwrap_or(u64::MAX);
            if size > MAX_REQUEST_BODY_BYTES - total {
                return Err(RefusalCode::BodyTooLarge);
            }
            let size = usize::try_from(size).map_err(|_| RefusalCode::BodyTooLarge)?;
            let mut left = size;
            while left > 0 {
                let piece = input.take(left).map_err(malformed)?;
                if piece.is_empty() {
                    return Err(RefusalCode::RequestMalformed);
                }
                left -= piece.len();
                body.extend_from_slice(&piece);
            }
            input.expect_crlf().map_err(malformed)?;
        },
    }
    Ok(body)
}

/// How a target frames its answer.
#[derive(Debug, Clone, Copy)]
enum Answer {
    Length(u64),
    Chunked { left: u64, started: bool },
    Close,
}

/// The head of a target's answer.
struct Head {
    status: u16,
    content_type: Option<String>,
    framing: Answer,
}

fn read_answer_head(input: &mut Buffered<'_>, limit: usize) -> io::Result<Head> {
    let invalid = || io::Error::from(io::ErrorKind::InvalidData);
    loop {
        let start = input.line(limit)?;
        let mut parts = start.splitn(3, ' ');
        let version = parts.next().unwrap_or_default();
        let status = parts.next().unwrap_or_default();
        if !version.starts_with("HTTP/1.") || status.len() != 3 {
            return Err(invalid());
        }
        let status: u16 = status.parse().map_err(|_| invalid())?;
        if !(100..=599).contains(&status) {
            return Err(invalid());
        }
        let mut content_type = None;
        let mut length = None;
        let mut chunked = false;
        let mut used = start.len();
        loop {
            let line = input.line(limit)?;
            used += line.len() + 2;
            if used > limit {
                return Err(invalid());
            }
            if line.is_empty() {
                break;
            }
            let (name, value) = line.split_once(':').ok_or_else(invalid)?;
            let value = value.trim();
            match name.trim().to_ascii_lowercase().as_str() {
                "content-type" if value.bytes().all(|byte| (0x20..0x7f).contains(&byte)) => {
                    content_type = Some(value.to_string());
                }
                "content-length" => length = Some(value.parse::<u64>().map_err(|_| invalid())?),
                "transfer-encoding" => chunked = value.to_ascii_lowercase().ends_with("chunked"),
                _ => {}
            }
        }
        // An interim answer is not the answer.
        if status < 200 {
            continue;
        }
        let framing = if chunked {
            Answer::Chunked {
                left: 0,
                started: false,
            }
        } else {
            length.map_or(Answer::Close, Answer::Length)
        };
        return Ok(Head {
            status,
            content_type,
            framing,
        });
    }
}

/// The next piece of a target's decoded body, or `None` at its end.
fn next_piece(
    input: &mut Buffered<'_>,
    framing: &mut Answer,
    line_limit: usize,
) -> io::Result<Option<Vec<u8>>> {
    let limit = |left: u64| usize::try_from(left).unwrap_or(usize::MAX).min(RELAY_CHUNK);
    match framing {
        Answer::Length(0) => Ok(None),
        Answer::Length(left) => {
            let piece = input.take(limit(*left))?;
            if piece.is_empty() {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            *left -= u64::try_from(piece.len()).unwrap_or(*left);
            Ok(Some(piece))
        }
        Answer::Chunked { left, started } => {
            if *left == 0 {
                if *started {
                    input.expect_crlf()?;
                }
                *started = true;
                *left = chunk_size(&input.line(line_limit)?)?;
                if *left == 0 {
                    while !input.line(line_limit)?.is_empty() {}
                    *framing = Answer::Length(0);
                    return Ok(None);
                }
            }
            let piece = input.take(limit(*left))?;
            if piece.is_empty() {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            *left -= u64::try_from(piece.len()).unwrap_or(*left);
            Ok(Some(piece))
        }
        Answer::Close => {
            let piece = input.take(RELAY_CHUNK)?;
            Ok((!piece.is_empty()).then_some(piece))
        }
    }
}

/// What a relay needs from the listening surface.
pub(crate) struct Limits {
    pub(crate) line_bytes: usize,
    pub(crate) timeout: Duration,
}

/// Relays one admitted request. A refusal is returned for the caller to write; once the
/// answer's head has been written, nothing more is refused.
pub(crate) fn serve(
    stream: &mut TcpStream,
    leftover: Vec<u8>,
    admitted: Admitted,
    relay: &Relay,
    limits: &Limits,
) -> Result<(), RefusalCode> {
    let deadline = Instant::now() + limits.timeout;
    if admitted.expects_continue
        && !crate::server::write_by(stream, b"HTTP/1.1 100 Continue\r\n\r\n", deadline)
    {
        return Err(RefusalCode::RequestMalformed);
    }
    let body = {
        let mut input = Buffered {
            source: Source::Client(stream, deadline),
            buffer: leftover,
            start: 0,
        };
        read_body(&mut input, admitted.framing, limits.line_bytes)?
    };
    let analysis = body::analyse(&body)?;
    let model = relay
        .models
        .get(&analysis.model)
        .ok_or(RefusalCode::ModelUnknown)?;
    if !model.wires.contains(&admitted.wire) {
        return Err(RefusalCode::WireNotServed);
    }
    let rewritten = analysis.rewrite(
        &body,
        model.upstream_model.as_str(),
        admitted.wire == Wire::Messages,
    );
    drop(body);
    let alias = model.alias.as_str();
    let target = relay
        .targets
        .acquire(alias)
        .ok_or(RefusalCode::TargetUnavailable)?;
    let failed = || {
        relay.targets.invalidate(alias, target.authority());
        RefusalCode::UpstreamFailed
    };
    let mut connection = target.connect().map_err(|_| failed())?;
    let request = format!(
        "POST {} HTTP/1.1\r\nhost: {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        admitted.wire.path(),
        target.authority(),
        rewritten.len()
    );
    let sent = connection
        .write_all(request.as_bytes())
        .and_then(|()| connection.write_all(&rewritten))
        .and_then(|()| connection.flush());
    drop(rewritten);
    if sent.is_err() {
        return Err(failed());
    }
    let mut input = Buffered {
        source: Source::Target(connection.as_mut()),
        buffer: Vec::new(),
        start: 0,
    };
    let Ok(mut head) = read_answer_head(&mut input, limits.line_bytes) else {
        return Err(failed());
    };
    // A gateway status from the target means the pod behind it is gone or unreachable; a
    // model error arrives as 4xx or 500 instead (llmgw `src/lib.rs:629-643`).
    if matches!(head.status, 502..=504) {
        return Err(failed());
    }
    stream_answer(stream, &mut input, &mut head, limits);
    // The target is released only now, after the last byte: row L12's hold.
    drop(input);
    drop(connection);
    drop(target);
    Ok(())
}

/// Writes the answer's head, then each decoded piece as one chunk as it arrives. A failure on
/// either side ends the answer without its last chunk, so the client sees it cut short.
fn stream_answer(
    stream: &mut TcpStream,
    input: &mut Buffered<'_>,
    head: &mut Head,
    limits: &Limits,
) {
    let phrase = crate::server::relayed_phrase(head.status);
    let content_type = head
        .content_type
        .as_ref()
        .map_or(String::new(), |value| format!("content-type: {value}\r\n"));
    let start = format!(
        "HTTP/1.1 {} {phrase}\r\n{content_type}cache-control: no-store\r\nconnection: close\r\ntransfer-encoding: chunked\r\n\r\n",
        head.status
    );
    if !crate::server::write_by(stream, start.as_bytes(), Instant::now() + limits.timeout) {
        return;
    }
    loop {
        match next_piece(input, &mut head.framing, limits.line_bytes) {
            Ok(Some(piece)) => {
                let mut framed = format!("{:x}\r\n", piece.len()).into_bytes();
                framed.extend_from_slice(&piece);
                framed.extend_from_slice(b"\r\n");
                // Each write has its own deadline: a stream may last as long as the model
                // talks, but a client that stops reading cannot hold the connection.
                if !crate::server::write_by(stream, &framed, Instant::now() + limits.timeout) {
                    return;
                }
            }
            Ok(None) => {
                crate::server::write_by(stream, b"0\r\n\r\n", Instant::now() + limits.timeout);
                return;
            }
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Framing, MAX_REQUEST_BODY_BYTES, Relay, RelayModel, RelayTargets, Wire, framing};
    use crate::{error::RefusalCode, error::RelayError, inventory::Label};
    use std::sync::Arc;

    struct NoTargets;

    impl RelayTargets for NoTargets {
        fn acquire(&self, _alias: &str) -> Option<Box<dyn super::RelayTarget>> {
            None
        }

        fn invalidate(&self, _alias: &str, _authority: &str) {}
    }

    fn model(alias: &str, wires: Vec<Wire>) -> Result<RelayModel, RelayError> {
        RelayModel::new(
            Label::new(alias).unwrap(),
            Label::new("served").unwrap(),
            wires,
        )
    }

    #[test]
    fn every_relay_error_has_a_composition_that_provokes_it() {
        for expected in RelayError::ALL {
            let observed = match expected {
                RelayError::NoWire => model("a", Vec::new()).unwrap_err(),
                RelayError::RepeatedWire => {
                    model("a", vec![Wire::Messages, Wire::Messages]).unwrap_err()
                }
                RelayError::DuplicateModel => Relay::new(
                    vec![
                        model("a", vec![Wire::Chat]).unwrap(),
                        model("a", vec![Wire::Responses]).unwrap(),
                    ],
                    Arc::new(NoTargets),
                )
                .unwrap_err(),
            };
            assert_eq!(observed, *expected);
        }
    }

    #[test]
    fn every_wire_is_found_by_its_own_path_and_no_other() {
        for wire in Wire::ALL {
            assert_eq!(Wire::from_path(wire.path()), Some(wire));
        }
        for path in ["/v1/chat", "/v1/messages/", "/v1/routes", "/v1/completions"] {
            assert_eq!(Wire::from_path(path), None, "{path}");
        }
    }

    #[test]
    fn the_body_framing_is_read_from_the_head_alone() {
        assert_eq!(framing(None, None), Ok(Framing::Length(0)));
        assert_eq!(
            framing(Some("33554432"), None),
            Ok(Framing::Length(33_554_432))
        );
        assert_eq!(
            framing(Some("33554433"), None),
            Err(RefusalCode::BodyTooLarge)
        );
        assert_eq!(
            framing(Some("99999999999999999999999"), None),
            Err(RefusalCode::BodyTooLarge)
        );
        assert_eq!(framing(None, Some("Chunked")), Ok(Framing::Chunked));
        for (length, coding) in [
            (Some("-1"), None),
            (Some("1 2"), None),
            (Some(""), None),
            (Some("+5"), None),
            (None, Some("gzip")),
            (None, Some("gzip, chunked")),
            (Some("5"), Some("chunked")),
        ] {
            assert_eq!(
                framing(length, coding),
                Err(RefusalCode::RequestMalformed),
                "{length:?} {coding:?}"
            );
        }
        assert_eq!(MAX_REQUEST_BODY_BYTES, 32 * 1024 * 1024);
    }
}
