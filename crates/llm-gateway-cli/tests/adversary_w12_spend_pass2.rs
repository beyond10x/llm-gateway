//! Adversarial pass 2 over story:spend-caps: whose ceiling the shipped composition binds its
//! pool with. Nothing leaves the loopback interface and no `connectors` but the fixture runs.

mod support;

use llm_gateway::RelayStream;
use llm_gateway_cli::{PodConnector, load, start_connected};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};
use support::{
    Connectors, Fixture, OWNER_SECRET, connected_document, created, listed, mutate,
    running_small_pod,
};

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

/// The owner's wake of `small` (`POST /v1/models/small/wake`); the whole answer.
fn post_wake(address: SocketAddr) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    write!(
        stream,
        "POST /v1/models/small/wake HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-length: 0\r\n\r\n"
    )
    .unwrap();
    let mut answer = String::new();
    drop(stream.read_to_string(&mut answer));
    answer
}

/// `daily_cap_usd` is "the account's spend ceiling" of the `[providers.<name>]` that declares it
/// (`spec/domains/deployment.yaml`, `ProviderDeclaration`; story acceptance 1: absent is no
/// ceiling). Here the connected provider `runpod`, whose account the pool runs, declares none,
/// and a second provider `reserve`, never connected and running nothing, declares 1 USD for
/// its own account. Were `reserve`'s ceiling checked against `runpod`'s billing (5.25 USD
/// today), the start would be refused `spend-ceiling`. Either fix satisfies this case: the
/// composition ignores a ceiling the pool's provider does not declare, or the document is
/// refused `config:value`.
///
/// The start is the owner's wake: since story:a-call-starts-no-pod a model call starts no pod
/// unless the model sets `start_on_request`, which needs a daily cap on its own provider, and
/// the document admits a ceiling on one provider only. A call here is `model-stopped` and would
/// reach no spend check at all.
#[test]
fn adversary_w12p2_another_providers_ceiling_does_not_bind_the_connected_account() {
    let fixture = Fixture::new("adversary-w12p2-foreign-ceiling");
    let connectors = Connectors::new(&fixture);
    connectors.script(&serde_json::json!({
        "invoke pods.list": [listed(&serde_json::json!([running_small_pod()]))],
        "invoke pods.list machine": [listed(&serde_json::json!([running_small_pod()]))],
        "invoke pod.create": [created(&running_small_pod())],
        "invoke billing.pods": [{"stdout": {
            "connector": "runpod", "instance": "gpu", "operation": "billing.pods", "revision": "revision-7",
            "result": {"status": 200, "body": [
                {"amount": 5.25, "time": "2999-01-01T00:00:00.000Z"},
            ]},
        }}],
    }));
    let text = connected_document(&fixture, "127.0.0.1:0", &connectors, 1);
    let text = mutate(
        &text,
        "\n[models.small]\n",
        "\n[providers.reserve]\nkind = \"runpod-vllm\"\ndaily_cap_usd = 1\n\n[models.small]\n",
    );
    let text = mutate(
        &text,
        "max_model_len = 1024\n",
        "max_model_len = 1024\nprice_per_hour_usd = 2\n",
    );
    let deployment = match load(&fixture.config(&text)) {
        Ok(deployment) => deployment,
        Err(refusal) => {
            assert_eq!(refusal.code(), "config:value", "{refusal}");
            return;
        }
    };
    let relaying = start_connected(&deployment, |transport| transport, Arc::new(NoPod)).unwrap();
    let answer = post_wake(relaying.local_addr());
    relaying.shutdown();
    let reads = connectors.calls_of("invoke billing.pods");
    assert!(
        reads.is_empty() && !answer.contains("spend-ceiling"),
        "the connected provider declares no ceiling, yet its account's spend was read {} \
         time(s) against another provider's 1 USD ceiling: {answer:?}",
        reads.len()
    );
    // The wake reached the start the ceiling would have refused.
    assert!(answer.starts_with("HTTP/1.1 202 "), "{answer:?}");
    assert_eq!(connectors.calls_of("invoke pod.create").len(), 1);
}
