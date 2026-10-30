//! Adversarial pass over story:model-wake-and-status: the lifecycle routes in the composition
//! the shipped binary runs (`start_connected` with the identity `wrap`). Nothing leaves the
//! loopback interface and no `connectors` but the fixture runs.

mod support;

use llm_gateway_cli::{ProxyConnector, load, start_connected};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};
use support::{Connectors, Fixture, OWNER_SECRET, connected_document, listed};

fn send(address: SocketAddr, method: &str, path: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    let length = if body.is_empty() {
        String::new()
    } else {
        format!(
            "content-type: application/json\r\ncontent-length: {}\r\n",
            body.len()
        )
    };
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\n{length}\r\n{body}"
    )
    .unwrap();
    let mut answer = String::new();
    drop(stream.read_to_string(&mut answer));
    answer
}

fn body(answer: &str) -> &str {
    answer.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

/// docs/gateway.md, docs/hosting.md and spec/domains/gateway.yaml all say: "A model of a
/// provider whose `connectors` connection is absent or unreachable is `target-unavailable` to a
/// wake and `stopped` to a status." The config admits two providers when only one declares a
/// connection; `start_connected` pools only that provider's models, and `PoolTargets::wake` /
/// `status` answer `None` (`self.models.get(alias)?`) for every other Runpod model, so the
/// gateway answers `404 path-unknown`, the code the contract keeps for a model with no
/// lifecycle (a hosted endpoint, a route). A request for the same model is
/// `target-unavailable`, as documented, so the two disagree about one model.
#[test]
fn adversary_w12_a_model_of_a_provider_with_no_connection_is_unavailable_to_a_wake_and_stopped() {
    let fixture = Fixture::new("adversary-w12-wake-unconnected-provider");
    let connectors = Connectors::new(&fixture);
    connectors.script(&serde_json::json!({
        "invoke pods.list": [listed(&serde_json::json!([]))],
    }));
    let text = connected_document(&fixture, "127.0.0.1:0", &connectors, 1);
    let text = format!(
        "{text}\n[providers.spare]\nkind = \"runpod-vllm\"\n\n[models.other]\nprovider = \"spare\"\nwires = [\"chat\"]\ncontext_window = 65536\nhf_model = \"example/other-model\"\nimage = \"vllm/vllm-openai:v0.27.1\"\ngpu_types = [\"NVIDIA L40S\"]\nmax_model_len = 1024\n"
    );
    let deployment = load(&fixture.config(&text)).unwrap();
    let relaying = start_connected(
        &deployment,
        |transport| transport,
        Arc::new(ProxyConnector::shipped()),
    )
    .unwrap();
    let address = relaying.local_addr();
    let connected = send(address, "GET", "/v1/models/small/status", "");
    let request = send(
        address,
        "POST",
        "/v1/chat/completions",
        r#"{"model":"other","messages":[{"role":"user","content":"hi"}]}"#,
    );
    let wake = send(address, "POST", "/v1/models/other/wake", "");
    let status = send(address, "GET", "/v1/models/other/status", "");
    relaying.shutdown();
    // Controls: the connected model is pooled, and a request for the other one is the
    // documented `target-unavailable`.
    assert!(connected.starts_with("HTTP/1.1 200 "), "{connected}");
    assert!(request.starts_with("HTTP/1.1 503 "), "{request}");
    assert!(
        request.contains("\"code\":\"target-unavailable\""),
        "{request}"
    );
    assert!(connectors.calls_of("invoke pod.create").is_empty());
    // The contract for a model whose provider has no connection.
    assert!(
        wake.starts_with("HTTP/1.1 503 ") && wake.contains("\"code\":\"target-unavailable\""),
        "a wake of a model whose provider has no connection: {wake}"
    );
    assert!(
        status.starts_with("HTTP/1.1 200 "),
        "a status of a model whose provider has no connection: {status}"
    );
    assert_eq!(body(&status), "{\"model\":\"other\",\"phase\":\"stopped\"}");
}
