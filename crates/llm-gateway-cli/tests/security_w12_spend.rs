//! Security review of story:spend-caps: the spend ceiling is a spending boundary in the
//! composition the shipped binary runs.
//!
//! `crate::start` composes `start_connected` over `ConnectorsRunpod`; `start_relaying` and
//! `runpod_pool` are a seam the binary never calls (`relaying.rs`). `docs/hosting.md` ("Spend
//! ceilings") and `spec/domains/deployment.yaml` (`SpendPolicy`: "that ceiling binds every pod
//! start of every model it runs") promise the check on every create. This drives the shipped
//! composition with the `connectors` fixture and a pod connector that opens no connection, so
//! nothing leaves the loopback interface.

mod support;

use llm_gateway::RelayStream;
use llm_gateway_cli::{PodConnector, load, start_connected};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};
use support::{Connectors, Fixture, OWNER_SECRET, connected_document, listed, mutate};

/// Opens no connection to any pod, probe or relay: no test here reaches past loopback.
struct NoPod;

impl PodConnector for NoPod {
    fn connect(&self, _authority: &str) -> io::Result<Box<dyn RelayStream>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no pod in this test",
        ))
    }
}

fn post_chat(address: SocketAddr) -> String {
    let body = r#"{"model":"small","messages":[{"role":"user","content":"hi"}]}"#;
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    write!(
        stream,
        "POST /v1/chat/completions HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut answer = String::new();
    drop(stream.read_to_string(&mut answer));
    answer
}

/// Invariants 1 and 4 through `start_connected`: with `daily_cap_usd = 1` declared and the
/// account's spend read answering 5 USD today, a request for the cold model sends no
/// `pod.create` (no approval either), reads `billing.pods`, and is answered `spend-ceiling`
/// with no amount in the body.
#[test]
fn sec_the_shipped_composition_checks_the_ceiling_before_any_create() {
    let fixture = Fixture::new("sec-w12-spend-shipped");
    let connectors = Connectors::new(&fixture);
    connectors.script(&serde_json::json!({
        "operations invoke pods.list": [listed(&serde_json::json!([]))],
        "operations invoke billing.pods": [{"stdout": {
            "adapter": "gpu", "operation": "billing.pods", "revision": "revision-7",
            "result": {"status": 200, "body": [
                {"amount": 5.25, "time": "2999-01-01T00:00:00.000Z"},
            ]},
        }}],
    }));
    let text = connected_document(&fixture, "127.0.0.1:0", &connectors, 2);
    let text = mutate(
        &text,
        "kind = \"runpod-vllm\"\n",
        "kind = \"runpod-vllm\"\ndaily_cap_usd = 1\n",
    );
    let text = mutate(
        &text,
        "max_model_len = 1024\n",
        "max_model_len = 1024\nprice_per_hour_usd = 2\n",
    );
    let deployment = load(&fixture.config(&text)).unwrap();
    let relaying = start_connected(&deployment, |transport| transport, Arc::new(NoPod)).unwrap();
    let answer = post_chat(relaying.local_addr());
    relaying.shutdown();

    let creates = connectors.calls_of("operations invoke pod.create");
    let approvals = connectors.calls_of("approvals prepare pod.create");
    assert!(
        creates.is_empty() && approvals.is_empty(),
        "a ceiling is declared and the account is over it, yet the shipped composition sent \
         {} pod.create invoke(s) and {} approval(s); billing.pods was read {} time(s); \
         answer: {answer:?}",
        creates.len(),
        approvals.len(),
        connectors.calls_of("operations invoke billing.pods").len()
    );
    assert!(
        !connectors
            .calls_of("operations invoke billing.pods")
            .is_empty(),
        "the spend was never read"
    );
    assert!(answer.starts_with("HTTP/1.1 503 "), "{answer:?}");
    assert!(answer.contains("spend-ceiling"), "{answer:?}");
    for amount in ["5.25", "4.25", "1.000000", "USD 1", "daily"] {
        assert!(
            !answer.contains(amount),
            "the client's refusal names {amount:?}: {answer:?}"
        );
    }
}
