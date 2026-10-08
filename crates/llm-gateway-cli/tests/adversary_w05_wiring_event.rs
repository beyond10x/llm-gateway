//! Adversarial pass over `story:live-runpod-wiring`: the `connectors connection unreachable`
//! event. In its own test binary: `tracing` caches a callsite's interest, and another test that
//! reaches the same callsite on a thread without a subscriber can leave a scoped one unasked.

mod support;

use llm_gateway_cli::{ProxyConnector, load, start_connected};
use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};
use support::{Connectors, Fixture, OWNER_SECRET, VLLM_KEY, connected_document, listed};

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

/// The `warn` event `connectors connection unreachable` (spec/domains/deployment.yaml, row O3)
/// is written, names the provider, and carries no key; a reachable connection writes none.
/// `start_connected` emits it on the calling thread, so a scoped subscriber observes it.
#[test]
fn adversary_w05_the_unreachable_event_is_written_and_carries_no_secret() {
    for reachable in [false, true] {
        let fixture = Fixture::new(&format!("adversary-w05-event-{reachable}"));
        let connectors = Connectors::new(&fixture);
        if reachable {
            connectors.script(&serde_json::json!({
                "operations invoke pods.list": [listed(&serde_json::json!([]))],
            }));
        }
        let deployment =
            load(&fixture.config(&connected_document(&fixture, "127.0.0.1:0", &connectors, 5)))
                .unwrap();
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(llm_gateway_cli::log_filter(Some("trace")))
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            start_connected(&deployment, |transport| transport, Arc::new(ProxyConnector))
                .unwrap()
                .shutdown();
        });
        let text = captured.text();
        let events: Vec<&str> = text
            .lines()
            .filter(|line| line.contains("connectors connection unreachable"))
            .collect();
        if reachable {
            assert!(events.is_empty(), "{text}");
        } else {
            assert_eq!(events.len(), 1, "{text}");
            assert!(events[0].contains(" WARN "), "{text}");
            assert!(events[0].contains("provider=\"runpod\""), "{text}");
        }
        for secret in [VLLM_KEY, OWNER_SECRET] {
            assert!(!text.contains(secret), "{text}");
        }
    }
}
