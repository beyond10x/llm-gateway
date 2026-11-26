//! The direct Runpod transport: Runpod's REST API v2, called by this process with an API key its
//! embedding hands in.
//!
//! [`ConnectorsRunpod`](super::ConnectorsRunpod) never holds the Runpod key; this transport does,
//! for a gateway on an operator's own machine whose process reads the key through an OS keychain
//! reference. The key is held as a [`BearerKey`], redacted in `Debug` and overwritten when
//! dropped, and written only into a [`Request`] buffer that is overwritten when dropped. No
//! answer, error or `Debug` text carries it.
//!
//! It speaks a minimal HTTP/1.1: one request per connection (`Connection: close`), an answer
//! read to its `Content-Length` or to its end, chunked answers decoded, at most
//! [`MAX_ANSWER_BYTES`] read. An `https://` URL is reached over the [`PodTls`] connection the
//! embedding hands in ([`RestRunpod::set_tls`]), the crate linking no TLS client; without one,
//! no call is made. An `http://` URL is accepted only for a loopback host, which only a test
//! fixture is.
//!
//! The calls (Runpod REST API v2, `https://api.runpod.io/v2`, the documented route of
//! 2026-11-26):
//!
//! * `list_pods`: `GET /pods`, `{"pods": [...], "pagination": {...}}`, followed page by page
//!   through `?cursor=<pagination.nextCursor>` while `pagination.hasNextPage`, at most
//!   [`MAX_PAGES`] pages. A first page that fails is a listing that failed outright (`Err`); a
//!   later one leaves the listing partial. Each entry is a v2 `Pod`: `id`, `name`, `status`,
//!   `env` and `gpu.id`. The listing applies only its `filters` parameter, which matches tags,
//!   so none is sent.
//! * `create_pod`: `POST /pods` with v2's `CreatePodRequest`: `name`, `image`, one GPU type as
//!   `gpu: {id, count}`, `cloud`, `disk`, `ports`, `env`, `entrypoint`, `dataCenterIds` and a
//!   network volume as `mounts.network`; v2 refuses any other key (422). A 2xx is `Created`; a
//!   4xx whose answer says Runpod has no instance available is `NoCapacity` (v2 answers capacity
//!   with 400 and only a human-readable `detail`), any other 4xx `Refused`. No answer, a 5xx or a timeout
//!   may have created the pod: one listing resolves it by its name and request tag, as
//!   connectors' `unknown` is resolved; otherwise it is `Lost`. A create is never sent twice.
//! * `terminate_pod`: `DELETE /pods/<id>`. A 2xx is `Terminated`, a 4xx `Refused`, anything
//!   else `Lost`.
//! * `container_started_at`: the `ts` of the last `start container for <image>: begin` line of
//!   the pod's system log, `GET /pods/<id>/logs?source=system&tail=100`. Not the pod's
//!   `startedAt`: on the live API that stays put while the container restarts (2026-11-26, a vLLM
//!   engine failing again and again), so the crash-loop rule never fired. Nor `runtime.uptime`
//!   of `GET /pods/<id>`, the one single answer that might reveal a restart: no restart has been
//!   seen to reset it, while the system log's start line was seen at each one. The log is a
//!   Server-Sent Events stream that stays open, so one read ends when the stream has been quiet
//!   for [`RestBinding::log_quiet`], after 5 s or after 1 MiB, whichever
//!   comes first. A pod's log is read at most once per [`RestBinding::log_every`], and not while
//!   its last probe found it serving: a restart of a serving container shows first as a failed
//!   probe. Each read is made under the pool's lock, so these bounds keep it off the relayed
//!   requests.
//! * `pod_placement`: the pod's `cloud` from the listing. v2 declares no `interruptible` and no
//!   way to ask for one, so a placement never reports it.
//! * `account_spend`: `GET /billing/pods?bucketSize=day`, an object whose `records` are one
//!   pod's one day each: `totalAmount` (USD) and the bucket's `startTime`. It covers Runpod's
//!   default window, the 30 days up to now, so the total is those 30 days' spend.
//! * [`RunpodAccount::account`]: the pods, the balance from `GET /billing/balance`
//!   (`balanceAmount`) and the spend rate from Runpod's GraphQL `myself { currentSpendPerHr }`.
//!   On a 403 or 404 balance (a key without `billing:read`, an organization without a billing
//!   account) the balance is GraphQL's `clientBalance`, read in the same query.
//!
//! Each failed call is kept as the transport's [`last failure`](RestRunpod::last_failure): the
//! host it went to and the HTTP status or the connection's own error, never the key.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use serde_json::{Map, Value, json};

use super::{
    connectors::started_at_ms,
    probe::{self, BearerKey, PodTls, Request},
};
use crate::{
    AccountSnapshot, AccountSpend, CreateAnswer, Pod, PodListing, PodPlacement, PodRequest,
    PodStatus, PodSummary, Probe, ProbeTarget, RunpodAccount, RunpodTransport, TAG_OWNER,
    TAG_REQUEST, TerminateAnswer, spend::reported_micro_usd,
};

/// The most of one answer, head included, that is read before it is treated as unreadable.
const MAX_ANSWER_BYTES: usize = 8 * 1024 * 1024;
/// The most pages one pod listing reads; a listing with more is not complete.
const MAX_PAGES: usize = 64;
/// How long a probe of a pod may take, connect to last byte.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Upper-case hexadecimal digits, for percent-encoding.
const HEX: &[u8; 16] = b"0123456789ABCDEF";
/// What every request names itself as.
const USER_AGENT: &str = "b10x-llm-gateway";
/// Runpod's answer to a create no machine of the GPU type can take.
const NO_CAPACITY: &str = "no instances currently available";
/// The spend-rate query, when the balance came from `GET /billing/balance`.
const SPEND_QUERY: &str = "query { myself { currentSpendPerHr } }";
/// The account query of the balance fallback: only fields Runpod's GraphQL `myself` is known to
/// answer.
const ACCOUNT_QUERY: &str = "query { myself { clientBalance currentSpendPerHr } }";
/// The most of one log read, head included; what arrived before it is read.
const MAX_LOG_BYTES: usize = 1024 * 1024;
/// The longest one log read lasts, however much the stream still sends.
const LOG_BOUND: Duration = Duration::from_secs(5);
/// How many of the pod's last system lines one log read asks for: v2's own default, written
/// out. A start older than the last 100 system lines is not the last start.
const LOG_TAIL: u32 = 100;
/// The system line Runpod writes each time a pod's container starts:
/// `start container for <image>: begin`.
const START_PREFIX: &str = "start container for ";
const START_SUFFIX: &str = ": begin";

/// Where the transport sends its calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestBinding {
    /// The REST API's base, without a trailing slash: `https://api.runpod.io/v2`.
    pub base_url: String,
    /// The GraphQL endpoint the account view reads: `https://api.runpod.io/graphql`.
    pub graphql_url: String,
    /// The bound on one call, connect to last byte.
    pub timeout: Duration,
    /// How long a pod's log stream may stay quiet before one read of it ends. Over `https` it
    /// also bounds the connection's setup, which the TLS connection takes one bound for.
    pub log_quiet: Duration,
    /// The least time between two reads of one pod's log; a read in between answers the start
    /// the last one found.
    pub log_every: Duration,
}

impl RestBinding {
    /// Runpod's own endpoints, a 30 s bound, a log read that ends after 1 s of quiet, and one
    /// log read per pod per 15 s: a crash loop's restarts come tens of seconds apart (33 s on
    /// 2026-11-26), and the crash window is minutes long.
    pub fn runpod() -> Self {
        Self {
            base_url: "https://api.runpod.io/v2".to_owned(),
            graphql_url: "https://api.runpod.io/graphql".to_owned(),
            timeout: Duration::from_secs(30),
            log_quiet: Duration::from_secs(1),
            log_every: Duration::from_secs(15),
        }
    }
}

/// What the transport knows of one pod's container starts.
#[derive(Debug, Default)]
struct StartWatch {
    /// The latest container start any log read found.
    started_at: Option<u64>,
    /// When its log was last read.
    read_at: Option<Instant>,
    /// Its last probe found vLLM serving.
    serving: bool,
}

/// Why one call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureCause {
    /// The host answered this HTTP status.
    Status(u16),
    /// The connection failed: its own error, a resolver's included.
    Connection(String),
    /// An `https://` URL and no TLS connection was handed in.
    NoTls,
    /// Not an `https://` URL, nor an `http://` one to a loopback host.
    NotAllowed,
    /// No whole HTTP/1.1 answer arrived inside the bound.
    NoAnswer,
}

impl fmt::Display for FailureCause {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status(status) => write!(formatter, "HTTP {status}"),
            Self::Connection(error) => formatter.write_str(error),
            Self::NoTls => formatter.write_str("no TLS connection"),
            Self::NotAllowed => formatter.write_str("not an https URL"),
            Self::NoAnswer => formatter.write_str("no whole HTTP/1.1 answer in time"),
        }
    }
}

/// One failed call: the host it went to and why it failed. Carries no key and no answer body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallFailure {
    /// The `host[:port]` the call went to.
    pub host: String,
    pub cause: FailureCause,
}

impl fmt::Display for CallFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.host, self.cause)
    }
}

/// The direct Runpod transport over Runpod's REST API.
pub struct RestRunpod {
    binding: RestBinding,
    api_key: BearerKey,
    keys: BTreeMap<String, BearerKey>,
    tls: Option<Arc<dyn PodTls>>,
    failure: Mutex<Option<CallFailure>>,
    starts: BTreeMap<String, StartWatch>,
}

impl fmt::Debug for RestRunpod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RestRunpod")
            .field("binding", &self.binding)
            .field("api_key", &self.api_key)
            .field("keys", &self.keys.keys().collect::<Vec<_>>())
            .field("tls", &self.tls.is_some())
            .finish_non_exhaustive()
    }
}

/// One answer: the status and the decoded body.
struct Reply {
    status: u16,
    body: Vec<u8>,
}

impl Reply {
    fn success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The body as JSON, only for a 2xx.
    fn json(&self) -> Option<Value> {
        self.success()
            .then(|| serde_json::from_slice(&self.body).ok())
            .flatten()
    }
}

/// A URL this transport may call: its scheme, `host[:port]` and the path with its query.
struct Target<'a> {
    tls: bool,
    authority: &'a str,
    path: String,
}

/// The `host[:port]` of an absolute URL, for a failure's text.
fn authority_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#']).next().unwrap_or_default()
}

/// Splits an absolute URL. `None` for a scheme other than `https`, for `http` to a host other
/// than a loopback one, and for an authority carrying user information.
fn target(url: &str) -> Option<Target<'_>> {
    let (tls, rest) = if let Some(rest) = url.strip_prefix("https://") {
        (true, rest)
    } else {
        (false, url.strip_prefix("http://")?)
    };
    let (authority, path) = rest.find('/').map_or((rest, ""), |at| rest.split_at(at));
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    if !tls && !loopback(authority) {
        return None;
    }
    Some(Target {
        tls,
        authority,
        path: if path.is_empty() {
            "/".to_owned()
        } else {
            path.to_owned()
        },
    })
}

/// Whether `authority` names a loopback host: `localhost`, `127.0.0.0/8` or `[::1]`.
fn loopback(authority: &str) -> bool {
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// `value` percent-encoded for a path segment or a query value: everything but RFC 3986's
/// unreserved characters.
fn encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

/// Every pod entry a listing read, and whether it read every page.
struct Entries {
    pods: Vec<Value>,
    whole: bool,
    /// At least the first page was read.
    read: bool,
}

impl RestRunpod {
    /// A transport calling `binding`'s endpoints with this Runpod API key. Calls nothing yet.
    pub fn new(binding: RestBinding, api_key: Vec<u8>) -> Self {
        Self {
            binding,
            api_key: BearerKey::new(api_key),
            keys: BTreeMap::new(),
            tls: None,
            failure: Mutex::new(None),
            starts: BTreeMap::new(),
        }
    }

    /// Hands the transport the TLS connection every `https://` call opens: the control plane's
    /// and the readiness probe's. This crate links no TLS client; without one no `https://` call
    /// is made.
    pub fn set_tls(&mut self, tls: Arc<dyn PodTls>) {
        self.tls = Some(tls);
    }

    /// Hands the transport one model's vLLM key, which the readiness probe of that model's pods
    /// sends as a bearer. Replaces any earlier key for the alias.
    pub fn set_vllm_key(&mut self, alias: &str, key: Vec<u8>) {
        self.keys.insert(alias.to_owned(), BearerKey::new(key));
    }

    /// The last call that failed, since the last listing or account read began: its host and
    /// its HTTP status or connection error. A 2xx answer that did not parse is not a failure
    /// here.
    pub fn last_failure(&self) -> Option<CallFailure> {
        self.failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn fail(&self, url: &str, cause: FailureCause) {
        *self.failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(CallFailure {
            host: authority_of(url).to_owned(),
            cause,
        });
    }

    fn forget_failure(&self) {
        *self.failure.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// One call to `url` with the Runpod key as a bearer. `None` when no whole answer arrived
    /// inside the binding's timeout, or the answer was not HTTP/1.1, or was longer than
    /// [`MAX_ANSWER_BYTES`]. A non-2xx answer is returned and kept as the last failure.
    fn call(&self, method: &str, url: &str, body: Option<&[u8]>) -> Option<Reply> {
        let Some(target) = target(url) else {
            self.fail(url, FailureCause::NotAllowed);
            return None;
        };
        let token = self.api_key.token()?;
        let mut head = format!(
            "{method} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {USER_AGENT}\r\nAccept: application/json\r\nConnection: close\r\n",
            target.path, target.authority
        );
        if let Some(body) = body {
            head.push_str("Content-Type: application/json\r\nContent-Length: ");
            head.push_str(&body.len().to_string());
            head.push_str("\r\n");
        }
        let request = Request::of(&[
            head.as_bytes(),
            b"Authorization: Bearer ",
            token.as_bytes(),
            b"\r\n\r\n",
            body.unwrap_or_default(),
        ]);
        let answer = self.exchange(&target, request.bytes());
        drop(request);
        let parsed = match answer {
            Ok(answer) => probe::parse(&answer),
            Err(cause) => {
                self.fail(url, cause);
                return None;
            }
        };
        let Some((status, body)) = parsed else {
            self.fail(url, FailureCause::NoAnswer);
            return None;
        };
        let reply = Reply { status, body };
        if !reply.success() {
            self.fail(url, FailureCause::Status(status));
        }
        Some(reply)
    }

    /// Sends one request and reads its whole answer, all inside the binding's timeout.
    fn exchange(&self, target: &Target<'_>, request: &[u8]) -> Result<Vec<u8>, FailureCause> {
        let timeout = self.binding.timeout;
        let deadline = Instant::now() + timeout;
        let connection = |error: std::io::Error| FailureCause::Connection(error.to_string());
        if target.tls {
            let tls = self.tls.as_ref().ok_or(FailureCause::NoTls)?;
            let mut stream = tls.connect(target.authority, timeout).map_err(connection)?;
            stream.write_all(request).map_err(connection)?;
            stream.flush().map_err(connection)?;
            return probe::read_answer(&mut *stream, deadline, MAX_ANSWER_BYTES, |_| Some(()))
                .ok_or(FailureCause::NoAnswer);
        }
        let stream = Self::connect_plain(target, timeout)?;
        (&stream).write_all(request).map_err(connection)?;
        probe::read_answer(&mut &stream, deadline, MAX_ANSWER_BYTES, |left| {
            stream
                .set_read_timeout(Some(left.max(Duration::from_millis(1))))
                .ok()
        })
        .ok_or(FailureCause::NoAnswer)
    }

    /// A plain connection to a loopback `target`, its connect and its writes bound by `timeout`.
    fn connect_plain(target: &Target<'_>, timeout: Duration) -> Result<TcpStream, FailureCause> {
        let connection = |error: std::io::Error| FailureCause::Connection(error.to_string());
        let address = if target.authority.contains(':') && !target.authority.ends_with(']') {
            target.authority.to_owned()
        } else {
            format!("{}:80", target.authority)
        };
        let mut last = FailureCause::NoAnswer;
        for address in address.to_socket_addrs().map_err(connection)? {
            match TcpStream::connect_timeout(&address, timeout) {
                Ok(stream) => {
                    stream
                        .set_write_timeout(Some(timeout))
                        .map_err(connection)?;
                    return Ok(stream);
                }
                Err(error) => last = connection(error),
            }
        }
        Err(last)
    }

    /// One read of a stream that stays open, with the Runpod key as a bearer: what arrived
    /// until it was quiet for [`RestBinding::log_quiet`], until [`LOG_BOUND`], or up to
    /// [`MAX_LOG_BYTES`], decoded as far as it arrived whole. `None`, kept as the last failure,
    /// for no answer or a non-2xx one.
    fn stream(&self, url: &str) -> Option<Vec<u8>> {
        let Some(target) = target(url) else {
            self.fail(url, FailureCause::NotAllowed);
            return None;
        };
        let token = self.api_key.token()?;
        let head = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {USER_AGENT}\r\nAccept: text/event-stream\r\nConnection: close\r\n",
            target.path, target.authority
        );
        let request = Request::of(&[
            head.as_bytes(),
            b"Authorization: Bearer ",
            token.as_bytes(),
            b"\r\n\r\n",
        ]);
        let answer = self.read_open(&target, request.bytes());
        drop(request);
        let answer = match answer {
            Ok(answer) => answer,
            Err(cause) => {
                self.fail(url, cause);
                return None;
            }
        };
        let Some((status, body)) = probe::parse_open(&answer) else {
            self.fail(url, FailureCause::NoAnswer);
            return None;
        };
        if !(200..300).contains(&status) {
            self.fail(url, FailureCause::Status(status));
            return None;
        }
        Some(body)
    }

    /// Sends one request and reads what its open answer sends until it is quiet, or until the
    /// read's bound in time or bytes. Over `https` the quiet bound also bounds the connection's
    /// setup: [`PodTls::connect`] takes one bound for both.
    fn read_open(&self, target: &Target<'_>, request: &[u8]) -> Result<Vec<u8>, FailureCause> {
        let quiet = self.binding.log_quiet;
        let deadline = Instant::now() + LOG_BOUND;
        let connection = |error: std::io::Error| FailureCause::Connection(error.to_string());
        if target.tls {
            let tls = self.tls.as_ref().ok_or(FailureCause::NoTls)?;
            let mut stream = tls.connect(target.authority, quiet).map_err(connection)?;
            stream.write_all(request).map_err(connection)?;
            stream.flush().map_err(connection)?;
            return Ok(read_until_quiet(&mut *stream, deadline));
        }
        let stream = Self::connect_plain(target, self.binding.timeout)?;
        stream
            .set_read_timeout(Some(quiet.max(Duration::from_millis(1))))
            .map_err(connection)?;
        (&stream).write_all(request).map_err(connection)?;
        Ok(read_until_quiet(&mut &stream, deadline))
    }

    /// The latest container start the pod's system log names, read once now.
    fn logged_start(&self, pod_id: &str) -> Option<u64> {
        let body = self.stream(&format!(
            "{}/pods/{}/logs?source=system&tail={LOG_TAIL}",
            self.binding.base_url,
            encode(pod_id)
        ))?;
        last_container_start(&body)
    }

    fn rest(&self, method: &str, path: &str, body: Option<&[u8]>) -> Option<Reply> {
        self.call(method, &format!("{}{path}", self.binding.base_url), body)
    }

    /// One page of `GET /pods`: its entries and the cursor of the next page, `Some(None)` on
    /// the last page. `None` when the answer is not a 2xx v2 page, or says there is a next page
    /// without naming its cursor.
    fn page(&self, cursor: Option<&str>) -> Option<(Vec<Value>, Option<String>)> {
        let query = cursor.map_or_else(String::new, |cursor| format!("?cursor={}", encode(cursor)));
        let page = self.rest("GET", &format!("/pods{query}"), None)?.json()?;
        let pods = page.get("pods")?.as_array()?.clone();
        let more = page
            .pointer("/pagination/hasNextPage")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !more {
            return Some((pods, None));
        }
        let next = page
            .pointer("/pagination/nextCursor")
            .and_then(Value::as_str)
            .filter(|cursor| !cursor.is_empty())?;
        Some((pods, Some(next.to_owned())))
    }

    /// Every pod entry of the account, page after page. `whole` only when every page was read:
    /// a page that fails, a cursor that comes back, or more than [`MAX_PAGES`] pages end the
    /// listing with what was read.
    fn entries(&self) -> Entries {
        let mut pods = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen = BTreeSet::new();
        for _ in 0..MAX_PAGES {
            let Some((page, next)) = self.page(cursor.as_deref()) else {
                return Entries {
                    pods,
                    whole: false,
                    read: cursor.is_some(),
                };
            };
            pods.extend(page);
            match next {
                None => {
                    return Entries {
                        pods,
                        whole: true,
                        read: true,
                    };
                }
                Some(next) if !seen.insert(next.clone()) => {
                    return Entries {
                        pods,
                        whole: false,
                        read: true,
                    };
                }
                Some(next) => cursor = Some(next),
            }
        }
        Entries {
            pods,
            whole: false,
            read: true,
        }
    }

    /// The listing's entry of `pod_id`, when the listing holds it.
    fn listed(&self, pod_id: &str) -> Option<Value> {
        self.entries()
            .pods
            .into_iter()
            .find(|pod| pod.get("id").and_then(Value::as_str) == Some(pod_id))
    }

    /// A create whose answer never said what happened: one whole listing, read for the pod of
    /// this create's name and request tag, never a resend. The name is per model alias, not
    /// per create: only a pod carrying this create's own request tag was made by it.
    fn resolve_create(&self, request: &PodRequest) -> CreateAnswer {
        let Some(tag) = request.env.get(TAG_REQUEST) else {
            return CreateAnswer::Lost;
        };
        let entries = self.entries();
        if !entries.whole {
            return CreateAnswer::Lost;
        }
        let named = entries
            .pods
            .iter()
            .filter(|pod| pod.get("name").and_then(Value::as_str) == Some(request.name.as_str()));
        let Some(pods) = named.map(pod_of).collect::<Option<Vec<Pod>>>() else {
            return CreateAnswer::Lost;
        };
        let mut ours = pods
            .into_iter()
            .filter(|pod| pod.env.get(TAG_REQUEST) == Some(tag));
        match (ours.next(), ours.next()) {
            (Some(pod), None) => CreateAnswer::Created(pod),
            _ => CreateAnswer::Lost,
        }
    }

    /// The balance and the spend rate: `balanceAmount` of `GET /billing/balance`, and GraphQL's
    /// `currentSpendPerHr`. On a 403 or 404 balance the same GraphQL query also reads
    /// `clientBalance`. Each is `None` when its read failed or did not report it.
    fn balance(&self) -> (Option<f64>, Option<f64>) {
        let reply = self.rest("GET", "/billing/balance", None);
        let fallback = reply
            .as_ref()
            .is_some_and(|reply| matches!(reply.status, 403 | 404));
        let balance = reply
            .and_then(|reply| reply.json())
            .as_ref()
            .and_then(|answer| answer.get("balanceAmount"))
            .and_then(number);
        let query = if fallback { ACCOUNT_QUERY } else { SPEND_QUERY };
        let body = json!({ "query": query }).to_string();
        let answer = self
            .call("POST", &self.binding.graphql_url, Some(body.as_bytes()))
            .and_then(|reply| reply.json());
        let field = |name: &str| {
            answer
                .as_ref()?
                .pointer(&format!("/data/myself/{name}"))
                .and_then(number)
        };
        let balance = if fallback {
            field("clientBalance")
        } else {
            balance
        };
        (balance, field("currentSpendPerHr"))
    }
}

/// A finite, non-negative number, given as a JSON number or a numeric string.
fn number(value: &Value) -> Option<f64> {
    let number = match value {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => text.trim().parse().ok()?,
        _ => return None,
    };
    (number.is_finite() && number >= 0.0).then_some(number)
}

/// One v2 `Pod`, or `None` when any field this adapter relies on is missing or of another
/// shape. Its `status` is read as: `PROVISIONING` and `STARTING` created and not running yet,
/// `RUNNING`, `EXITED` and `ERROR` exited (the pod still exists and is billed), `TERMINATED`.
fn pod_of(value: &Value) -> Option<Pod> {
    let status = match value.get("status")?.as_str()? {
        "PROVISIONING" | "STARTING" => PodStatus::Created,
        "RUNNING" => PodStatus::Running,
        "EXITED" | "ERROR" => PodStatus::Exited,
        "TERMINATED" => PodStatus::Terminated,
        _ => return None,
    };
    let env = match value.get("env") {
        None | Some(Value::Null) => BTreeMap::new(),
        Some(Value::Object(env)) => env
            .iter()
            .map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
            .collect::<Option<_>>()?,
        Some(_) => return None,
    };
    Some(Pod {
        id: value.get("id")?.as_str()?.to_owned(),
        name: value.get("name")?.as_str()?.to_owned(),
        status,
        env,
        gpu_type: value
            .pointer("/gpu/id")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// A v2 pod's placement: `cloud` (`SECURE` or `COMMUNITY`). v2 declares no `interruptible` and
/// no way to create an interruptible pod, so it is never reported.
fn placement_of(value: &Value) -> PodPlacement {
    PodPlacement {
        secure_cloud: match value.get("cloud").and_then(Value::as_str) {
            Some("SECURE") => Some(true),
            Some("COMMUNITY") => Some(false),
            _ => None,
        },
        interruptible: None,
    }
}

/// A v2 pod's `startedAt`, `None` while it is null.
fn pod_started_at_ms(value: &Value) -> Option<u64> {
    value
        .get("startedAt")
        .and_then(Value::as_str)
        .and_then(rest_started_at_ms)
}

/// Reads `stream` until it ends, fails, or a read waits past the stream's own read bound (the
/// quiet bound), until `deadline`, or until [`MAX_LOG_BYTES`] arrived; what arrived before is
/// kept.
fn read_until_quiet(stream: &mut dyn Read, deadline: Instant) -> Vec<u8> {
    let mut answer = Vec::new();
    let mut buffer = [0_u8; 8192];
    while Instant::now() < deadline && answer.len() < MAX_LOG_BYTES {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => answer.extend_from_slice(buffer.get(..read).unwrap_or_default()),
        }
    }
    answer.truncate(MAX_LOG_BYTES);
    answer
}

/// The latest `ts` of a `system` event whose line is `start container for <image>: begin`, in a
/// log stream's Server-Sent Events (`data: {"source", "line", "ts"}`, one JSON object per `data`
/// line). An event that is not such a line, or whose `ts` is not RFC 3339 UTC, names no start.
fn last_container_start(body: &[u8]) -> Option<u64> {
    String::from_utf8_lossy(body)
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
        .filter(|event| event.get("source").and_then(Value::as_str) == Some("system"))
        .filter(|event| {
            event
                .get("line")
                .and_then(Value::as_str)
                .map(str::trim)
                .is_some_and(|line| line.starts_with(START_PREFIX) && line.ends_with(START_SUFFIX))
        })
        .filter_map(|event| started_at_ms(event.get("ts")?.as_str()?))
        .max()
}

/// One pod of a `GET /pods` answer for the account view, or `None` without an id and a name.
/// The uptime is `runtime.uptime`, which the listing leaves null.
fn summary(value: &Value) -> Option<PodSummary> {
    let text = |pointer: &str| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    Some(PodSummary {
        id: text("/id")?,
        name: text("/name")?,
        desired_status: text("/status").unwrap_or_default(),
        cost_per_hour_usd: value.get("cost").and_then(number),
        gpu: text("/gpu/id"),
        uptime_seconds: value.pointer("/runtime/uptime").and_then(Value::as_u64),
        last_started_at_ms: pod_started_at_ms(value),
        env_owner: value
            .pointer(&format!("/env/{TAG_OWNER}"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// A v2 `date-time` in UTC (`2026-11-04T13:43:01Z`, a fraction allowed), or the
/// `2026-11-04 13:43:01.245 +0000 UTC` form Runpod's v1 REST API wrote, in milliseconds since
/// the Unix epoch.
fn rest_started_at_ms(stamp: &str) -> Option<u64> {
    match stamp.strip_suffix(" +0000 UTC") {
        Some(utc) => started_at_ms(&format!("{}Z", utc.replacen(' ', "T", 1))),
        None => started_at_ms(stamp),
    }
}

/// The account's spend from v2's `GET /billing/pods?bucketSize=day` `records`
/// (`PodBillingRecord`: one pod, one day bucket), read only when every record has a finite,
/// non-negative `totalAmount` in USD and a `startTime` in RFC 3339 UTC. The total is every
/// record's; the daily spend is the records whose bucket starts at or after `day_start_ms`. Each
/// amount is rounded to the nearest micro-USD.
fn spend_of_records(records: &[Value], day_start_ms: u64) -> Option<AccountSpend> {
    let mut spend = AccountSpend {
        daily_micro_usd: 0,
        total_micro_usd: 0,
    };
    for record in records {
        let amount = reported_micro_usd(record.get("totalAmount")?.as_f64()?)?;
        let bucket = started_at_ms(record.get("startTime")?.as_str()?)?;
        spend.total_micro_usd = spend.total_micro_usd.checked_add(amount)?;
        if bucket >= day_start_ms {
            spend.daily_micro_usd = spend.daily_micro_usd.checked_add(amount)?;
        }
    }
    Some(spend)
}

/// Whether a refused create's answer says Runpod has no machine of the GPU type free.
fn no_capacity(body: &[u8]) -> bool {
    String::from_utf8_lossy(body)
        .to_ascii_lowercase()
        .contains(NO_CAPACITY)
}

/// Where a network volume is mounted when the request names none: Runpod v1's default
/// `volumeMountPath`, which v2's `NetworkMount` no longer supplies.
const DEFAULT_MOUNT_PATH: &str = "/workspace";

/// Runpod v2's `POST /pods` body, its `CreatePodRequest`, which refuses any key it does not
/// declare: `name`, `image`, the one GPU type as `gpu: {id, count}`, `cloud`, the container disk
/// as `disk`, `ports`, `env`, the vLLM argv as `entrypoint` (the image's CMD is left as it is),
/// `dataCenterIds` when any is pinned, and a network volume as `mounts.network`. v2 has no
/// `interruptible`: every pod it creates is on demand. No pod volume is asked for.
fn create_body_v2(request: &PodRequest) -> Map<String, Value> {
    let mut body = Map::new();
    body.insert("name".to_owned(), json!(request.name));
    body.insert("image".to_owned(), json!(request.image));
    body.insert(
        "gpu".to_owned(),
        json!({ "id": request.gpu_type, "count": request.gpu_count }),
    );
    body.insert("cloud".to_owned(), json!(request.cloud_type.as_str()));
    body.insert("disk".to_owned(), json!(request.container_disk_gb));
    body.insert("ports".to_owned(), json!(request.ports));
    body.insert("env".to_owned(), json!(request.env));
    if !request.docker_entrypoint.is_empty() {
        body.insert("entrypoint".to_owned(), json!(request.docker_entrypoint));
    }
    if !request.data_center_ids.is_empty() {
        body.insert("dataCenterIds".to_owned(), json!(request.data_center_ids));
    }
    if let Some(volume) = &request.network_volume_id {
        let path = request
            .volume_mount_path
            .as_deref()
            .unwrap_or(DEFAULT_MOUNT_PATH);
        body.insert(
            "mounts".to_owned(),
            json!({ "network": [{ "volumeId": volume, "path": path }] }),
        );
    }
    body
}

impl RunpodTransport for RestRunpod {
    fn list_pods(&mut self) -> Result<PodListing, ()> {
        self.forget_failure();
        let entries = self.entries();
        // Not even the first page: the listing failed outright.
        if !entries.read {
            return Err(());
        }
        let parsed: Vec<Option<Pod>> = entries.pods.iter().map(pod_of).collect();
        let complete = entries.whole && parsed.iter().all(Option::is_some);
        Ok(PodListing {
            pods: parsed.into_iter().flatten().collect(),
            complete,
        })
    }

    fn create_pod(&mut self, request: &PodRequest) -> CreateAnswer {
        let body = Value::Object(create_body_v2(request)).to_string();
        match self.rest("POST", "/pods", Some(body.as_bytes())) {
            Some(reply) if reply.success() => reply
                .json()
                .as_ref()
                .and_then(pod_of)
                .map_or_else(|| self.resolve_create(request), CreateAnswer::Created),
            // 408: the request timed out on Runpod's side, and may have been acted on.
            Some(reply) if (400..500).contains(&reply.status) && reply.status != 408 => {
                if no_capacity(&reply.body) {
                    CreateAnswer::NoCapacity
                } else {
                    CreateAnswer::Refused
                }
            }
            _ => self.resolve_create(request),
        }
    }

    fn terminate_pod(&mut self, pod_id: &str) -> TerminateAnswer {
        match self.rest("DELETE", &format!("/pods/{}", encode(pod_id)), None) {
            Some(reply) if reply.success() => {
                self.starts.remove(pod_id);
                TerminateAnswer::Terminated
            }
            // Including 404: nothing was terminated by this call.
            Some(reply) if (400..500).contains(&reply.status) && reply.status != 408 => {
                TerminateAnswer::Refused
            }
            _ => TerminateAnswer::Lost,
        }
    }

    /// Also remembers whether the pod served, which spares a serving pod its log reads.
    fn probe_ready(&mut self, target: &ProbeTarget<'_>) -> Probe {
        let probe = target.endpoint.map_or(Probe::Unreachable, |endpoint| {
            let key = target.model.and_then(|model| self.keys.get(model));
            probe::models(endpoint, key, PROBE_TIMEOUT, self.tls.as_deref())
        });
        self.starts
            .entry(target.pod_id.to_owned())
            .or_default()
            .serving = matches!(probe, Probe::Ready { .. });
        probe
    }

    /// The latest container start the pod's system log named. The log is read when the pod's
    /// last probe did not find it serving and no read of it was made in the last
    /// [`RestBinding::log_every`]; otherwise the start the last read found is answered. A read
    /// that finds no start, or an earlier one, leaves the start found before.
    fn container_started_at(&mut self, pod_id: &str) -> Option<u64> {
        let every = self.binding.log_every;
        let due = self.starts.get(pod_id).is_none_or(|watch| {
            !watch.serving && watch.read_at.is_none_or(|at| at.elapsed() >= every)
        });
        if due {
            let logged = self.logged_start(pod_id);
            let watch = self.starts.entry(pod_id.to_owned()).or_default();
            watch.read_at = Some(Instant::now());
            watch.started_at = watch.started_at.max(logged);
        }
        self.starts.get(pod_id).and_then(|watch| watch.started_at)
    }

    fn pod_placement(&mut self, pod_id: &str) -> Option<PodPlacement> {
        self.listed(pod_id).as_ref().map(placement_of)
    }

    /// `GET /billing/pods?bucketSize=day`, over Runpod's default window: the 30 days up to now,
    /// today's bucket included. The total is that window's, not the account's whole history.
    fn account_spend(&mut self, day_start_ms: u64) -> Option<AccountSpend> {
        let answer = self
            .rest("GET", "/billing/pods?bucketSize=day", None)?
            .json()?;
        spend_of_records(answer.get("records")?.as_array()?, day_start_ms)
    }
}

impl RunpodAccount for RestRunpod {
    /// A failed pod listing leaves the pods unread (`pods_read: false`, no pod), and a failed
    /// balance read leaves only the balance and spend rate unreported. `None` only when neither
    /// the pods, nor the balance, nor the spend rate was read. A pod entry without an id or a
    /// name is left out.
    fn account(&mut self) -> Option<AccountSnapshot> {
        self.forget_failure();
        let entries = self.entries();
        let pods: Vec<PodSummary> = if entries.whole {
            entries.pods.iter().filter_map(summary).collect()
        } else {
            Vec::new()
        };
        let (balance_usd, spend_per_hour_usd) = self.balance();
        if !entries.whole && balance_usd.is_none() && spend_per_hour_usd.is_none() {
            return None;
        }
        Some(AccountSnapshot {
            balance_usd,
            spend_per_hour_usd,
            pods,
            pods_read: entries.whole,
        })
    }

    fn last_failure(&self) -> Option<crate::CallFailure> {
        RestRunpod::last_failure(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{authority_of, encode, loopback, rest_started_at_ms, target};

    #[test]
    fn the_rest_start_stamp_is_read_in_utc() {
        let rfc = rest_started_at_ms("2026-11-04T13:43:01.245Z");
        assert!(rfc.is_some());
        assert_eq!(rest_started_at_ms("2026-11-04 13:43:01.245 +0000 UTC"), rfc);
        assert_eq!(
            rest_started_at_ms("2026-11-04 13:43:01.245 +0100 CET"),
            None
        );
    }

    #[test]
    fn plain_http_is_only_for_a_loopback_host() {
        assert!(target("http://127.0.0.1:9/v2").is_some());
        assert!(target("http://localhost:9/v2").is_some());
        assert!(target("http://[::1]:9/v2").is_some());
        assert!(target("http://api.runpod.io/v2").is_none());
        assert!(target("http://10.0.0.1/v2").is_none());
        assert!(target("https://user@api.runpod.io/v2").is_none());
        assert!(target("ftp://127.0.0.1/").is_none());
        assert!(!loopback("127.0.0.1.example.com"));
        let runpod = target("https://api.runpod.io/v2").expect("https");
        assert_eq!(
            (runpod.authority, runpod.path.as_str()),
            ("api.runpod.io", "/v2")
        );
    }

    #[test]
    fn a_failure_names_the_authority_of_its_url() {
        assert_eq!(
            authority_of("https://api.runpod.io/v2/pods"),
            "api.runpod.io"
        );
        assert_eq!(authority_of("http://127.0.0.1:9/graphql"), "127.0.0.1:9");
        assert_eq!(authority_of("https://h.example.test?x"), "h.example.test");
    }

    #[test]
    fn a_query_value_is_percent_encoded() {
        assert_eq!(encode("b10x-llm-qwen"), "b10x-llm-qwen");
        assert_eq!(encode("a b&c=d/"), "a%20b%26c%3Dd%2F");
    }
}
