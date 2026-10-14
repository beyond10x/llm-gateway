//! Adversarial pass over `story:live-runpod-wiring`: `start_connected` decides a document's
//! validity without depending on the connection. (The pass also held the shipped `start`'s
//! refusal of every spelling of a `connectors` table; story:pod-proxy-tls removed that
//! refusal.) Nothing leaves the loopback interface and no `connectors` but the fixture runs.
//! The unreachable event is in `adversary_w05_wiring_event.rs`.

mod support;

use llm_gateway_cli::{ProxyConnector, load, start_connected};
use std::sync::Arc;
use support::{Connectors, Fixture, connected_document, listed, mutate};

/// A connected document whose model the pool cannot run: `image` holds a space, which `load`
/// accepts (printable ASCII) and the pool refuses (a hosting identifier is graphic ASCII).
fn unrunnable(fixture: &Fixture, connectors: &Connectors) -> String {
    mutate(
        &connected_document(fixture, "127.0.0.1:0", connectors, 5),
        "image = \"vllm/vllm-openai:v0.27.1\"",
        "image = \"vllm/vllm-openai v0.27.1\"",
    )
}

/// `relaying.rs` says of `start_connected`: "Checked before the connection is: whether a
/// document is valid never depends on it." A model the pool cannot run is only found when the
/// pool is built, which is after the reachability `pods.list`. With the connection up the
/// document is refused, but only after a `connectors` call.
#[test]
fn adversary_w05_a_document_the_pool_refuses_is_refused_before_any_connectors_call() {
    let fixture = Fixture::new("adversary-w05-unrunnable-up");
    let connectors = Connectors::new(&fixture);
    connectors.script(&serde_json::json!({
        "operations invoke pods.list": [listed(&serde_json::json!([]))],
    }));
    let deployment = load(&fixture.config(&unrunnable(&fixture, &connectors))).unwrap();
    match start_connected(&deployment, |transport| transport, Arc::new(ProxyConnector::shipped())) {
        Ok(relaying) => {
            relaying.shutdown();
            panic!("a model the pool cannot run was served");
        }
        Err(refusal) => assert_eq!(refusal.code(), "config:value", "{refusal}"),
    }
    assert!(
        connectors.calls().is_empty(),
        "the document was refused only after the fixture received {:?}",
        connectors.calls()
    );
}

/// The same document with the connection down: it must be refused just the same, because its
/// validity does not depend on the connection. It is served instead.
#[test]
fn adversary_w05_a_document_the_pool_refuses_is_refused_with_the_connection_down() {
    let fixture = Fixture::new("adversary-w05-unrunnable-down");
    let connectors = Connectors::new(&fixture);
    // No script: the reachability `pods.list` fails.
    let deployment = load(&fixture.config(&unrunnable(&fixture, &connectors))).unwrap();
    match start_connected(&deployment, |transport| transport, Arc::new(ProxyConnector::shipped())) {
        Ok(relaying) => {
            relaying.shutdown();
            panic!(
                "a model the pool cannot run was accepted because the connection was down; \
                 the fixture received {:?}",
                connectors.calls()
            );
        }
        Err(refusal) => assert_eq!(refusal.code(), "config:value", "{refusal}"),
    }
}
