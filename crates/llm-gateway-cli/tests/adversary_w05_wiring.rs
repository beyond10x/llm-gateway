//! Adversarial pass over `story:live-runpod-wiring`: the shipped `start` refuses every spelling
//! of a `connectors` table before any call, and `start_connected` decides a document's validity
//! without depending on the connection. Nothing leaves the loopback interface and no
//! `connectors` but the fixture runs. The unreachable event is in `adversary_w05_wiring_event.rs`.

mod support;

use llm_gateway_cli::{ProxyConnector, load, start, start_connected};
use std::sync::Arc;
use support::{Connectors, Fixture, connected_document, listed, mutate};

/// The `[providers.runpod.connectors]` block `connected_document` wrote, as it appears.
fn table_block(connectors: &Connectors) -> String {
    format!("\n[providers.runpod.connectors]\n{}", connectors.table())
}

/// Every TOML spelling of the same `connectors` table, and two that are not one. Whatever
/// `load` makes of it, the shipped `start` must not run a single `connectors` call.
#[test]
fn adversary_w05_every_spelling_of_connectors_is_refused_before_any_call() {
    let fixture = Fixture::new("adversary-w05-spellings");
    let connectors = Connectors::new(&fixture);
    connectors.script(&serde_json::json!({
        "operations invoke pods.list": [listed(&serde_json::json!([]))],
    }));
    let base = connected_document(&fixture, "127.0.0.1:0", &connectors, 5);
    let block = table_block(&connectors);
    let exe = connectors.executable().display().to_string();
    let work = connectors.work().display().to_string();
    let inline = format!(
        "connectors = {{ executable = \"{exe}\", adapter = \"gpu\", connection = \"conn-1\", work_directory = \"{work}\" }}\n"
    );
    let dotted = format!(
        "connectors.executable = \"{exe}\"\nconnectors.adapter = \"gpu\"\nconnectors.connection = \"conn-1\"\nconnectors.work_directory = \"{work}\"\n"
    );
    let variants: Vec<(&str, String)> = vec![
        ("table", base.clone()),
        ("inline table", mutate(&base, &block, &inline)),
        ("dotted keys", mutate(&base, &block, &dotted)),
        (
            "quoted key",
            mutate(
                &base,
                "[providers.runpod.connectors]",
                "[providers.runpod.\"connectors\"]",
            ),
        ),
        (
            "escaped key",
            mutate(
                &base,
                "[providers.runpod.connectors]",
                "[providers.runpod.\"conn\\u0065ctors\"]",
            ),
        ),
        (
            "array of tables",
            mutate(
                &base,
                "[providers.runpod.connectors]",
                "[[providers.runpod.connectors]]",
            ),
        ),
        (
            "capitalised",
            mutate(
                &base,
                "[providers.runpod.connectors]",
                "[providers.runpod.Connectors]",
            ),
        ),
        (
            "on a second provider without models",
            mutate(
                &mutate(
                    &base,
                    "[providers.runpod.connectors]",
                    "[providers.zeta.connectors]",
                ),
                "\n[providers.zeta.connectors]\n",
                "\n[providers.zeta]\nkind = \"runpod-vllm\"\n\n[providers.zeta.connectors]\n",
            ),
        ),
    ];
    for (name, text) in variants {
        match load(&fixture.config(&text)) {
            Err(refusal) => assert!(
                matches!(refusal.code(), "config:schema" | "config:value"),
                "{name}: {refusal}"
            ),
            Ok(deployment) => {
                assert!(
                    deployment
                        .providers
                        .values()
                        .any(|provider| provider.connectors.is_some()),
                    "{name}: loaded without its connectors table"
                );
                match start(&deployment) {
                    Ok(running) => {
                        running.shutdown();
                        panic!("{name}: the shipped start served a connectors document");
                    }
                    Err(refusal) => assert_eq!(refusal.code(), "config:value", "{name}: {refusal}"),
                }
            }
        }
        assert!(
            connectors.calls().is_empty(),
            "{name}: the fixture received {:?}",
            connectors.calls()
        );
    }
}

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
    match start_connected(&deployment, |transport| transport, Arc::new(ProxyConnector)) {
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
    match start_connected(&deployment, |transport| transport, Arc::new(ProxyConnector)) {
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
