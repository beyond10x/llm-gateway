//! The readiness probe: `GET <endpoint>models` to the pod's vLLM server, with the model's vLLM
//! key as a bearer (`llm-gateway.runpod.ProbeAnswer`).
//!
//! It speaks plain HTTP/1.1 over a `std` socket and nothing else. An `https://` endpoint, which
//! is what llm's Runpod description builds for a live pod, answers `Unreachable`: this crate has
//! no TLS client, and giving the binary one is `story:pod-proxy-tls`'s.

use std::{
    fmt,
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    time::{Duration, Instant},
};

use serde_json::Value;

use crate::Probe;

/// The most of an answer the probe reads.
const MAX_ANSWER_BYTES: usize = 1024 * 1024;

/// One model's vLLM key. Redacted in `Debug`, and overwritten when dropped.
pub(super) struct BearerKey(Vec<u8>);

impl BearerKey {
    pub(super) fn new(key: Vec<u8>) -> Self {
        Self(key)
    }

    /// The key, if it is one printable ASCII token a header can carry.
    fn token(&self) -> Option<&str> {
        let text = std::str::from_utf8(&self.0).ok()?;
        (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_graphic())).then_some(text)
    }
}

impl fmt::Debug for BearerKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BearerKey([REDACTED])")
    }
}

impl Drop for BearerKey {
    fn drop(&mut self) {
        self.0.fill(0);
        std::hint::black_box(&self.0);
    }
}

/// Probes `<endpoint>models`. `200` is `Ready` with the served model names, `401`/`403`
/// `Refused`, any other answer `NotReady`, and no answer `Unreachable`.
pub(super) fn models(endpoint: &str, key: Option<&BearerKey>, timeout: Duration) -> Probe {
    let Some((authority, path)) = endpoint
        .strip_prefix("http://")
        .and_then(|rest| rest.split_once('/'))
    else {
        return Probe::Unreachable;
    };
    if authority.is_empty() || authority.contains('@') {
        return Probe::Unreachable;
    }
    let mut request = format!(
        "GET /{path}models HTTP/1.1\r\nHost: {authority}\r\nAccept: application/json\r\nConnection: close\r\n"
    );
    if let Some(key) = key {
        let Some(token) = key.token() else {
            return Probe::Unreachable;
        };
        request.push_str("Authorization: Bearer ");
        request.push_str(token);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    let answer = exchange(authority, request.as_bytes(), timeout);
    request.clear();
    let Some(answer) = answer else {
        return Probe::Unreachable;
    };
    match parse(&answer) {
        None => Probe::Unreachable,
        Some((200, body)) => served(&body).map_or(Probe::NotReady, |served_models| Probe::Ready {
            served_models,
        }),
        Some((401 | 403, _)) => Probe::Refused,
        Some(_) => Probe::NotReady,
    }
}

/// Sends the request and reads the answer to its end, all inside the timeout.
fn exchange(authority: &str, request: &[u8], timeout: Duration) -> Option<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let address = if authority.contains(':') {
        authority.to_owned()
    } else {
        format!("{authority}:80")
    };
    let mut stream = address
        .to_socket_addrs()
        .ok()?
        .find_map(|address| TcpStream::connect_timeout(&address, timeout).ok())?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    stream.write_all(request).ok()?;
    let mut answer = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        stream
            .set_read_timeout(Some(left.max(Duration::from_millis(1))))
            .ok()?;
        match stream.read(&mut buffer) {
            Ok(0) => return Some(answer),
            Ok(read) => {
                answer.extend_from_slice(buffer.get(..read)?);
                if answer.len() > MAX_ANSWER_BYTES {
                    return None;
                }
                if complete(&answer) {
                    return Some(answer);
                }
            }
            Err(_) => return None,
        }
    }
}

fn head_end(answer: &[u8]) -> Option<usize> {
    answer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|at| at + 4)
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then_some(value.trim())
    })
}

/// Whether a `Content-Length` answer has all its bytes; a chunked or unsized one ends at EOF.
fn complete(answer: &[u8]) -> bool {
    let Some(end) = head_end(answer) else {
        return false;
    };
    let head = String::from_utf8_lossy(answer.get(..end).unwrap_or_default());
    header(&head, "content-length")
        .and_then(|length| length.parse::<usize>().ok())
        .is_some_and(|length| answer.len().saturating_sub(end) >= length)
}

/// The status code and the body of an HTTP/1.1 answer.
fn parse(answer: &[u8]) -> Option<(u16, Vec<u8>)> {
    let end = head_end(answer)?;
    let head = std::str::from_utf8(answer.get(..end)?).ok()?;
    let status: u16 = head
        .lines()
        .next()?
        .strip_prefix("HTTP/1.")?
        .get(2..)?
        .split(' ')
        .next()?
        .parse()
        .ok()?;
    let rest = answer.get(end..)?;
    let body = if header(head, "transfer-encoding")
        .is_some_and(|coding| coding.eq_ignore_ascii_case("chunked"))
    {
        dechunk(rest)?
    } else if let Some(length) = header(head, "content-length") {
        rest.get(..length.parse::<usize>().ok()?)?.to_vec()
    } else {
        rest.to_vec()
    };
    Some((status, body))
}

fn dechunk(mut rest: &[u8]) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let line_end = rest.windows(2).position(|window| window == b"\r\n")?;
        let size_text = std::str::from_utf8(rest.get(..line_end)?).ok()?;
        let size = usize::from_str_radix(size_text.split(';').next()?.trim(), 16).ok()?;
        rest = rest.get(line_end + 2..)?;
        if size == 0 {
            return Some(body);
        }
        body.extend_from_slice(rest.get(..size)?);
        rest = rest.get(size + 2..)?;
    }
}

/// The `data[].id` names of an OpenAI-style model list.
fn served(body: &[u8]) -> Option<Vec<String>> {
    let list: Value = serde_json::from_slice(body).ok()?;
    list.get("data")?
        .as_array()?
        .iter()
        .map(|model| model.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect()
}
