//! Adversarial pass over story:spend-caps: the spend ceiling in the composition the shipped
//! binary runs. Nothing leaves the loopback interface and no `connectors` but the fixture runs.

mod support;

use llm_gateway_cli::{ProxyConnector, load, start_connected};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};
use support::{Connectors, Fixture, OWNER_SECRET, connected_document, listed, mutate};

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

/// `serve.rs` `start_with` composes `start_connected` with the identity `wrap`: this is the
/// shipped binary's path. The provider declares a total ceiling of 10 USD and the account has
/// billed 100 USD, so acceptance 2 refuses the start with `spend-ceiling` and sends no create.
/// `start_connected` builds its `RunpodPool` without `with_spend`, so no spend is ever read and
/// the create is sent.
#[test]
fn adversary_w12_the_shipped_composition_refuses_a_start_past_the_ceiling() {
    let fixture = Fixture::new("adversary-w12-shipped-ceiling");
    let connectors = Connectors::new(&fixture);
    connectors.script(&serde_json::json!({
        "invoke pods.list": [listed(&serde_json::json!([]))],
        "invoke billing.pods": [{"stdout": {
            "connector": "runpod", "instance": "gpu", "operation": "billing.pods", "revision": "revision-7",
            "result": {"status": 200, "body": [
                {"amount": 100.0, "time": "2026-01-01T00:00:00.000Z"},
            ]},
        }}],
    }));
    let text = connected_document(&fixture, "127.0.0.1:0", &connectors, 1);
    let text = mutate(
        &text,
        "kind = \"runpod-vllm\"\n",
        "kind = \"runpod-vllm\"\ntotal_cap_usd = 10\n",
    );
    let text = mutate(
        &text,
        "max_model_len = 1024\n",
        "max_model_len = 1024\nprice_per_hour_usd = 2\n",
    );
    let deployment = load(&fixture.config(&text)).unwrap();
    let relaying = start_connected(
        &deployment,
        |transport| transport,
        Arc::new(ProxyConnector::shipped()),
    )
    .unwrap();
    let answer = post_chat(relaying.local_addr());
    relaying.shutdown();
    let creates = connectors.calls_of("invoke pod.create");
    let reads = connectors.calls_of("invoke billing.pods");
    assert!(
        creates.is_empty(),
        "a create was sent past a 10 USD ceiling on 100 USD billed ({} spend reads): {answer}",
        reads.len()
    );
    assert!(answer.starts_with("HTTP/1.1 503 "), "{answer}");
    assert!(answer.contains("\"code\":\"spend-ceiling\""), "{answer}");
    assert_eq!(reads.len(), 1);
}
