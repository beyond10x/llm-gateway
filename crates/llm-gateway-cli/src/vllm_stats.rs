//! What a hosted route's vLLM server says about itself, for the dashboard (story:live-dashboard).
//!
//! vLLM's answers carry no cache fields, so a call to such a route has no cache read; the
//! server's `/metrics` counts its prefix cache in tokens. Every [`POLL`], `serve` reads
//! `/metrics` and `<base path>/models` of each hosted route whose `base_url` is a Runpod pod's
//! proxy address (`https://<pod>-<port>.proxy.runpod.net`, where only vLLM images run), with the
//! route's own key, and hands the dashboard the hits and queries counted since it began, the
//! running and waiting requests, the KV cache usage and whether the server listed its models.
//! Only `GET`s are sent; the pod is never changed.

use crate::{
    config::{Deployment, Hosted},
    keys::read_token,
    refusal::{Refusal, StartupRefusal},
    relaying::PodConnector,
    trusted::{self, Wiped},
};
use llm_gateway::{AuthKind, Dashboard, UpstreamStats};
use std::{
    io::{Read as _, Write as _},
    sync::Arc,
    time::Duration,
};

/// How often each server is read.
pub const POLL: Duration = Duration::from_secs(15);
/// How long one read may take to connect, and each read and write.
const TIMEOUT: Duration = Duration::from_secs(5);
/// The most bytes of an answer read.
const MAX_ANSWER: usize = 4 << 20;
/// The address suffix of a Runpod pod's HTTP proxy.
const RUNPOD_PROXY: &str = ".proxy.runpod.net";

/// The pod a Runpod proxy `base_url` reaches, `https://<pod>-<port>.proxy.runpod.net/…`.
pub(crate) fn runpod_pod_id(base_url: &str) -> Option<String> {
    let host = base_url.strip_prefix("https://")?.split('/').next()?;
    let (pod, port) = host.strip_suffix(RUNPOD_PROXY)?.rsplit_once('-')?;
    let valid = !pod.is_empty()
        && pod.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && !port.is_empty()
        && port.bytes().all(|byte| byte.is_ascii_digit());
    valid.then(|| pod.to_owned())
}

/// One reading of a vLLM server's `/metrics`: each figure summed over its label sets.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Sample {
    pub hits: Option<f64>,
    pub queries: Option<f64>,
    pub running: Option<f64>,
    pub waiting: Option<f64>,
    pub kv_usage: Option<f64>,
}

/// The figures of a Prometheus text answer this module reads.
pub(crate) fn parse(text: &str) -> Sample {
    let mut sample = Sample::default();
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        let name_end = line.find(['{', ' ']).unwrap_or(line.len());
        let name = &line[..name_end];
        let rest = match line[name_end..].find('}') {
            Some(close) if line[name_end..].starts_with('{') => &line[name_end + close + 1..],
            _ => &line[name_end..],
        };
        let Some(value) = rest
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite())
        else {
            continue;
        };
        let slot = match name {
            "vllm:prefix_cache_hits_total" => &mut sample.hits,
            "vllm:prefix_cache_queries_total" => &mut sample.queries,
            "vllm:num_requests_running" => &mut sample.running,
            "vllm:num_requests_waiting" => &mut sample.waiting,
            "vllm:kv_cache_usage_perc" => &mut sample.kv_usage,
            _ => continue,
        };
        *slot = Some(slot.unwrap_or(0.0) + value);
    }
    sample
}

/// The prefix cache's hits and queries since the first reading: the sum of each reading's
/// increase. A counter that fell (the server restarted) adds nothing and counts from there.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Tracker {
    last: Option<(f64, f64)>,
    pub hits: u64,
    pub queries: u64,
}

impl Tracker {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub(crate) fn observe(&mut self, sample: &Sample) {
        let (Some(hits), Some(queries)) = (sample.hits, sample.queries) else {
            return;
        };
        if let Some((last_hits, last_queries)) = self.last
            && hits >= last_hits
            && queries >= last_queries
        {
            self.hits = self.hits.saturating_add((hits - last_hits) as u64);
            self.queries = self.queries.saturating_add((queries - last_queries) as u64);
        }
        self.last = Some((hits, queries));
    }
}

/// One `GET`, answered with its status and body; `None` when nothing answered.
pub(crate) trait Fetch: Send {
    fn get(&mut self, authority: &str, path: &str, bearer: Option<&[u8]>)
    -> Option<(u16, Vec<u8>)>;
}

/// `GET` over the pod connector's verified TLS, HTTP/1.1 with `Connection: close`.
pub(crate) struct Https(pub Arc<dyn PodConnector>);

impl Fetch for Https {
    fn get(
        &mut self,
        authority: &str,
        path: &str,
        bearer: Option<&[u8]>,
    ) -> Option<(u16, Vec<u8>)> {
        let mut stream = self.0.connect_probe(authority, TIMEOUT).ok()?;
        let mut request = Wiped::with_capacity(512);
        let mut head = format!(
            "GET {path} HTTP/1.1\r\nhost: {authority}\r\naccept: */*\r\nconnection: close\r\n"
        )
        .into_bytes();
        if let Some(key) = bearer {
            head.extend_from_slice(b"authorization: Bearer ");
            head.extend_from_slice(key);
            head.extend_from_slice(b"\r\n");
        }
        head.extend_from_slice(b"\r\n");
        request.extend(&head);
        head.fill(0);
        stream.write_all(request.as_slice()).ok()?;
        drop(request);
        let mut answer = Vec::new();
        let mut piece = [0_u8; 16 << 10];
        loop {
            match stream.read(&mut piece) {
                Ok(0) => break,
                Ok(read) => {
                    answer.extend_from_slice(&piece[..read]);
                    if answer.len() > MAX_ANSWER {
                        return None;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) if !answer.is_empty() => break,
                Err(_) => return None,
            }
        }
        split_answer(&answer)
    }
}

/// The status and body of an HTTP/1.1 answer read to its end, chunked or not.
fn split_answer(answer: &[u8]) -> Option<(u16, Vec<u8>)> {
    let end = answer.windows(4).position(|window| window == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&answer[..end]).ok()?;
    let status = head.split(' ').nth(1)?.parse().ok()?;
    let body = &answer[end + 4..];
    let chunked = head.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.trim().eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
        })
    });
    Some((
        status,
        if chunked {
            dechunk(body)?
        } else {
            body.to_vec()
        },
    ))
}

fn dechunk(mut body: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = body.windows(2).position(|window| window == b"\r\n")?;
        let size = std::str::from_utf8(&body[..line_end]).ok()?;
        let size = usize::from_str_radix(size.split(';').next()?.trim(), 16).ok()?;
        body = &body[line_end + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(body.get(..size)?);
        body = body.get(size + 2..)?;
    }
}

/// One hosted route whose server is read.
pub(crate) struct Watched {
    pub alias: String,
    authority: String,
    base_path: String,
    key: Option<Wiped>,
    tracker: Tracker,
}

impl Watched {
    /// One reading: `/metrics`, then `<base path>/models`.
    pub(crate) fn read(&mut self, fetch: &mut dyn Fetch, now_ms: u64) -> UpstreamStats {
        let key = self.key.as_ref().map(Wiped::as_slice);
        let sample = fetch
            .get(&self.authority, "/metrics", key)
            .filter(|(status, _)| *status == 200)
            .map(|(_, body)| parse(&String::from_utf8_lossy(&body)))
            .unwrap_or_default();
        self.tracker.observe(&sample);
        let models = format!("{}/models", self.base_path.trim_end_matches('/'));
        let ready = fetch
            .get(&self.authority, &models, key)
            .map(|(status, _)| status == 200);
        UpstreamStats {
            source: "vllm".to_owned(),
            read_ms: now_ms,
            prefix_cache_hit_tokens: self.tracker.hits,
            prefix_cache_query_tokens: self.tracker.queries,
            requests_running: sample.running,
            requests_waiting: sample.waiting,
            kv_cache_usage: sample.kv_usage,
            ready,
        }
    }
}

/// The hosted routes on a Runpod pod's proxy: alias and pod id.
pub(crate) fn pod_routes(deployment: &Deployment) -> Vec<(String, String)> {
    deployment
        .hosted
        .iter()
        .filter_map(|(alias, hosted)| Some((alias.clone(), runpod_pod_id(&hosted.base_url)?)))
        .collect()
}

/// The route's bearer key, from its file or its keychain entry; `None` for a route that sends
/// none.
fn bearer(alias: &str, hosted: &Hosted) -> Result<Option<Wiped>, Refusal> {
    if hosted.auth_kind != AuthKind::Bearer {
        return Ok(None);
    }
    let rules = (
        StartupRefusal::EndpointCredentialTooLarge,
        StartupRefusal::EndpointCredentialNotAToken,
    );
    let key = match (&hosted.credential_file, &hosted.credential_keychain) {
        (Some(path), _) => read_token(
            path,
            &format!("hosted.{alias}.credential_file"),
            &trusted::ENDPOINT_CREDENTIAL,
            rules,
        )?,
        (None, Some(reference)) => crate::keychain::read_keychain_token(
            reference,
            &format!("hosted.{alias}.credential"),
            (
                StartupRefusal::EndpointCredentialUnreadable,
                rules.0,
                rules.1,
            ),
        )?,
        (None, None) => return Ok(None),
    };
    Ok(Some(Wiped::adopt(key)))
}

/// Every route [`pod_routes`] names, with its key read once.
pub(crate) fn watched(deployment: &Deployment) -> Result<Vec<Watched>, Refusal> {
    let mut out = Vec::new();
    for (alias, _) in pod_routes(deployment) {
        let hosted = &deployment.hosted[&alias];
        let rest = hosted.base_url.strip_prefix("https://").unwrap_or_default();
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        out.push(Watched {
            authority: authority.to_owned(),
            base_path: format!("/{path}"),
            key: bearer(&alias, hosted)?,
            alias,
            tracker: Tracker::default(),
        });
    }
    Ok(out)
}

/// Reads every route in `routes` every `every` into `dashboard`, until the returned handle drops.
pub(crate) fn start(
    mut routes: Vec<Watched>,
    mut fetch: Box<dyn Fetch>,
    dashboard: Arc<Dashboard>,
    every: Duration,
) -> crate::account::Polling {
    crate::account::every("vllm-stats", every, move || {
        for route in &mut routes {
            let stats = route.read(fetch.as_mut(), crate::heartbeat::unix_ms());
            dashboard.set_upstream(&route.alias, Some(stats));
        }
    })
}

/// The reader for `deployment`'s pod routes; `None` when it has none.
pub(crate) fn start_for(
    deployment: &Deployment,
    pods: &crate::ProxyConnector,
    dashboard: Arc<Dashboard>,
) -> Result<Option<crate::account::Polling>, Refusal> {
    let routes = watched(deployment)?;
    if routes.is_empty() {
        return Ok(None);
    }
    Ok(Some(start(
        routes,
        Box::new(Https(Arc::new(pods.clone()))),
        dashboard,
        POLL,
    )))
}

#[cfg(test)]
mod tests {
    use super::{Fetch, Sample, Tracker, Watched, parse, runpod_pod_id, split_answer};
    use crate::trusted::Wiped;

    /// The figures as a vLLM 0.27 server with two label sets writes them.
    const METRICS: &str = "# HELP vllm:prefix_cache_hits_total Prefix cache hits, in terms of number of cached tokens.\n\
        # TYPE vllm:prefix_cache_hits_total counter\n\
        vllm:prefix_cache_hits_total{engine=\"0\",model_name=\"qwen3.6-35b-a3b\"} 3031168.0\n\
        vllm:prefix_cache_queries_total{engine=\"0\",model_name=\"qwen3.6-35b-a3b\"} 6634067.0\n\
        vllm:num_requests_running{engine=\"0\",model_name=\"qwen3.6-35b-a3b\"} 2.0\n\
        vllm:num_requests_waiting{engine=\"0\",model_name=\"qwen3.6-35b-a3b\"} 1.0\n\
        vllm:kv_cache_usage_perc{engine=\"0\",model_name=\"qwen3.6-35b-a3b\"} 0.25\n\
        vllm:kv_cache_usage_perc{engine=\"1\",model_name=\"qwen3.6-35b-a3b\"} 0.125\n\
        vllm:prefix_cache_hits_created{engine=\"0\"} 1.7e9\n";

    #[test]
    fn a_runpod_proxy_address_names_its_pod() {
        assert_eq!(
            runpod_pod_id("https://abc123xyz-8000.proxy.runpod.net/v1").as_deref(),
            Some("abc123xyz")
        );
        assert_eq!(runpod_pod_id("https://api.anthropic.com"), None);
        assert_eq!(runpod_pod_id("http://abc-8000.proxy.runpod.net/v1"), None);
        assert_eq!(runpod_pod_id("https://-8000.proxy.runpod.net/v1"), None);
    }

    #[test]
    fn the_vllm_figures_are_read_and_summed_over_label_sets() {
        assert_eq!(
            parse(METRICS),
            Sample {
                hits: Some(3_031_168.0),
                queries: Some(6_634_067.0),
                running: Some(2.0),
                waiting: Some(1.0),
                kv_usage: Some(0.375),
            }
        );
    }

    #[test]
    fn the_hit_rate_counts_the_increase_since_the_first_reading_and_survives_a_restart() {
        let mut tracker = Tracker::default();
        let reading = |hits: f64, queries: f64| Sample {
            hits: Some(hits),
            queries: Some(queries),
            ..Sample::default()
        };
        tracker.observe(&reading(3_031_168.0, 6_634_067.0));
        assert_eq!((tracker.hits, tracker.queries), (0, 0));
        tracker.observe(&reading(3_031_268.0, 6_634_267.0));
        assert_eq!((tracker.hits, tracker.queries), (100, 200));
        tracker.observe(&reading(10.0, 40.0));
        assert_eq!((tracker.hits, tracker.queries), (100, 200));
        tracker.observe(&reading(70.0, 100.0));
        assert_eq!((tracker.hits, tracker.queries), (160, 260));
    }

    #[test]
    fn a_chunked_answer_is_joined() {
        let answer = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\n1\r\n!\r\n0\r\n\r\n";
        assert_eq!(split_answer(answer), Some((200, b"hello!".to_vec())));
        assert_eq!(
            split_answer(b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\n\r\n"),
            Some((401, Vec::new()))
        );
    }

    /// Answers `/metrics` from a list of readings and `/v1/models` with 200, recording the key.
    struct Fixture {
        readings: Vec<String>,
        seen: Vec<(String, Option<Vec<u8>>)>,
    }

    impl Fetch for Fixture {
        fn get(
            &mut self,
            authority: &str,
            path: &str,
            bearer: Option<&[u8]>,
        ) -> Option<(u16, Vec<u8>)> {
            assert_eq!(authority, "pod1-8000.proxy.runpod.net");
            self.seen
                .push((path.to_owned(), bearer.map(<[u8]>::to_vec)));
            match path {
                "/metrics" => Some((200, self.readings.remove(0).into_bytes())),
                "/v1/models" => Some((200, b"{}".to_vec())),
                _ => None,
            }
        }
    }

    #[test]
    fn a_route_reading_feeds_the_dashboard_a_server_side_hit_rate() {
        let next = METRICS
            .replace("3031168.0", "3031468.0")
            .replace("6634067.0", "6634467.0");
        let mut fixture = Fixture {
            readings: vec![METRICS.to_owned(), next],
            seen: Vec::new(),
        };
        let mut route = Watched {
            alias: "qwen".to_owned(),
            authority: "pod1-8000.proxy.runpod.net".to_owned(),
            base_path: "/v1".to_owned(),
            key: Some(Wiped::adopt(b"route-key".to_vec())),
            tracker: Tracker::default(),
        };
        let dashboard = llm_gateway::Dashboard::default();
        route.read(&mut fixture, 1);
        let stats = route.read(&mut fixture, 2);
        assert_eq!(stats.hit_ratio(), Some(0.75));
        assert_eq!(stats.ready, Some(true));
        assert_eq!(stats.kv_cache_usage, Some(0.375));
        assert!(
            fixture
                .seen
                .iter()
                .all(|(_, key)| key.as_deref() == Some(&b"route-key"[..]))
        );
        dashboard.set_upstream("qwen", Some(stats));
        let metrics = dashboard.render_prometheus();
        assert!(
            metrics.contains("llmgw_upstream_prefix_cache_hit_ratio{route=\"qwen\"} 0.75\n"),
            "{metrics}"
        );
        assert!(
            metrics.contains("llmgw_upstream_requests_running{route=\"qwen\"} 2\n"),
            "{metrics}"
        );
        let snapshot = dashboard.snapshot();
        assert!(
            snapshot.contains("\"server_cache_hit_rate\":0.750"),
            "{snapshot}"
        );
        assert!(
            snapshot.contains("\"server_cache_hit_tokens\":300"),
            "{snapshot}"
        );
    }
}
