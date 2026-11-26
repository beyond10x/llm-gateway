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
//!   later one leaves the listing partial. Each entry is read as [`ConnectorsRunpod`](super::ConnectorsRunpod)
//!   reads a `pods.list` entry. The listing ignores query filters, so none is sent.
//! * `create_pod`: `POST /pods` with v2's body: v1's keys, but `image` for `imageName` and one
//!   GPU type as `gpu: {id, count}`. A 2xx is `Created`; a 4xx whose answer says Runpod has no
//!   instance available is `NoCapacity`, any other 4xx `Refused`. No answer, a 5xx or a timeout
//!   may have created the pod: one listing resolves it by its name and request tag, as
//!   connectors' `unknown` is resolved; otherwise it is `Lost`. A create is never sent twice.
//! * `terminate_pod`: `DELETE /pods/<id>`. A 2xx is `Terminated`, a 4xx `Refused`, anything
//!   else `Lost`.
//! * `container_started_at` and `pod_placement`: the pod's `lastStartedAt`, and its
//!   `machine.secureCloud` and `interruptible`, from the listing (v2 documents no read of one
//!   pod).
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
    io::Write,
    net::{TcpStream, ToSocketAddrs},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use serde_json::{Map, Value, json};

use super::{
    connectors::{create_body, placement_of, pod_of, started_at_ms},
    probe::{self, BearerKey, PodTls, Request},
};
use crate::{
    AccountSnapshot, AccountSpend, CreateAnswer, Pod, PodListing, PodPlacement, PodRequest,
    PodSummary, Probe, ProbeTarget, RunpodAccount, RunpodTransport, TAG_OWNER, TAG_REQUEST,
    TerminateAnswer, spend::reported_micro_usd,
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

/// Where the transport sends its calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestBinding {
    /// The REST API's base, without a trailing slash: `https://api.runpod.io/v2`.
    pub base_url: String,
    /// The GraphQL endpoint the account view reads: `https://api.runpod.io/graphql`.
    pub graphql_url: String,
    /// The bound on one call, connect to last byte.
    pub timeout: Duration,
}

impl RestBinding {
    /// Runpod's own endpoints and a 30 s bound.
    pub fn runpod() -> Self {
        Self {
            base_url: "https://api.runpod.io/v2".to_owned(),
            graphql_url: "https://api.runpod.io/graphql".to_owned(),
            timeout: Duration::from_secs(30),
        }
    }
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
        let address = if target.authority.contains(':') && !target.authority.ends_with(']') {
            target.authority.to_owned()
        } else {
            format!("{}:80", target.authority)
        };
        let mut last = FailureCause::NoAnswer;
        let mut connected = None;
        for address in address.to_socket_addrs().map_err(connection)? {
            match TcpStream::connect_timeout(&address, timeout) {
                Ok(stream) => {
                    connected = Some(stream);
                    break;
                }
                Err(error) => last = connection(error),
            }
        }
        let stream = connected.ok_or(last)?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(connection)?;
        (&stream).write_all(request).map_err(connection)?;
        probe::read_answer(&mut &stream, deadline, MAX_ANSWER_BYTES, |left| {
            stream
                .set_read_timeout(Some(left.max(Duration::from_millis(1))))
                .ok()
        })
        .ok_or(FailureCause::NoAnswer)
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

/// One pod of a `GET /pods` answer for the account view, or `None` without an id and a name.
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
        desired_status: text("/desiredStatus").unwrap_or_default(),
        cost_per_hour_usd: value.get("costPerHr").and_then(number),
        gpu: text("/gpu/id").or_else(|| text("/machine/gpuTypeId")),
        uptime_seconds: value.get("uptimeSeconds").and_then(Value::as_u64),
        last_started_at_ms: value
            .get("lastStartedAt")
            .and_then(Value::as_str)
            .and_then(rest_started_at_ms),
        env_owner: value
            .pointer(&format!("/env/{TAG_OWNER}"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// Runpod REST's `lastStartedAt`, `2026-11-04 13:43:01.245 +0000 UTC` (UTC only), or the RFC 3339
/// form GraphQL writes, in milliseconds since the Unix epoch.
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

/// Runpod v2's `POST /pods` body: [`create_body`]'s keys, with `image` for `imageName` and the
/// one GPU type as `gpu: {id, count}` for `gpuTypeIds` and `gpuCount`.
fn create_body_v2(request: &PodRequest) -> Map<String, Value> {
    let mut body = create_body(request);
    for v1 in ["imageName", "gpuTypeIds", "gpuCount"] {
        body.remove(v1);
    }
    body.insert("image".to_owned(), json!(request.image));
    body.insert(
        "gpu".to_owned(),
        json!({ "id": request.gpu_type, "count": request.gpu_count }),
    );
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
            Some(reply) if reply.success() => TerminateAnswer::Terminated,
            // Including 404: nothing was terminated by this call.
            Some(reply) if (400..500).contains(&reply.status) && reply.status != 408 => {
                TerminateAnswer::Refused
            }
            _ => TerminateAnswer::Lost,
        }
    }

    fn probe_ready(&mut self, target: &ProbeTarget<'_>) -> Probe {
        let Some(endpoint) = target.endpoint else {
            return Probe::Unreachable;
        };
        let key = target.model.and_then(|model| self.keys.get(model));
        probe::models(endpoint, key, PROBE_TIMEOUT, self.tls.as_deref())
    }

    fn container_started_at(&mut self, pod_id: &str) -> Option<u64> {
        self.listed(pod_id)?
            .get("lastStartedAt")
            .and_then(Value::as_str)
            .and_then(rest_started_at_ms)
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
