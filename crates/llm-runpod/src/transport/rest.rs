//! The direct Runpod transport: Runpod's REST API, called by this process with an API key its
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
//! The calls (Runpod REST `https://rest.runpod.io/v1`):
//!
//! * `list_pods`: `GET /pods`, each entry read as [`ConnectorsRunpod`](super::ConnectorsRunpod)
//!   reads a `pods.list` entry.
//! * `create_pod`: `POST /pods` with the same body `pod.create` carries, unwrapped. A 2xx is
//!   `Created`; a 4xx whose answer says Runpod has no instance available is `NoCapacity`, any
//!   other 4xx `Refused`. No answer, a 5xx or a timeout may have created the pod: one
//!   `GET /pods?name=<name>` resolves it by its name and request tag, as connectors' `unknown`
//!   is resolved; otherwise it is `Lost`. A create is never sent twice.
//! * `terminate_pod`: `DELETE /pods/<id>`. A 2xx is `Terminated`, a 4xx `Refused`, anything
//!   else `Lost`.
//! * `container_started_at`: `GET /pods/<id>`, `lastStartedAt`.
//! * `pod_placement`: `GET /pods/<id>?includeMachine=true`, `machine.secureCloud` and
//!   `interruptible`.
//! * `account_spend`: `GET /billing/pods?bucketSize=day`.
//! * [`RunpodAccount::account`]: Runpod's GraphQL `myself { clientBalance currentSpendPerHr }`
//!   for the balance and spend rate, and `GET /pods` for the pods.

use std::{
    collections::BTreeMap,
    fmt,
    io::Write,
    net::{TcpStream, ToSocketAddrs},
    sync::Arc,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use super::{
    connectors::{create_body, placement_of, pod_of, spend_of, started_at_ms},
    probe::{self, BearerKey, PodTls, Request},
};
use crate::{
    AccountSnapshot, AccountSpend, CreateAnswer, Pod, PodListing, PodPlacement, PodRequest,
    PodSummary, Probe, ProbeTarget, RunpodAccount, RunpodTransport, TAG_OWNER, TAG_REQUEST,
    TerminateAnswer,
};

/// The most of one answer, head included, that is read before it is treated as unreadable.
const MAX_ANSWER_BYTES: usize = 8 * 1024 * 1024;
/// How long a probe of a pod may take, connect to last byte.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Upper-case hexadecimal digits, for percent-encoding.
const HEX: &[u8; 16] = b"0123456789ABCDEF";
/// What every request names itself as.
const USER_AGENT: &str = "b10x-llm-gateway";
/// Runpod's answer to a create no machine of the GPU type can take.
const NO_CAPACITY: &str = "no instances currently available";
/// The account query: only fields Runpod's GraphQL `myself` is known to answer.
const ACCOUNT_QUERY: &str = "query { myself { clientBalance currentSpendPerHr } }";

/// Where the transport sends its calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestBinding {
    /// The REST API's base, without a trailing slash: `https://rest.runpod.io/v1`.
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
            base_url: "https://rest.runpod.io/v1".to_owned(),
            graphql_url: "https://api.runpod.io/graphql".to_owned(),
            timeout: Duration::from_secs(30),
        }
    }
}

/// The direct Runpod transport over Runpod's REST API.
pub struct RestRunpod {
    binding: RestBinding,
    api_key: BearerKey,
    keys: BTreeMap<String, BearerKey>,
    tls: Option<Arc<dyn PodTls>>,
}

impl fmt::Debug for RestRunpod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RestRunpod")
            .field("binding", &self.binding)
            .field("api_key", &self.api_key)
            .field("keys", &self.keys.keys().collect::<Vec<_>>())
            .field("tls", &self.tls.is_some())
            .finish()
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

impl RestRunpod {
    /// A transport calling `binding`'s endpoints with this Runpod API key. Calls nothing yet.
    pub fn new(binding: RestBinding, api_key: Vec<u8>) -> Self {
        Self {
            binding,
            api_key: BearerKey::new(api_key),
            keys: BTreeMap::new(),
            tls: None,
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

    /// One call to `url` with the Runpod key as a bearer. `None` when no whole answer arrived
    /// inside the binding's timeout, or the answer was not HTTP/1.1, or was longer than
    /// [`MAX_ANSWER_BYTES`].
    fn call(&self, method: &str, url: &str, body: Option<&[u8]>) -> Option<Reply> {
        let target = target(url)?;
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
        let (status, body) = probe::parse(&answer?)?;
        Some(Reply { status, body })
    }

    /// Sends one request and reads its whole answer, all inside the binding's timeout.
    fn exchange(&self, target: &Target<'_>, request: &[u8]) -> Option<Vec<u8>> {
        let timeout = self.binding.timeout;
        let deadline = Instant::now() + timeout;
        if target.tls {
            let mut stream = self.tls.as_ref()?.connect(target.authority, timeout).ok()?;
            stream.write_all(request).ok()?;
            stream.flush().ok()?;
            return probe::read_answer(&mut *stream, deadline, MAX_ANSWER_BYTES, |_| Some(()));
        }
        let address = if target.authority.contains(':') && !target.authority.ends_with(']') {
            target.authority.to_owned()
        } else {
            format!("{}:80", target.authority)
        };
        let stream = address
            .to_socket_addrs()
            .ok()?
            .find_map(|address| TcpStream::connect_timeout(&address, timeout).ok())?;
        stream.set_write_timeout(Some(timeout)).ok()?;
        (&stream).write_all(request).ok()?;
        probe::read_answer(&mut &stream, deadline, MAX_ANSWER_BYTES, |left| {
            stream
                .set_read_timeout(Some(left.max(Duration::from_millis(1))))
                .ok()
        })
    }

    fn rest(&self, method: &str, path: &str, body: Option<&[u8]>) -> Option<Reply> {
        self.call(method, &format!("{}{path}", self.binding.base_url), body)
    }

    /// `GET /pods<query>`: every entry, or `None` when the answer is not a 2xx array.
    fn pods(&self, query: &str) -> Option<Vec<Value>> {
        match self.rest("GET", &format!("/pods{query}"), None)?.json()? {
            Value::Array(pods) => Some(pods),
            _ => None,
        }
    }

    /// `GET /pods/<id><query>`: the pod, only when the answer names this id.
    fn pod(&self, pod_id: &str, query: &str) -> Option<Value> {
        let pod = self
            .rest("GET", &format!("/pods/{}{query}", encode(pod_id)), None)?
            .json()?;
        (pod.get("id").and_then(Value::as_str) == Some(pod_id)).then_some(pod)
    }

    /// A create whose answer never said what happened: one read by its name, never a resend.
    /// The name is per model alias, not per create: only a pod carrying this create's own
    /// request tag was made by it.
    fn resolve_create(&self, request: &PodRequest) -> CreateAnswer {
        let Some(tag) = request.env.get(TAG_REQUEST) else {
            return CreateAnswer::Lost;
        };
        let Some(entries) = self.pods(&format!("?name={}", encode(&request.name))) else {
            return CreateAnswer::Lost;
        };
        let Some(pods) = entries.iter().map(pod_of).collect::<Option<Vec<Pod>>>() else {
            return CreateAnswer::Lost;
        };
        let mut ours = pods
            .into_iter()
            .filter(|pod| pod.name == request.name && pod.env.get(TAG_REQUEST) == Some(tag));
        match (ours.next(), ours.next()) {
            (Some(pod), None) => CreateAnswer::Created(pod),
            _ => CreateAnswer::Lost,
        }
    }

    /// The GraphQL account fields: `(clientBalance, currentSpendPerHr)`, each `None` when the
    /// read failed or did not report it.
    fn balance(&self) -> (Option<f64>, Option<f64>) {
        let body = json!({ "query": ACCOUNT_QUERY }).to_string();
        let answer = self
            .call("POST", &self.binding.graphql_url, Some(body.as_bytes()))
            .and_then(|reply| reply.json());
        let field = |name: &str| {
            answer
                .as_ref()?
                .pointer(&format!("/data/myself/{name}"))
                .and_then(number)
        };
        (field("clientBalance"), field("currentSpendPerHr"))
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

/// Whether a refused create's answer says Runpod has no machine of the GPU type free.
fn no_capacity(body: &[u8]) -> bool {
    String::from_utf8_lossy(body)
        .to_ascii_lowercase()
        .contains(NO_CAPACITY)
}

impl RunpodTransport for RestRunpod {
    fn list_pods(&mut self) -> Result<PodListing, ()> {
        let Some(entries) = self.pods("") else {
            return Ok(PodListing {
                pods: Vec::new(),
                complete: false,
            });
        };
        let parsed: Vec<Option<Pod>> = entries.iter().map(pod_of).collect();
        let complete = parsed.iter().all(Option::is_some);
        Ok(PodListing {
            pods: parsed.into_iter().flatten().collect(),
            complete,
        })
    }

    fn create_pod(&mut self, request: &PodRequest) -> CreateAnswer {
        let body = Value::Object(create_body(request)).to_string();
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
        self.pod(pod_id, "")?
            .get("lastStartedAt")
            .and_then(Value::as_str)
            .and_then(rest_started_at_ms)
    }

    fn pod_placement(&mut self, pod_id: &str) -> Option<PodPlacement> {
        self.pod(pod_id, "?includeMachine=true")
            .as_ref()
            .map(placement_of)
    }

    fn account_spend(&mut self, day_start_ms: u64) -> Option<AccountSpend> {
        let entries = self
            .rest("GET", "/billing/pods?bucketSize=day", None)?
            .json()?;
        spend_of(entries.as_array()?, day_start_ms)
    }
}

impl RunpodAccount for RestRunpod {
    /// `None` when the pod listing failed; a failed balance read leaves only the balance and
    /// spend rate unreported. A pod entry without an id or a name is left out.
    fn account(&mut self) -> Option<AccountSnapshot> {
        let pods = self.pods("?includeMachine=true")?;
        let (balance_usd, spend_per_hour_usd) = self.balance();
        Some(AccountSnapshot {
            balance_usd,
            spend_per_hour_usd,
            pods: pods.iter().filter_map(summary).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{encode, loopback, rest_started_at_ms, target};

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
        assert!(target("http://127.0.0.1:9/v1").is_some());
        assert!(target("http://localhost:9/v1").is_some());
        assert!(target("http://[::1]:9/v1").is_some());
        assert!(target("http://rest.runpod.io/v1").is_none());
        assert!(target("http://10.0.0.1/v1").is_none());
        assert!(target("https://user@rest.runpod.io/v1").is_none());
        assert!(target("ftp://127.0.0.1/").is_none());
        assert!(!loopback("127.0.0.1.example.com"));
        let runpod = target("https://rest.runpod.io/v1").expect("https");
        assert_eq!(
            (runpod.authority, runpod.path.as_str()),
            ("rest.runpod.io", "/v1")
        );
    }

    #[test]
    fn a_query_value_is_percent_encoded() {
        assert_eq!(encode("b10x-llm-qwen"), "b10x-llm-qwen");
        assert_eq!(encode("a b&c=d/"), "a%20b%26c%3Dd%2F");
    }
}
