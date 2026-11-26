//! The binary's logs and its `GET /metrics` (story:gateway-observability), each test named after
//! the row of `docs/llmgw-capability-matrix.md` it closes: O3 and R4. The specification is
//! `spec/domains/telemetry.yaml` and the standard-error lines of `spec/domains/deployment.yaml`.
//! Nothing leaves the loopback interface.

mod support;

use llm_gateway::{Disposition, Outcome, RefusalCode, UsageRecord, UsageRecords, Wire};
use llm_gateway_cli::{TracingRecords, log_filter};
use rustix::process::{Pid, Signal, kill_process};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::{Shutdown, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};
use support::{Fixture, OWNER_SECRET};

const LISTENING: &str = "b10x-llm-gateway: listening on ";
const WAIT: Duration = Duration::from_secs(10);

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
}

/// Everything a subscriber writes, kept in memory.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Captured {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

fn record(disposition: Disposition, refusal: Option<RefusalCode>, status: u16) -> UsageRecord {
    UsageRecord {
        call: "0123456789abcdef0123456789abcdef-1".to_owned(),
        model: Some("small".to_owned()),
        wire: Wire::Chat,
        disposition,
        outcome: Outcome::of(refusal, false),
        refusal,
        status,
        target_status: (refusal.is_none()).then_some(status),
        reported_model: None,
        input_tokens: None,
        output_tokens: None,
        cached_input_tokens: None,
        cache_creation_input_tokens: None,
        reasoning_output_tokens: None,
        cost_usd: None,
        response_bytes: 10,
        duration_ms: 7,
        profile: None,
        principal: None,
    }
}

/// What `TracingRecords` writes for `records` under the binary's filter for `rust_log`.
fn logged(rust_log: Option<&str>, records: &[UsageRecord]) -> String {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(log_filter(rust_log))
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        for record in records {
            TracingRecords.record(record);
        }
    });
    captured.text()
}

#[test]
fn o3_each_usage_record_is_one_structured_info_event() {
    let text = logged(
        None,
        &[
            record(Disposition::Relayed, None, 200),
            record(
                Disposition::UpstreamFailed,
                Some(RefusalCode::UpstreamFailed),
                502,
            ),
        ],
    );
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    for line in &lines {
        assert!(line.contains(" INFO "), "{line}");
        for field in [
            "model=\"small\"",
            "wire=chat",
            "response_bytes=10",
            "duration_ms=7",
        ] {
            assert!(line.contains(field), "{field} is missing from {line}");
        }
    }
    assert!(lines[0].contains("disposition=Relayed"), "{}", lines[0]);
    assert!(lines[0].contains("outcome=ok"), "{}", lines[0]);
    assert!(lines[0].contains(" status=200"), "{}", lines[0]);
    assert!(lines[0].contains("target_status=200"), "{}", lines[0]);
    assert!(!lines[0].contains("refusal="), "{}", lines[0]);
    assert!(
        lines[1].contains("disposition=UpstreamFailed"),
        "{}",
        lines[1]
    );
    assert!(lines[1].contains("refusal=upstream-failed"), "{}", lines[1]);
    assert!(lines[1].contains("outcome=upstream_error"), "{}", lines[1]);
    assert!(lines[1].contains(" status=502"), "{}", lines[1]);
    assert!(!lines[1].contains("target_status="), "{}", lines[1]);
}

/// story:call-id-and-usage-record: the event names the call and carries each token counter the
/// record has, under the call record's key names; a counter the record does not carry is no
/// field, never `0`.
#[test]
fn o3_the_usage_event_names_the_call_and_carries_the_token_counters_the_record_has() {
    let mut reported = record(Disposition::Relayed, None, 200);
    reported.input_tokens = Some(11);
    reported.output_tokens = Some(0);
    reported.cached_input_tokens = Some(3);
    reported.reasoning_output_tokens = Some(5);
    let mut cache_written = record(Disposition::Relayed, None, 200);
    cache_written.cache_creation_input_tokens = Some(64);
    let text = logged(
        None,
        &[
            reported,
            record(Disposition::Relayed, None, 200),
            cache_written,
        ],
    );
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    for line in &lines {
        assert!(
            line.contains(" call=0123456789abcdef0123456789abcdef-1"),
            "{line}"
        );
    }
    for (line, present, absent) in [
        (
            lines[0],
            &["input_tokens=11", "output_tokens=0", "cache_read_tokens=3"][..],
            &["cache_creation_tokens", "reasoning"][..],
        ),
        (
            lines[1],
            &[][..],
            &[
                "input_tokens",
                "output_tokens",
                "cache_read_tokens",
                "cache_creation_tokens",
            ][..],
        ),
        (
            lines[2],
            &["cache_creation_tokens=64"][..],
            &["input_tokens", "output_tokens", "cache_read_tokens"][..],
        ),
    ] {
        for field in present {
            assert!(line.contains(field), "{field} is missing from {line}");
        }
        for field in absent {
            assert!(!line.contains(field), "{field} is in {line}");
        }
    }
}

#[test]
fn o3_the_level_comes_from_rust_log_and_defaults_to_info() {
    let one = [record(Disposition::Relayed, None, 200)];
    assert_eq!(logged(None, &one).lines().count(), 1);
    assert_eq!(logged(Some("info"), &one).lines().count(), 1);
    assert_eq!(logged(Some("debug"), &one).lines().count(), 1);
    assert_eq!(logged(Some("warn"), &one), "");
    assert_eq!(logged(Some("off"), &one), "");
    // A value that names no level is not a reason to go quiet or to start shouting.
    assert_eq!(logged(Some(""), &one).lines().count(), 1);
    // A directive that cannot be read is dropped; the readable ones beside it still apply.
    assert_eq!(
        logged(Some("llm_gateway_cli=loud"), &one).lines().count(),
        1
    );
    assert_eq!(logged(Some("llm_gateway_cli=loud,warn"), &one), "");
}

/// Runs the binary on `config` with `RUST_LOG` set to `rust_log` (removed for `None`), scrapes
/// `GET /metrics` as the owner, stops it with SIGTERM, and returns its exit status, every line
/// it wrote to standard error and the scrape.
fn serve_once(config: &Path, rust_log: Option<&str>) -> (Option<i32>, Vec<String>, String) {
    serve_request(
        config,
        rust_log,
        &format!(
            "GET /metrics HTTP/1.1\r\nhost: g\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
        ),
    )
}

/// As [`serve_once`], sending `request` in place of the scrape and returning its answer.
fn serve_request(
    config: &Path,
    rust_log: Option<&str>,
    request: &str,
) -> (Option<i32>, Vec<String>, String) {
    let mut command = Command::new(binary());
    command
        .args(["--config", config.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    match rust_log {
        Some(value) => command.env("RUST_LOG", value),
        None => command.env_remove("RUST_LOG"),
    };
    let mut child = command.spawn().unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut lines = Vec::new();
    let address: SocketAddr = loop {
        let mut line = String::new();
        assert!(
            stderr.read_line(&mut line).unwrap() > 0,
            "the gateway exited before listening: {lines:?}"
        );
        let line = line.trim_end().to_owned();
        let listening = line.strip_prefix(LISTENING).map(str::to_owned);
        lines.push(line);
        if let Some(address) = listening {
            break address.parse().unwrap();
        }
    };
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    // The request is whole: a refusal the gateway lingers after ends at once.
    stream.shutdown(Shutdown::Write).unwrap();
    let mut scraped = String::new();
    drop(stream.read_to_string(&mut scraped));
    kill_process(Pid::from_child(&child), Signal::TERM).unwrap();
    let mut rest = String::new();
    stderr.read_to_string(&mut rest).unwrap();
    lines.extend(rest.lines().map(str::to_owned));
    let status = child.wait().unwrap();
    (status.code(), lines, scraped)
}

#[test]
fn o3_the_binary_writes_debug_events_only_when_rust_log_asks_and_its_lines_stay_as_published() {
    let fixture = Fixture::new("o3-binary");
    let config = fixture.valid_config("127.0.0.1:0");
    let (status, quiet, _) = serve_once(&config, None);
    assert_eq!(status, Some(0), "{quiet:?}");
    // At the default level the process writes exactly its published lines.
    assert_eq!(quiet.len(), 2, "{quiet:?}");
    assert!(quiet[0].starts_with(LISTENING), "{quiet:?}");
    assert!(
        quiet[1].starts_with("b10x-llm-gateway: stopped by SIGTERM accepted="),
        "{quiet:?}"
    );
    let (status, verbose, _) = serve_once(&config, Some("debug"));
    assert_eq!(status, Some(0), "{verbose:?}");
    assert!(
        verbose
            .iter()
            .any(|line| line.contains(" DEBUG ") && line.contains("deployment loaded")),
        "{verbose:?}"
    );
    assert!(verbose.iter().any(|line| line.starts_with(LISTENING)));
}

/// story:call-id-and-usage-record: a model call the binary refuses at its head (`GET` on a wire
/// path) carries an id, and its `usage` event names that id; a second process gives out ids
/// whose per-process part is not the first's, so no id repeats after a restart.
#[test]
fn o3_each_process_gives_out_its_own_call_ids_and_the_usage_event_names_the_call() {
    let fixture = Fixture::new("o3-call-ids");
    let config = fixture.valid_config("127.0.0.1:0");
    let request = format!(
        "GET /v1/chat/completions HTTP/1.1\r\nhost: g\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
    );
    let mut parts = Vec::new();
    for _ in 0..2 {
        let (status, lines, answered) = serve_request(&config, None, &request);
        assert_eq!(status, Some(0), "{lines:?}");
        assert!(answered.starts_with("HTTP/1.1 405 "), "{answered}");
        let id = answered
            .split("\r\n")
            .find_map(|line| line.strip_prefix("x-llm-gateway-call: "))
            .unwrap_or_else(|| panic!("no call id in {answered}"))
            .to_owned();
        assert!(
            lines
                .iter()
                .any(|line| line.contains(" usage:") && line.contains(&format!(" call={id}"))),
            "no usage event names {id}: {lines:?}"
        );
        let (part, counter) = id.rsplit_once('-').unwrap();
        assert_eq!(counter, "1", "the first id a process gives out: {id}");
        parts.push(part.to_owned());
    }
    assert_ne!(parts[0], parts[1], "two processes gave out the same ids");
}

#[test]
fn r4_the_binary_serves_its_counters_to_the_owner() {
    let fixture = Fixture::new("r4-binary");
    let config = fixture.valid_config("127.0.0.1:0");
    let (status, lines, scraped) = serve_once(&config, None);
    assert_eq!(status, Some(0), "{lines:?}");
    assert!(scraped.starts_with("HTTP/1.1 200 "), "{scraped}");
    assert!(
        scraped.contains("\r\n\r\n# HELP llmgw_inference_requests_total "),
        "{scraped}"
    );
    assert!(
        scraped.contains("\nllmgw_pod_starts_total 0\n"),
        "{scraped}"
    );
}

/// A chat answer reporting 1200 prompt tokens, 200 of them cached, and 300 completion tokens.
const PRICED_ANSWER: &str = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 113\r\nconnection: close\r\n\r\n{\"id\":\"c-1\",\"usage\":{\"prompt_tokens\":1200,\"completion_tokens\":300,\"prompt_tokens_details\":{\"cached_tokens\":200}}}";

/// A fake upstream on a loopback port: answers each of `count` connections with `answer` once
/// it has read the request's head and its `content-length` body.
fn upstream(count: usize, answer: &'static str) -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for _ in 0..count {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            stream.set_read_timeout(Some(WAIT)).unwrap();
            let mut seen = Vec::new();
            let mut chunk = [0_u8; 4096];
            let complete = |seen: &[u8]| {
                let text = String::from_utf8_lossy(seen);
                let Some((head, body)) = text.split_once("\r\n\r\n") else {
                    return false;
                };
                let length = head
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                body.len() >= length
            };
            while !complete(&seen) {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => seen.extend_from_slice(&chunk[..read]),
                }
            }
            drop(stream.write_all(answer.as_bytes()));
        }
    });
    address
}

/// story:cost-per-call, through the binary: a `[hosted.<alias>.token_price]` model's call is
/// logged with the cost its reported tokens come to, by llm's pricing (1000 uncached input
/// tokens at 3, 200 cached at 0.3 and 300 output at 15 USD per million: 0.00756), and the same
/// call of a model declaring no price is logged without one.
#[test]
fn cost_the_binary_logs_the_cost_of_a_priced_call_and_none_for_an_unpriced_one() {
    let fixture = Fixture::new("cost-binary");
    let pod = upstream(2, PRICED_ANSWER);
    let config = fixture.config(&format!(
        "listen = \"127.0.0.1:0\"\nowner_secret_file = \"{}\"\n\n\
         [hosted.priced]\nwires = [\"chat\"]\nbase_url = \"http://{pod}/v1\"\nauth_kind = \"anonymous\"\n\n\
         [hosted.priced.token_price]\ninput = \"3\"\noutput = \"15\"\ncache_read = \"0.3\"\n\n\
         [hosted.plain]\nwires = [\"chat\"]\nbase_url = \"http://{pod}/v1\"\nauth_kind = \"anonymous\"\n",
        fixture.owner_secret().display()
    ));
    let mut child = Command::new(binary())
        .args(["--config", config.to_str().unwrap()])
        .env_remove("RUST_LOG")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut lines = Vec::new();
    let address: SocketAddr = loop {
        let mut line = String::new();
        assert!(
            stderr.read_line(&mut line).unwrap() > 0,
            "the gateway exited before listening: {lines:?}"
        );
        let line = line.trim_end().to_owned();
        let listening = line.strip_prefix(LISTENING).map(str::to_owned);
        lines.push(line);
        if let Some(address) = listening {
            break address.parse().unwrap();
        }
    };
    for alias in ["priced", "plain"] {
        let body = format!("{{\"model\":\"{alias}\",\"messages\":[]}}");
        let mut stream = TcpStream::connect(address).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        write!(
            stream,
            "POST /v1/chat/completions HTTP/1.1\r\nhost: g\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut answer = String::new();
        drop(stream.read_to_string(&mut answer));
        assert!(answer.starts_with("HTTP/1.1 200 "), "{alias}: {answer}");
    }
    kill_process(Pid::from_child(&child), Signal::TERM).unwrap();
    let mut rest = String::new();
    stderr.read_to_string(&mut rest).unwrap();
    lines.extend(rest.lines().map(str::to_owned));
    assert_eq!(child.wait().unwrap().code(), Some(0), "{lines:?}");
    let usage = |alias: &str| {
        lines
            .iter()
            .find(|line| line.contains(" usage: ") && line.contains(&format!("model=\"{alias}\"")))
            .unwrap_or_else(|| panic!("no usage event for {alias}: {lines:?}"))
            .clone()
    };
    assert!(
        usage("priced").contains(" cost_usd=\"0.00756\""),
        "{}",
        usage("priced")
    );
    assert!(!usage("plain").contains("cost_usd"), "{}", usage("plain"));
}

/// Adversary pass, wave 2026-11-25-w03. `website/docs/getting-started.md` and
/// `website/docs/guides/scrape-the-counters.md` show one `usage` event and say it is "pasted
/// from a run of the binary": the refused call of the getting-started run, the only model call
/// it makes. The binary's event for that record, after its timestamp, must be the page's line
/// field for field, and the id of a process's only call ends in its counter `1`.
#[test]
fn adv_w03_the_usage_event_the_site_pages_show_is_one_the_binary_writes() {
    for (page, text) in [
        (
            "getting-started.md",
            include_str!("../../../website/docs/getting-started.md"),
        ),
        (
            "guides/scrape-the-counters.md",
            include_str!("../../../website/docs/guides/scrape-the-counters.md"),
        ),
    ] {
        let shown = text
            .lines()
            .find(|line| line.contains(" INFO usage: usage record "))
            .unwrap_or_else(|| panic!("{page} shows no usage event"));
        let (_, shown_fields) = shown.split_once(" INFO ").unwrap();
        let id = shown_fields
            .split(' ')
            .find_map(|field| field.strip_prefix("call="))
            .unwrap_or_else(|| panic!("{page}: the event names no call: {shown}"));
        let refusal = Some(RefusalCode::TargetUnavailable);
        let written = logged(
            None,
            &[UsageRecord {
                call: id.to_owned(),
                model: Some("small".to_owned()),
                wire: Wire::Chat,
                disposition: Disposition::Refused,
                outcome: Outcome::of(refusal, false),
                refusal,
                status: 503,
                target_status: None,
                reported_model: None,
                input_tokens: None,
                output_tokens: None,
                cached_input_tokens: None,
                cache_creation_input_tokens: None,
                reasoning_output_tokens: None,
                cost_usd: None,
                response_bytes: 0,
                duration_ms: 0,
                profile: None,
                principal: None,
            }],
        );
        let line = written.lines().next().unwrap();
        let (_, written_fields) = line.split_once(" INFO ").unwrap();
        let counter = id.rsplit_once('-').map(|(_, counter)| counter);
        assert_eq!(
            (shown_fields, counter),
            (written_fields, Some("1")),
            "{page}: the page's event against the binary's for the same record (left: the page)"
        );
    }
}
