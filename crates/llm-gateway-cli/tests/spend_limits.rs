//! The owner's read of the spend ceilings in the binary's composition (story:limits-read):
//! `GET /v1/limits` lists each ceiling the pool enforces with its window, what the spend check
//! counts as spent in it, what is left and the last start it refused. Composed in process
//! through the `start_relaying` seam with `EmulatedRunpod` and a ledger file of the test's own;
//! no Runpod API is called and nothing leaves the loopback interface.

use crate::support;

use llm_gateway_cli::{Deployment, load, start_relaying};
use llm_runpod::{AccountSpend, EmulatedRunpod, LedgerEntry, ManualClock, encode_ledger};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::Path,
    sync::Arc,
    time::Duration,
};
use support::{Fixture, OWNER_SECRET, document, mutate, no_spend, with_vllm_key};

const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 24 * HOUR_MS;

/// Connects nowhere: no test here relays a call to a pod.
struct NoPod;

impl llm_gateway_cli::PodConnector for NoPod {
    fn connect(&self, _authority: &str) -> std::io::Result<Box<dyn llm_gateway::RelayStream>> {
        Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused))
    }
}

/// The support document with `caps` on its provider (each a `key = value` line), the model at
/// 2 USD an hour (its startup estimate over the default 600 s is 0.333334 USD), and `ledger`
/// as the provider's `spend_ledger_file` when given.
fn deployment(fixture: &Fixture, caps: &str, ledger: Option<&Path>) -> Deployment {
    let secret = fixture.owner_secret();
    let key = fixture.vllm_key();
    let text = with_vllm_key(&document("127.0.0.1:0", &secret), &key);
    let ledger = ledger.map_or_else(String::new, |path| {
        format!("spend_ledger_file = \"{}\"\n", path.display())
    });
    let text = mutate(
        &text,
        "kind = \"runpod-vllm\"\n",
        &format!("kind = \"runpod-vllm\"\n{caps}{ledger}"),
    );
    let text = mutate(
        &text,
        "max_model_len = 1024\n",
        "max_model_len = 1024\nprice_per_hour_usd = 2\n",
    );
    load(&fixture.config(&text)).unwrap()
}

/// A ledger holding one stopped pod of `small` at 2 USD an hour, from `started_ms` for `run_ms`.
fn ledger_text(started_ms: u64, run_ms: u64) -> String {
    encode_ledger(&[LedgerEntry {
        deployment: "small-1".to_owned(),
        request: "request-before-the-restart".to_owned(),
        pod: Some("pod9".to_owned()),
        model: "small".to_owned(),
        price_per_hour_micro_usd: 2_000_000,
        started_ms,
        stopped_ms: Some(started_ms + run_ms),
    }])
}

/// One request, with the owner credential unless `authorization` names another line (empty:
/// none); the whole answer, read to the close.
fn send(address: SocketAddr, method: &str, path: &str, authorization: Option<&str>) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let owner = format!("authorization: Bearer {OWNER_SECRET}\r\n");
    let line = authorization.unwrap_or(&owner);
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nhost: gateway\r\n{line}\r\n"
    )
    .unwrap();
    let mut answer = String::new();
    drop(stream.read_to_string(&mut answer));
    answer
}

fn limits(address: SocketAddr) -> String {
    let answer = send(address, "GET", "/v1/limits", None);
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    body(&answer).to_owned()
}

fn wake(address: SocketAddr) -> String {
    send(address, "POST", "/v1/models/small/wake", None)
}

fn body(answer: &str) -> &str {
    answer.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

/// One entry of the list as the route writes it.
fn row(
    ceiling: &str,
    cap: &str,
    window: (u64, u64),
    spent_left: Option<(&str, &str)>,
    refusal: Option<(u64, &str)>,
) -> String {
    let (spent, left) = spent_left.map_or_else(
        || ("null".to_owned(), "null".to_owned()),
        |(spent, left)| (format!("\"{spent}\""), format!("\"{left}\"")),
    );
    let refusal = refusal.map_or_else(
        || "null".to_owned(),
        |(at, model)| format!("{{\"at_ms\":{at},\"model\":\"{model}\"}}"),
    );
    format!(
        "{{\"provider\":\"runpod\",\"ceiling\":\"{ceiling}\",\"cap_usd\":\"{cap}\",\
         \"window_start_ms\":{},\"window_end_ms\":{},\"spent_usd\":{spent},\
         \"left_usd\":{left},\"last_refusal\":{refusal}}}",
        window.0, window.1
    )
}

fn list(rows: &[String]) -> String {
    format!(
        "{{\"format\":\"llm-gateway.limits/1\",\"limits\":[{}]}}",
        rows.join(",")
    )
}

fn spend(runpod: &EmulatedRunpod, daily_micro_usd: u64, total_micro_usd: u64) {
    runpod.set_account_spend(Some(AccountSpend {
        daily_micro_usd,
        total_micro_usd,
    }));
}

/// Each ceiling's row reads spent and left from the ledger, which holds more than the account
/// reports, and the daily window is the UTC day of the fixed clock: 5 h into the fourth day.
#[test]
fn limits_each_ceiling_reads_spent_and_left_from_the_ledger_in_the_window_of_the_clock() {
    let fixture = Fixture::new("limits-ledger");
    let now = 3 * DAY_MS + 5 * HOUR_MS;
    // 4.9 h at 2 USD from 1 s into the day: 9.8 USD today, nothing reported.
    let ledger = fixture.write(
        "spend-ledger.json",
        ledger_text(3 * DAY_MS + 1_000, 17_640_000).as_bytes(),
        0o600,
    );
    let deployment = deployment(
        &fixture,
        "daily_cap_usd = 10\ntotal_cap_usd = 100\n",
        Some(&ledger),
    );
    let runpod = EmulatedRunpod::new();
    no_spend(&runpod);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        Arc::new(NoPod),
        Arc::new(ManualClock::new(now)),
    )
    .unwrap();
    let read = limits(running.local_addr());
    running.shutdown();
    assert_eq!(
        read,
        list(&[
            row(
                "daily",
                "10.000000",
                (3 * DAY_MS, 4 * DAY_MS),
                Some(("9.800000", "0.200000")),
                None,
            ),
            row(
                "total",
                "100.000000",
                (0, now),
                Some(("9.800000", "90.200000")),
                None,
            ),
        ])
    );
    assert_eq!(runpod.spend_reads(), 1, "a read is one spend read");
    assert_eq!(runpod.create_calls(), 0);
}

/// The total window is the 30 days up to now once the clock is past them.
#[test]
fn limits_the_total_window_is_the_thirty_days_up_to_now() {
    let fixture = Fixture::new("limits-total-window");
    let now = 40 * DAY_MS + 7 * HOUR_MS;
    let deployment = deployment(&fixture, "total_cap_usd = 100\n", None);
    let runpod = EmulatedRunpod::new();
    spend(&runpod, 1_000_000, 4_030_000);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        Arc::new(NoPod),
        Arc::new(ManualClock::new(now)),
    )
    .unwrap();
    let read = limits(running.local_addr());
    running.shutdown();
    assert_eq!(
        read,
        list(&[row(
            "total",
            "100.000000",
            (now - 30 * DAY_MS, now),
            Some(("4.030000", "95.970000")),
            None,
        )])
    );
}

/// Spent past the cap is read as it is, and left stops at 0.
#[test]
fn limits_left_is_never_below_zero() {
    let fixture = Fixture::new("limits-clamped");
    let deployment = deployment(&fixture, "daily_cap_usd = 10\ntotal_cap_usd = 100\n", None);
    let runpod = EmulatedRunpod::new();
    spend(&runpod, 12_000_000, 12_000_000);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        Arc::new(NoPod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let read = limits(running.local_addr());
    running.shutdown();
    assert_eq!(
        read,
        list(&[
            row(
                "daily",
                "10.000000",
                (0, DAY_MS),
                Some(("12.000000", "0.000000")),
                None,
            ),
            row(
                "total",
                "100.000000",
                (0, 1_000),
                Some(("12.000000", "88.000000")),
                None,
            ),
        ])
    );
}

/// No ceiling refused a start yet: `null`. A wake the daily ceiling refuses names its instant
/// and model on the daily row, and the total row, which refused nothing, stays `null`.
#[test]
fn limits_the_last_refusal_is_null_before_and_names_the_start_the_ceiling_refused() {
    let fixture = Fixture::new("limits-refusal");
    let deployment = deployment(&fixture, "daily_cap_usd = 10\ntotal_cap_usd = 100\n", None);
    let runpod = EmulatedRunpod::new();
    spend(&runpod, 9_666_667, 9_666_667);
    let now = 2 * DAY_MS + HOUR_MS;
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        Arc::new(NoPod),
        Arc::new(ManualClock::new(now)),
    )
    .unwrap();
    let before = limits(running.local_addr());
    let refused = wake(running.local_addr());
    let after = limits(running.local_addr());
    running.shutdown();
    let daily_window = (2 * DAY_MS, 3 * DAY_MS);
    let rows = |refusal| {
        list(&[
            row(
                "daily",
                "10.000000",
                daily_window,
                Some(("9.666667", "0.333333")),
                refusal,
            ),
            row(
                "total",
                "100.000000",
                (0, now),
                Some(("9.666667", "90.333333")),
                None,
            ),
        ])
    };
    assert_eq!(before, rows(None));
    assert!(refused.starts_with("HTTP/1.1 503 "), "{refused}");
    assert!(refused.contains("\"code\":\"spend-ceiling\""), "{refused}");
    assert_eq!(after, rows(Some((now, "small"))));
    assert_eq!(runpod.create_calls(), 0);
}

/// A deployment that declares no ceiling lists none.
#[test]
fn limits_without_a_ceiling_is_an_empty_list() {
    let fixture = Fixture::new("limits-none");
    let deployment = deployment(&fixture, "", None);
    let runpod = EmulatedRunpod::new();
    no_spend(&runpod);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        Arc::new(NoPod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let read = limits(running.local_addr());
    running.shutdown();
    assert_eq!(read, "{\"format\":\"llm-gateway.limits/1\",\"limits\":[]}");
    assert_eq!(runpod.spend_reads(), 0, "no ceiling, no spend read");
}

/// Without the owner credential the read is refused with the status and body `/v1/routes`
/// gets: none, a malformed one, and another token.
#[test]
fn limits_are_refused_without_the_owner_credential_as_the_routes_are() {
    let fixture = Fixture::new("limits-credential");
    let deployment = deployment(&fixture, "daily_cap_usd = 10\n", None);
    let runpod = EmulatedRunpod::new();
    no_spend(&runpod);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        Arc::new(NoPod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let address = running.local_addr();
    let mut seen = Vec::new();
    for (line, code) in [
        ("", "credential-absent"),
        ("authorization: Basic abc\r\n", "credential-malformed"),
        (
            "authorization: Bearer someone-else-0123456789abcdefghijklmn\r\n",
            "credential-rejected",
        ),
    ] {
        let read = send(address, "GET", "/v1/limits", Some(line));
        let routes = send(address, "GET", "/v1/routes", Some(line));
        seen.push((code, read, routes));
    }
    running.shutdown();
    for (code, read, routes) in seen {
        let status = |answer: &str| answer.lines().next().unwrap_or_default().to_owned();
        assert_eq!(status(&read), status(&routes), "{read}");
        assert_eq!(body(&read), body(&routes), "{read}");
        assert!(
            body(&read).contains(&format!("\"code\":\"{code}\"")),
            "{read}"
        );
    }
    assert_eq!(runpod.spend_reads(), 0, "a refused read reads no spend");
}

/// At the boundary the read and the check agree. Spent equal to the cap reads left 0 and a
/// start is refused; spent one start estimate below the cap reads left 0.333334, exactly the
/// estimate, and a start is admitted.
#[test]
fn limits_and_the_spend_check_agree_at_the_boundary() {
    let fixture = Fixture::new("limits-boundary");
    let deployment = deployment(&fixture, "daily_cap_usd = 10\n", None);
    let mut answers = Vec::new();
    for daily in [10_000_000, 9_666_666] {
        let runpod = EmulatedRunpod::new();
        spend(&runpod, daily, daily);
        let running = start_relaying(
            &deployment,
            runpod.clone(),
            Arc::new(NoPod),
            Arc::new(ManualClock::new(1_000)),
        )
        .unwrap();
        let read = limits(running.local_addr());
        let woke = wake(running.local_addr());
        running.shutdown();
        answers.push((read, woke, runpod.create_calls()));
    }
    let (at_cap, refused, refused_creates) = &answers[0];
    assert_eq!(
        *at_cap,
        list(&[row(
            "daily",
            "10.000000",
            (0, DAY_MS),
            Some(("10.000000", "0.000000")),
            None,
        )])
    );
    assert!(refused.starts_with("HTTP/1.1 503 "), "{refused}");
    assert!(refused.contains("\"code\":\"spend-ceiling\""), "{refused}");
    assert_eq!(*refused_creates, 0);
    let (below, admitted, admitted_creates) = &answers[1];
    assert_eq!(
        *below,
        list(&[row(
            "daily",
            "10.000000",
            (0, DAY_MS),
            Some(("9.666666", "0.333334")),
            None,
        )])
    );
    assert!(admitted.starts_with("HTTP/1.1 202 "), "{admitted}");
    assert_eq!(*admitted_creates, 1);
}

/// A spend the check cannot count reads `null` spent and left, every start is refused, and the
/// refusal, which names no ceiling, is on no row.
#[test]
fn limits_an_unknown_spend_reads_null_and_names_no_refusal() {
    let fixture = Fixture::new("limits-unknown");
    let deployment = deployment(&fixture, "daily_cap_usd = 10\n", None);
    let runpod = EmulatedRunpod::new();
    runpod.set_account_spend(None);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        Arc::new(NoPod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let refused = wake(running.local_addr());
    let read = limits(running.local_addr());
    running.shutdown();
    assert!(refused.contains("\"code\":\"spend-ceiling\""), "{refused}");
    assert_eq!(
        read,
        list(&[row("daily", "10.000000", (0, DAY_MS), None, None)])
    );
}
