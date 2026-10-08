//! The binary's logs and its `GET /metrics` (story:gateway-observability), each test named after
//! the row of `docs/llmgw-capability-matrix.md` it closes: O3 and R4. The specification is
//! `spec/domains/telemetry.yaml` and the standard-error lines of `spec/domains/deployment.yaml`.
//! Nothing leaves the loopback interface.

mod support;

use llm_gateway::{Disposition, RefusalCode, UsageRecord, UsageRecords, Wire};
use llm_gateway_cli::{TracingRecords, log_filter};
use rustix::process::{Pid, Signal, kill_process};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
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
        model: Some("small".to_owned()),
        wire: Wire::Chat,
        disposition,
        refusal,
        status,
        target_status: (refusal.is_none()).then_some(status),
        reported_model: None,
        input_tokens: None,
        output_tokens: None,
        cached_input_tokens: None,
        cache_creation_input_tokens: None,
        reasoning_output_tokens: None,
        response_bytes: 10,
        duration_ms: 7,
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
    assert!(lines[0].contains(" status=200"), "{}", lines[0]);
    assert!(lines[0].contains("target_status=200"), "{}", lines[0]);
    assert!(!lines[0].contains("refusal="), "{}", lines[0]);
    assert!(
        lines[1].contains("disposition=UpstreamFailed"),
        "{}",
        lines[1]
    );
    assert!(lines[1].contains("refusal=upstream-failed"), "{}", lines[1]);
    assert!(lines[1].contains(" status=502"), "{}", lines[1]);
    assert!(!lines[1].contains("target_status="), "{}", lines[1]);
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
    write!(
        stream,
        "GET /metrics HTTP/1.1\r\nhost: g\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
    )
    .unwrap();
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
