//! The spend ledger in the binary's composition (story:spend-cap-counts-earlier-pods): a
//! provider with a spend ceiling keeps its pool's ledger in the file `spend_ledger_file` names,
//! read once at start through the trusted-file rules and rewritten whole after each change.
//! Composed in process through the `start_relaying` seam with `EmulatedRunpod`; no Runpod API is
//! called and nothing leaves the loopback interface.

use crate::support;

use llm_gateway_cli::{Deployment, StartupRefusal, load, start_relaying};
use llm_runpod::{EmulatedRunpod, LedgerEntry, ManualClock, decode_ledger, encode_ledger};
use std::{
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::Arc,
    time::Duration,
};
use support::{Fixture, OWNER_SECRET, document, mutate, no_spend, with_vllm_key};

const HOUR_MS: u64 = 3_600_000;

/// Connects nowhere: no test here relays a call to a pod.
struct NoPod;

impl llm_gateway_cli::PodConnector for NoPod {
    fn connect(&self, _authority: &str) -> std::io::Result<Box<dyn llm_gateway::RelayStream>> {
        Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused))
    }
}

/// The support document with a daily cap of 10 USD on its provider, the model at 2 USD an hour
/// (its startup estimate is a third of a dollar), and `ledger` as the provider's
/// `spend_ledger_file`.
fn deployment(fixture: &Fixture, ledger: &Path) -> Deployment {
    let secret = fixture.owner_secret();
    let key = fixture.vllm_key();
    let text = with_vllm_key(&document("127.0.0.1:0", &secret), &key);
    let text = mutate(
        &text,
        "kind = \"runpod-vllm\"\n",
        &format!(
            "kind = \"runpod-vllm\"\ndaily_cap_usd = 10\nspend_ledger_file = \"{}\"\n",
            ledger.display()
        ),
    );
    let text = mutate(
        &text,
        "max_model_len = 1024\n",
        "max_model_len = 1024\nprice_per_hour_usd = 2\n",
    );
    load(&fixture.config(&text)).unwrap()
}

/// A ledger holding one stopped pod of `small` that ran `hours` at 2 USD an hour from 1 s.
fn ledger_text(run_ms: u64) -> String {
    encode_ledger(&[LedgerEntry {
        deployment: "small-1".to_owned(),
        request: "request-before-the-restart".to_owned(),
        pod: Some("pod9".to_owned()),
        model: "small".to_owned(),
        price_per_hour_micro_usd: 2_000_000,
        started_ms: 1_000,
        stopped_ms: Some(1_000 + run_ms),
    }])
}

fn wake(address: SocketAddr) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "POST /v1/models/small/wake HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
    )
    .unwrap();
    let mut answer = String::new();
    drop(stream.read_to_string(&mut answer));
    answer
}

/// Composes the gateway at 5 h into the first UTC day and wakes `small` once.
fn wake_once(deployment: &Deployment, runpod: &EmulatedRunpod) -> String {
    let running = start_relaying(
        deployment,
        runpod.clone(),
        Arc::new(NoPod),
        Arc::new(ManualClock::new(5 * HOUR_MS)),
    )
    .unwrap();
    let answer = wake(running.local_addr());
    running.shutdown();
    answer
}

#[test]
fn spend_ledger_a_started_gateway_reads_its_ledger_and_refuses_a_wake_it_puts_over_the_cap() {
    let fixture = Fixture::new("spend-ledger-over");
    // 4.9 h at 2 USD: 9.8 USD today, nothing reported; the start's 0.333334 passes 10.
    let ledger = fixture.write(
        "spend-ledger.json",
        ledger_text(17_640_000).as_bytes(),
        0o600,
    );
    let runpod = EmulatedRunpod::new();
    no_spend(&runpod);
    let answer = wake_once(&deployment(&fixture, &ledger), &runpod);
    assert!(answer.starts_with("HTTP/1.1 503 "), "{answer}");
    assert!(answer.contains("\"code\":\"spend-ceiling\""), "{answer}");
    assert_eq!(runpod.create_calls(), 0);
}

#[test]
fn spend_ledger_a_started_pod_is_written_to_the_ledger_file_private() {
    let fixture = Fixture::new("spend-ledger-written");
    let ledger = fixture.path("state").join("spend-ledger.json");
    let runpod = EmulatedRunpod::new();
    no_spend(&runpod);
    let answer = wake_once(&deployment(&fixture, &ledger), &runpod);
    assert!(answer.starts_with("HTTP/1.1 202 "), "{answer}");
    assert_eq!(runpod.create_calls(), 1);
    let entries = decode_ledger(&fs::read_to_string(&ledger).unwrap()).expect("a ledger");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].model, "small");
    assert_eq!(entries[0].pod.as_deref(), Some("pod1"));
    assert_eq!(entries[0].price_per_hour_micro_usd, 2_000_000);
    assert_eq!(entries[0].started_ms, 5 * HOUR_MS);
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&ledger), 0o600);
    assert_eq!(mode(ledger.parent().unwrap()), 0o700);
}

#[test]
fn spend_ledger_a_ledger_file_the_trusted_rules_refuse_refuses_every_wake_as_unknown() {
    let fixture = Fixture::new("spend-ledger-refused");
    // Within the cap, but writable by the group: unread, so the spend is unknown.
    let text = ledger_text(HOUR_MS);
    let ledger = fixture.write("spend-ledger.json", text.as_bytes(), 0o620);
    let runpod = EmulatedRunpod::new();
    no_spend(&runpod);
    let answer = wake_once(&deployment(&fixture, &ledger), &runpod);
    assert!(answer.starts_with("HTTP/1.1 503 "), "{answer}");
    assert!(answer.contains("\"code\":\"spend-ceiling\""), "{answer}");
    assert_eq!(runpod.create_calls(), 0);
    assert_eq!(
        fs::read_to_string(&ledger).unwrap(),
        text,
        "never written over"
    );

    let fixture = Fixture::new("spend-ledger-garbled");
    let ledger = fixture.write("spend-ledger.json", b"{\"format\":\"other/1\"}", 0o600);
    let answer = wake_once(&deployment(&fixture, &ledger), &runpod);
    assert!(answer.starts_with("HTTP/1.1 503 "), "{answer}");
    assert_eq!(runpod.create_calls(), 0);
}

#[test]
fn spend_ledger_a_ledger_file_without_a_spend_ceiling_is_refused() {
    let fixture = Fixture::new("spend-ledger-uncapped");
    let secret = fixture.owner_secret();
    let text = mutate(
        &document("127.0.0.1:0", &secret),
        "kind = \"runpod-vllm\"\n",
        "kind = \"runpod-vllm\"\nspend_ledger_file = \"ledger.json\"\n",
    );
    let refusal = load(&fixture.config(&text)).unwrap_err();
    assert_eq!(refusal.kind(), StartupRefusal::ConfigValue);
    assert!(
        refusal
            .message()
            .starts_with("providers.runpod.spend_ledger_file"),
        "{}",
        refusal.message()
    );
}
