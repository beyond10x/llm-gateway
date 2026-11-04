//! The passthrough of a client-credential model (story:subscription-passthrough).
//!
//! A model marked [`RelayModel::with_client_credentials`], behind a gateway that authenticates
//! nobody, is sent the request as the client sent it: its request target with the query, every
//! header but the [`UNFORWARDED_HEADERS`](super::UNFORWARDED_HEADERS) and the ones the client's
//! `connection` line names, and its body, rewritten only where the route resolves an alias or
//! caps the output. The answer reaches the client the same way: its status line, every header
//! but the hop-by-hop ones, and its body, re-chunked only when the target chunked it.
//!
//! A 429, 502, 503 or 529 answer is retried while no answer byte has reached the client: after
//! the target's `retry-after` (seconds or an HTTP date), else after an exponential backoff with
//! jitter, never when the target says `x-should-retry: false`, and only within the model's
//! attempt and wait budget. When the budget is spent the last answer is passed through as it
//! came. A client that sends `x-stainless-retry-count` retries on its own, so its calls are
//! never retried here: one attempt per client attempt. Every attempt is recorded in the call.
//!
//! A content-encoded answer still reaches the client byte for byte; a copy of it is decoded
//! ([`BodyDecoder`]) for the usage and for the error body a record keeps.

use super::{
    Admitted, Answer, Buffered, Call, ClientCredentials, Head, Limits, MAX_RELAYED_BYTES,
    MAX_RELAYED_VALUE, RelayModel, RelayTarget, RequestHead, Source, client_closed,
    connection_names, hop_by_hop, next_piece, relays_answer_line, target_head, write_request,
};
use crate::{
    dashboard::{Attempt, credential_label},
    error::RefusalCode,
    usage::UsageScanner,
};
use std::{
    hash::BuildHasher,
    net::TcpStream,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// How many attempts a passthrough call makes at most, unless its model says otherwise.
pub const DEFAULT_RETRY_ATTEMPTS: u32 = 4;
/// How long a passthrough call waits for retries in all, unless its model says otherwise.
pub const DEFAULT_RETRY_WAIT: Duration = Duration::from_secs(60);
/// The longest backoff between two attempts when the target names no `retry-after`.
const BACKOFF_CAP: Duration = Duration::from_secs(30);
/// The answer statuses a passthrough call is retried on.
const RETRIED: [u16; 4] = [429, 502, 503, 529];
/// The most bytes of a non-2xx answer's body a call record keeps.
pub const MAX_ERROR_BODY: usize = 2048;
/// The most bytes of an encoded answer kept to decode for its usage.
const MAX_ENCODED_COPY: usize = 16 << 20;
/// The most bytes a decoded copy of an answer may grow to.
const MAX_DECODED_COPY: usize = 64 << 20;
/// The most bytes of an encoded error body read to decode its start.
const MAX_ENCODED_ERROR: usize = 64 << 10;
/// The request line by which a client says it retries on its own.
const CLIENT_RETRY_LINE: &str = "x-stainless-retry-count";
/// How often a call waiting to retry looks whether its client is still there.
const WAIT_SLICE: Duration = Duration::from_millis(100);

/// How a client-credential model's call is retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Retry {
    pub(super) max_attempts: u32,
    pub(super) max_wait: Duration,
}

impl Default for Retry {
    fn default() -> Self {
        Self {
            max_attempts: DEFAULT_RETRY_ATTEMPTS,
            max_wait: DEFAULT_RETRY_WAIT,
        }
    }
}

/// What a [`BodyDecoder`] made of an encoded body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Decoded {
    /// The decoded bytes, at most the limit asked for.
    pub bytes: Vec<u8>,
    /// The whole body was decoded without an error and within the limit.
    pub complete: bool,
}

/// Decodes a copy of a content-encoded answer body. The gateway crate holds no decoder of its
/// own; the embedding supplies one ([`super::Relay::with_decoder`]).
pub trait BodyDecoder: Send + Sync {
    /// `encoded` decoded by `coding`, the answer's `content-encoding` value as sent, at most
    /// `limit` bytes of it; `None` for a coding it does not know.
    fn decode(&self, coding: &str, encoded: &[u8], limit: usize) -> Option<Decoded>;
}

/// Relays one passthrough call, retrying it as [`Retry`] allows. `failed` invalidates the
/// target and answers the refusal a call without an answer head gets.
pub(super) fn relay(
    stream: &mut TcpStream,
    admitted: &Admitted,
    (model, target): (&RelayModel, &dyn RelayTarget),
    (body, forwarded): (&[u8], &ClientCredentials),
    limits: &Limits,
    call: &mut Call,
    failed: &dyn Fn() -> RefusalCode,
) -> Result<(), RefusalCode> {
    let head = request_head(target, &admitted.request_target, body.len(), forwarded);
    call.credential = forwarded
        .value("authorization")
        .or_else(|| forwarded.value("x-api-key"))
        .map(credential_label);
    let retry = if forwarded.value(CLIENT_RETRY_LINE).is_some() {
        Retry {
            max_attempts: 1,
            ..model.retry
        }
    } else {
        model.retry
    };
    let mut waited = Duration::ZERO;
    let mut number = 0_u32;
    loop {
        number += 1;
        let began = Instant::now();
        let mut attempt = Attempt {
            number,
            ..Attempt::default()
        };
        let finish = |mut attempt: Attempt, call: &mut Call| {
            attempt.duration_ms = millis(began.elapsed());
            call.attempts.push(attempt);
        };
        let Ok(mut connection) = target.connect() else {
            finish(attempt, call);
            return Err(failed());
        };
        if number == 1 {
            call.queue_wait_ms = call.since_begun();
        }
        if write_request(connection.as_mut(), &head, body).is_err() {
            finish(attempt, call);
            return Err(failed());
        }
        let mut input = Buffered {
            source: Source::Target(connection.as_mut()),
            buffer: Vec::new(),
            start: 0,
        };
        // A client gone before anything was written to it: no one is left to answer.
        let mut answer = match target_head(stream, &mut input, limits, call) {
            Ok(Some(answer)) => answer,
            Ok(None) => {
                call.client_disconnected = true;
                finish(attempt, call);
                return Ok(());
            }
            Err(()) => {
                finish(attempt, call);
                return Err(failed());
            }
        };
        attempt.status = Some(answer.status);
        attempt.retry_after = value(&answer, "retry-after");
        attempt.headers = recorded_headers(&answer);
        call.target_status = Some(answer.status);
        if answer.status == 429 || answer.status >= 500 {
            call.retry_after.clone_from(&attempt.retry_after);
        }
        let Some(wait) = next_wait(&answer, number, retry, waited) else {
            let error = !(200..300).contains(&answer.status);
            attempt.error_body = pass_answer(
                stream,
                &mut input,
                &mut answer,
                limits,
                admitted.chunked_answer,
                call,
                error,
            );
            finish(attempt, call);
            return Ok(());
        };
        attempt.error_body = Some(error_body(&mut input, &mut answer, limits));
        attempt.wait_ms = millis(wait);
        finish(attempt, call);
        drop(input);
        drop(connection);
        waited += wait;
        if !wait_for_retry(stream, wait, call, number + 1) {
            call.client_disconnected = true;
            return Ok(());
        }
    }
}

/// The head of a passthrough request: `POST` at the client's request target (under the
/// target's base path when it has one), the target's `host`, the target's static headers but
/// a credential, the client's forwarded lines, and the gateway's own `content-length` and
/// `connection: close`.
fn request_head(
    target: &dyn RelayTarget,
    request_target: &str,
    length: usize,
    forwarded: &ClientCredentials,
) -> RequestHead {
    let path = match target.base_path() {
        Some(base) => match request_target.split_once('?') {
            Some((path, query)) => format!("{}?{query}", base.join(path)),
            None => base.join(request_target),
        },
        None => request_target.to_owned(),
    };
    let mut start = format!("POST {path} HTTP/1.1\r\nhost: {}\r\n", target.authority());
    for header in target.headers() {
        if matches!(header.name(), "authorization" | "x-api-key") {
            continue;
        }
        start.push_str(header.name());
        start.push_str(": ");
        start.push_str(header.value());
        start.push_str("\r\n");
    }
    let end = format!("content-length: {length}\r\nconnection: close\r\n\r\n");
    let client = forwarded.as_bytes();
    let mut head = RequestHead(Vec::with_capacity(start.len() + client.len() + end.len()));
    head.0.extend_from_slice(start.as_bytes());
    head.0.extend_from_slice(client);
    head.0.extend_from_slice(end.as_bytes());
    head
}

/// The value of the answer line `name`, trimmed, when it is UTF-8.
fn value(head: &Head, name: &str) -> Option<String> {
    head.lines
        .iter()
        .find(|(line_name, _)| line_name == name)
        .and_then(|(_, line)| {
            let colon = line.iter().position(|&byte| byte == b':')?;
            std::str::from_utf8(&line[colon + 1..])
                .ok()
                .map(|value| value.trim().to_owned())
        })
}

/// The answer lines an attempt records ([`relays_answer_line`]), each value short printable
/// ASCII, at most [`MAX_RELAYED_BYTES`] in all.
fn recorded_headers(head: &Head) -> Vec<(String, String)> {
    let mut used = 0_usize;
    let mut out = Vec::new();
    for (name, _) in &head.lines {
        if !relays_answer_line(name) || out.iter().any(|(known, _)| known == name) {
            continue;
        }
        let Some(value) = value(head, name).filter(|value| {
            value.len() <= MAX_RELAYED_VALUE
                && value.bytes().all(|byte| (0x20..0x7f).contains(&byte))
        }) else {
            continue;
        };
        used += name.len() + value.len();
        if used > MAX_RELAYED_BYTES {
            break;
        }
        out.push((name.clone(), value));
    }
    out
}

/// How long to wait before attempt `number + 1`, or `None` when the answer is final: not a
/// retried status, `x-should-retry: false`, the attempts spent, or a wait past the budget.
fn next_wait(head: &Head, number: u32, retry: Retry, waited: Duration) -> Option<Duration> {
    if !RETRIED.contains(&head.status) || number >= retry.max_attempts {
        return None;
    }
    if value(head, "x-should-retry").is_some_and(|value| value.eq_ignore_ascii_case("false")) {
        return None;
    }
    let wait = value(head, "retry-after")
        .and_then(|value| retry_after(&value, SystemTime::now()))
        .unwrap_or_else(|| backoff(number));
    (waited + wait <= retry.max_wait).then_some(wait)
}

/// The wait a `retry-after` value asks for: whole seconds, or an IMF-fixdate (RFC 9110
/// section 5.6.7) from `now`; `None` for a value that is neither.
fn retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.parse().ok().map(Duration::from_secs);
    }
    let at = http_date(value)?;
    let now = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(Duration::from_secs(at.saturating_sub(now)))
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The seconds since the Unix epoch of an IMF-fixdate, `Sun, 06 Nov 1994 08:49:37 GMT`.
fn http_date(value: &str) -> Option<u64> {
    let parts: Vec<&str> = value.split_whitespace().collect();
    let [_, day, month, year, time, "GMT"] = parts.as_slice() else {
        return None;
    };
    let month = i64::try_from(MONTHS.iter().position(|known| known == month)? + 1).ok()?;
    let day: i64 = day.parse().ok()?;
    let year: i64 = year.parse().ok()?;
    let mut clock = time.split(':').map(str::parse::<i64>);
    let (Some(Ok(hours)), Some(Ok(minutes)), Some(Ok(seconds)), None) =
        (clock.next(), clock.next(), clock.next(), clock.next())
    else {
        return None;
    };
    // Days from the civil date (H. Hinnant's algorithm).
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let of_era = shifted - era * 400;
    let of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let of_cycle = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    let days = era * 146_097 + of_cycle - 719_468;
    u64::try_from(days * 86_400 + hours * 3600 + minutes * 60 + seconds).ok()
}

/// The backoff before attempt `number + 1`: 1 s, 2 s, 4 s, 8 s and so on, plus up to a fifth
/// more as jitter, at most [`BACKOFF_CAP`].
fn backoff(number: u32) -> Duration {
    let base = Duration::from_secs(1_u64 << number.saturating_sub(1).min(16));
    let jitter_ms =
        std::collections::hash_map::RandomState::new().hash_one(number) % (millis(base) / 5).max(1);
    (base + Duration::from_millis(jitter_ms)).min(BACKOFF_CAP)
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Waits `wait` before attempt `next`, shown in flight as waiting to retry; `false` when the
/// client went away meanwhile.
fn wait_for_retry(stream: &TcpStream, wait: Duration, call: &Call, next: u32) -> bool {
    let until = Instant::now() + wait;
    if let Some(progress) = &call.progress {
        progress.retrying(next, Some(crate::dashboard::now_ms() + millis(wait)));
    }
    let mut there = true;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        std::thread::sleep(left.min(WAIT_SLICE));
        if client_closed(stream) {
            there = false;
            break;
        }
    }
    if let Some(progress) = &call.progress {
        progress.retrying(next, None);
    }
    there
}

/// The start of a retried answer's body for its record, read at most [`MAX_ERROR_BODY`]
/// bytes.
fn error_body(input: &mut Buffered<'_>, head: &mut Head, limits: &Limits) -> String {
    let cap = if coding(head).is_some() {
        MAX_ENCODED_ERROR
    } else {
        MAX_ERROR_BODY
    };
    let mut kept = Vec::new();
    while kept.len() < cap {
        match next_piece(input, &mut head.framing, limits.line_bytes) {
            Ok(Some(piece)) => kept.extend_from_slice(&piece),
            _ => break,
        }
    }
    describe_body(head, &kept, limits)
}

/// The answer's content coding, unless it is none or `identity`.
fn coding(head: &Head) -> Option<String> {
    value(head, "content-encoding").filter(|coding| !coding.eq_ignore_ascii_case("identity"))
}

/// What a record keeps of an answer body that starts `kept`: its first [`MAX_ERROR_BODY`]
/// bytes with every token-like string redacted, a compressed one decoded first; only its
/// coding when it cannot be decoded.
fn describe_body(head: &Head, kept: &[u8], limits: &Limits) -> String {
    let decoded;
    let kept = match coding(head) {
        None => kept,
        Some(coding) => {
            match limits
                .decoder
                .as_ref()
                .and_then(|decoder| decoder.decode(&coding, kept, MAX_ERROR_BODY))
                .filter(|decoded| !decoded.bytes.is_empty())
            {
                Some(found) => {
                    decoded = found.bytes;
                    &decoded
                }
                None => return format!("[{coding}-encoded body, undecodable]"),
            }
        }
    };
    let end = kept.len().min(MAX_ERROR_BODY);
    redact(&String::from_utf8_lossy(&kept[..end]))
}

/// `text` with every token-like run replaced by `[redacted]`: one starting `sk-`, or one of 32
/// or more letters, digits and `-_+/=` holding both letters and digits. A request id
/// (`req_…`) is kept.
pub(crate) fn redact(text: &str) -> String {
    let token_like = |run: &str| {
        (run.starts_with("sk-") && run.len() >= 8)
            || (run.len() >= 32
                && !run.starts_with("req_")
                && run.bytes().any(|byte| byte.is_ascii_digit())
                && run.bytes().any(|byte| byte.is_ascii_alphabetic()))
    };
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    for character in text.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '+' | '/' | '=') {
            run.push(character);
            continue;
        }
        out.push_str(if token_like(&run) { "[redacted]" } else { &run });
        run.clear();
        out.push(character);
    }
    out.push_str(if token_like(&run) { "[redacted]" } else { &run });
    out
}

/// Writes the target's answer to the client as it came: the status line, every line but the
/// hop-by-hop ones (and `content-length` unless the target framed its body by length), then
/// the body, re-chunked for an HTTP/1.1 client when the target chunked it. Fills `call` as
/// [`super::stream_answer`] does; for an `error` answer, answers what its record keeps of the
/// body.
fn pass_answer(
    stream: &mut TcpStream,
    input: &mut Buffered<'_>,
    head: &mut Head,
    limits: &Limits,
    chunked_client: bool,
    call: &mut Call,
    error: bool,
) -> Option<String> {
    let connection = connection_names(value(head, "connection").as_deref());
    let encoded = coding(head);
    let by_length = matches!(head.framing, Answer::Length(_));
    let rechunk = matches!(head.framing, Answer::Chunked { .. }) && chunked_client;
    let mut out = format!("HTTP/1.1 {} {}\r\n", head.status, head.reason).into_bytes();
    for (name, line) in &head.lines {
        let kept = if name == "content-length" {
            by_length
        } else {
            !hop_by_hop(name, &connection)
        };
        if kept {
            out.extend_from_slice(line);
            out.extend_from_slice(b"\r\n");
        }
    }
    if rechunk {
        out.extend_from_slice(b"transfer-encoding: chunked\r\n");
    }
    out.extend_from_slice(b"connection: close\r\n\r\n");
    let deadline = || Instant::now() + limits.timeout;
    if !crate::server::write_by(stream, &out, deadline()) {
        call.client_disconnected = true;
        return error.then(|| describe_body(head, &[], limits));
    }
    let mut scanner = UsageScanner::default();
    let mut kept = Vec::new();
    // A copy of an encoded answer, `None` once it outgrew its bound.
    let mut copy = encoded.as_ref().map(|_| Vec::new());
    let mut written = 0_u64;
    loop {
        match next_piece(input, &mut head.framing, limits.line_bytes) {
            Ok(Some(piece)) => {
                let size = u64::try_from(piece.len()).unwrap_or(u64::MAX);
                match &mut copy {
                    _ if encoded.is_none() => scanner.feed(&piece),
                    Some(bytes) if bytes.len() + piece.len() <= MAX_ENCODED_COPY => {
                        bytes.extend_from_slice(&piece);
                    }
                    _ => copy = None,
                }
                if error && encoded.is_none() && kept.len() < MAX_ERROR_BODY {
                    kept.extend_from_slice(&piece[..piece.len().min(MAX_ERROR_BODY - kept.len())]);
                }
                let framed = if rechunk {
                    let mut framed = format!("{:x}\r\n", piece.len()).into_bytes();
                    framed.extend_from_slice(&piece);
                    framed.extend_from_slice(b"\r\n");
                    framed
                } else {
                    piece
                };
                if !crate::server::write_by(stream, &framed, deadline()) {
                    call.client_disconnected = true;
                    break;
                }
                call.first_byte();
                written = written.saturating_add(size);
            }
            Ok(None) => {
                if rechunk {
                    crate::server::write_by(stream, b"0\r\n\r\n", deadline());
                }
                break;
            }
            Err(_) => break,
        }
    }
    call.response_bytes = written;
    if let Some(coding) = &encoded {
        let decoded = copy.as_ref().and_then(|bytes| {
            limits
                .decoder
                .as_ref()?
                .decode(coding, bytes, MAX_DECODED_COPY)
        });
        match decoded {
            Some(decoded) => {
                scanner.feed(&decoded.bytes);
                if !decoded.complete && !call.client_disconnected {
                    call.usage_note = Some(format!("undecodable({coding})"));
                }
                if error {
                    kept = decoded.bytes;
                    kept.truncate(MAX_ERROR_BODY);
                }
            }
            None => call.usage_note = Some(format!("undecodable({coding})")),
        }
    }
    call.usage = scanner.usage();
    if error && encoded.is_some() && !kept.is_empty() {
        return Some(redact(&String::from_utf8_lossy(&kept)));
    }
    error.then(|| describe_body(head, &kept, limits))
}

#[cfg(test)]
mod tests {
    use super::{backoff, http_date, redact, retry_after};
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn a_retry_after_is_seconds_or_an_http_date() {
        assert_eq!(
            http_date("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(784_111_777)
        );
        let now = UNIX_EPOCH + Duration::from_secs(784_111_770);
        assert_eq!(
            retry_after("Sun, 06 Nov 1994 08:49:37 GMT", now),
            Some(Duration::from_secs(7))
        );
        assert_eq!(retry_after("12", now), Some(Duration::from_secs(12)));
        assert_eq!(retry_after("soon", now), None);
        assert_eq!(retry_after("-1", now), None);
    }

    #[test]
    fn the_backoff_doubles_with_jitter_up_to_its_cap() {
        for (number, base) in [(1, 1_000), (2, 2_000), (3, 4_000), (4, 8_000)] {
            let wait = u64::try_from(backoff(number).as_millis()).unwrap_or(u64::MAX);
            assert!((base..=base + base / 5).contains(&wait), "{number}: {wait}");
        }
        assert_eq!(backoff(9), Duration::from_secs(30));
    }

    #[test]
    fn a_token_like_string_is_redacted_and_a_request_id_kept() {
        let text = "{\"error\":\"bad key sk-ant-oat01-abc\",\"request_id\":\"req_011CUabcdefghijklmnopqrstuvwxyz0123\",\"t\":\"Abc123def456ghi789jkl012mno345pqr678\"}";
        let out = redact(text);
        assert!(!out.contains("sk-ant"), "{out}");
        assert!(
            out.contains("req_011CUabcdefghijklmnopqrstuvwxyz0123"),
            "{out}"
        );
        assert!(!out.contains("Abc123def456"), "{out}");
        assert!(out.contains("\"error\":\"bad key [redacted]\""), "{out}");
    }
}
