//! The status data, `website/data/status.json` (`b10x-status/1`), and the status page, from
//! [`CAPABILITIES`].
//!
//! The repository keeps no status record of its own, so this list is the source. A shipped
//! capability names the test that holds it, as `path::function` from the repository root.
//! Generation fails when that file has no such function, so a claim cannot outlive the test it
//! rests on. A capability that is not shipped names none.

use std::{fmt::Write as _, fs, path::Path};

use serde_json::{Value, json};

use crate::{MDX_HEADER, Result, yaml_string};

/// Where the status data lands, relative to the repository root.
pub const DATA: &str = "website/data/status.json";
/// Where the status page lands, relative to the repository root.
pub const PAGE: &str = "website/docs/status.mdx";

/// The date the list below was last checked against the code.
const AS_OF: &str = "2026-10-08";

/// One capability on the status page.
pub struct Capability {
    pub area: &'static str,
    pub label: &'static str,
    pub detail: &'static str,
    /// `shipped`, `decided` or `planned`.
    pub status: &'static str,
    /// For a shipped capability, the test that holds it: `path::function`.
    pub evidence: Option<&'static str>,
    pub href: Option<&'static str>,
}

const fn shipped(
    area: &'static str,
    label: &'static str,
    detail: &'static str,
    evidence: &'static str,
    href: Option<&'static str>,
) -> Capability {
    Capability {
        area,
        label,
        detail,
        status: "shipped",
        evidence: Some(evidence),
        href,
    }
}

const fn planned(
    area: &'static str,
    label: &'static str,
    detail: &'static str,
    href: Option<&'static str>,
) -> Capability {
    Capability {
        area,
        label,
        detail,
        status: "planned",
        evidence: None,
        href,
    }
}

const GATEWAY: Option<&str> = Some("/docs/concepts/single-owner-gateway");
const RELAY: Option<&str> = Some("/docs/concepts/relay");
const HOSTING: Option<&str> = Some("/docs/concepts/hosting");
const DOCUMENT: Option<&str> = Some("/docs/reference/deployment-document");

/// Every capability the site claims, in the order the status table lists them.
pub const CAPABILITIES: &[Capability] = &[
    shipped(
        "Gateway",
        "One authenticated owner",
        "Every route but the probes and the two public routes takes the owner's bearer credential, compared in constant time; a rejected credential is never echoed back.",
        "crates/llm-gateway/tests/gateway.rs::a_rejected_or_malformed_credential_is_never_echoed_back",
        GATEWAY,
    ),
    shipped(
        "Gateway",
        "Liveness and readiness probes",
        "GET /health and GET /ready need no credential and are answered before load is shed.",
        "crates/llm-gateway/tests/gateway.rs::liveness_is_answered_before_load_is_shed",
        GATEWAY,
    ),
    shipped(
        "Gateway",
        "Read-only route inspection",
        "GET /v1/routes and GET /v1/routes/<alias> render the operator's identifiers and nothing else; a limit the source did not report stays absent.",
        "crates/llm-gateway/tests/gateway.rs::the_owner_inspects_routes_without_a_credential_or_a_secret_read",
        GATEWAY,
    ),
    shipped(
        "Gateway",
        "Public model listing",
        "GET /v1/models lists each model with its wires and max_model_len, without a credential and without waking a pod. On main after 0.5.0.",
        "crates/llm-gateway/tests/public_listing.rs::r5_the_listing_answers_without_a_credential_one_entry_per_model_with_its_wires",
        Some("/docs/guides/set-up-a-client"),
    ),
    shipped(
        "Gateway",
        "Setup instructions per client",
        "GET / answers the Codex, Claude Code or Loom settings for each model, chosen by User-Agent, and never the owner secret. On main after 0.5.0.",
        "crates/llm-gateway/tests/public_listing.rs::r1_the_rules_are_tried_in_order_codex_then_claude_then_a_browser",
        Some("/docs/guides/set-up-a-client"),
    ),
    shipped(
        "Gateway",
        "Published refusals and bounds",
        "Every refusal code, status and message, and every numeric bound, is checked against the published contract.",
        "crates/llm-gateway/tests/gateway.rs::the_published_refusal_table_matches_the_codes_the_crate_can_emit",
        Some("/docs/reference/refusals"),
    ),
    shipped(
        "Gateway",
        "Graceful drain and stop",
        "A drain reports unready while it keeps serving; a stop answers every accepted request before it returns.",
        "crates/llm-gateway/tests/gateway.rs::graceful_shutdown_drains_readiness_then_stops_accepting",
        GATEWAY,
    ),
    shipped(
        "Gateway",
        "No secret, upstream or provisioning in the gateway library",
        "The gateway crate's whole dependency closure is itself: nothing in it can resolve a secret, reach an upstream or create a resource.",
        "crates/llm-gateway/tests/dependency_boundary.rs::the_transitive_closure_contains_nothing_that_can_resolve_a_secret_or_provision",
        GATEWAY,
    ),
    shipped(
        "Relay",
        "Chat, Responses and Messages wires relayed by the library",
        "The owner's POST to /v1/chat/completions, /v1/responses or /v1/messages reaches the target the embedding hands out and streams back as it arrives; only the top-level model is rewritten.",
        "crates/llm-gateway/tests/wire_relay.rs::r6_chat_completions_are_relayed_to_the_chat_path_streaming_and_not",
        RELAY,
    ),
    shipped(
        "Relay",
        "A tool request to a model without tool calling is refused before a pod starts",
        "A non-empty top-level tools array for a model whose tool_calling is absent is tools-not-served, decided before any target is asked for.",
        "crates/llm-gateway/tests/wire_relay.rs::tools_a_tool_request_to_a_model_without_tool_calling_is_refused_without_asking_for_a_target",
        RELAY,
    ),
    shipped(
        "Relay",
        "A failed target is dropped and replaced",
        "A connection failure or a 502, 503 or 504 answers upstream-failed and drops that endpoint; the next request reaches a replacement.",
        "crates/llm-gateway/tests/wire_relay.rs::w7_an_upstream_502_503_or_504_drops_that_endpoint_and_the_next_request_reaches_a_replacement",
        RELAY,
    ),
    shipped(
        "Relay",
        "Cold-start hold in the Runpod composition",
        "A request for a model whose pod is starting waits up to request_hold_seconds, then is model-cold-start with retry-after 30. Tested through a seam: the shipped binary reaches no pod yet.",
        "crates/llm-gateway-cli/tests/relaying.rs::l6_a_request_for_a_pod_still_starting_is_held_until_the_pod_serves",
        RELAY,
    ),
    shipped(
        "Binary",
        "One closed TOML deployment document",
        "b10x-llm-gateway --config <file>: an unknown key at any level is refused as config:schema, naming its line and the keys allowed there.",
        "crates/llm-gateway-cli/tests/config.rs::k29_every_key_outside_the_closed_document_is_refused",
        DOCUMENT,
    ),
    shipped(
        "Binary",
        "Trusted-file reader",
        "The document, the owner secret and each vLLM key file are opened once, never through a symlink, and checked for owner, mode, size and UTF-8 on that one handle.",
        "crates/llm-gateway-cli/tests/binary.rs::k30_a_symlinked_owner_secret_is_refused",
        DOCUMENT,
    ),
    shipped(
        "Binary",
        "A vLLM key file per model",
        "A model may name vllm_api_key_file; the key is read once at startup and written nowhere.",
        "crates/llm-gateway-cli/tests/binary.rs::b9_a_model_naming_a_vllm_api_key_file_is_served_and_the_key_is_written_nowhere",
        DOCUMENT,
    ),
    shipped(
        "Binary",
        "Stop on SIGINT or SIGTERM",
        "The binary answers what it accepted, prints the accepted and completed counts, and exits 0.",
        "crates/llm-gateway-cli/tests/binary.rs::d2_sigterm_stops_the_server",
        Some("/docs/reference/cli"),
    ),
    shipped(
        "Binary",
        "Container image definition",
        "The root Dockerfile builds a distroless, non-root image of b10x-llm-gateway on port 8080. No image is published.",
        "crates/llm-gateway-cli/tests/dockerfile.rs::d1_the_final_stage_is_distroless",
        None,
    ),
    shipped(
        "Observability",
        "llmgw's 13 counters at /metrics",
        "Prometheus text behind the owner credential, under llmgw's series names, so an llmgw scrape keeps working.",
        "crates/llm-gateway/tests/observability.rs::r4_metrics_is_prometheus_text_carrying_llmgws_13_series_names_in_order",
        Some("/docs/guides/scrape-the-counters"),
    ),
    shipped(
        "Observability",
        "One usage record per authenticated wire request",
        "Relayed, refused or upstream-failed, each request makes one record; the binary writes it as one tracing event at info. Token counts are always absent.",
        "crates/llm-gateway/tests/observability.rs::o1_o2_a_relayed_answer_a_refusal_and_an_upstream_failure_each_make_one_record",
        Some("/docs/guides/scrape-the-counters"),
    ),
    shipped(
        "Hosting",
        "Hosting lifecycle contract",
        "Provider-assigned identity, one lease at a time, stop obligations that only a complete inventory discharges, and an in-process FakeProvider.",
        "crates/llm-provision/tests/hosting.rs::concurrent_acquisition_grants_exactly_one_lease",
        HOSTING,
    ),
    shipped(
        "Hosting",
        "Runpod vLLM pool over an emulator",
        "One pod per model under concurrent first requests, GPU fallback in declared order, crash and deadline termination, idle reaping and an orphan sweep, against EmulatedRunpod.",
        "crates/llm-runpod/tests/runpod.rs::concurrent_first_requests_create_exactly_one_pod",
        HOSTING,
    ),
    shipped(
        "Hosting",
        "Runpod transport through the connectors CLI",
        "ConnectorsRunpod creates, lists and terminates pods only through the connectors CLI, each write with an approval proof for its exact input. Tested against a fixture of that CLI, not against Runpod.",
        "crates/llm-runpod/tests/transport.rs::create_prepares_issues_and_invokes_pod_create_with_a_proof_for_its_exact_input",
        HOSTING,
    ),
    shipped(
        "Specification",
        "ESS specification and conformance suite",
        "The system llm-gateway is specified in ESS; the gate answers at least 201 of 201 scenarios, skips none, and fails on drift between the specification and its generated suite.",
        "checks/conformance/src/gate.rs::every_authored_scenario_is_declared",
        Some("/docs/guides/run-the-checks"),
    ),
    planned(
        "Relay",
        "Relay to a Runpod pod from the binary",
        "The binary refuses a document whose provider declares connectors until it reaches a pod's https proxy URL over verified TLS; until then a relayed model is target-unavailable.",
        RELAY,
    ),
    planned(
        "Relay",
        "Fallback to the next target",
        "A model with ordered targets tries the next one when an attempt fails before any answer byte reaches the client.",
        None,
    ),
    planned(
        "Relay",
        "Protocol translation",
        "Translating a supported subset between wires; the gateway translates nothing today.",
        None,
    ),
    planned(
        "Relay",
        "Hosted endpoints",
        "A model served by any server that speaks its wire at a declared base URL, not only by a Runpod pod.",
        None,
    ),
    planned(
        "Observability",
        "Token counts in usage records",
        "Each usage record carries the token counters and the model name the target reported; a count it did not report stays absent.",
        None,
    ),
    planned(
        "Clients",
        "Live sessions of Claude Code, Codex and Loom",
        "One recorded live session per client against a model on a real Runpod pod through the binary. Nothing is qualified yet.",
        Some("/docs/guides/set-up-a-client"),
    ),
    planned(
        "Hosting",
        "Modal adapter",
        "b10x-llm-modal exports nothing yet.",
        HOSTING,
    ),
];

/// The checked list, ready to render.
pub struct Record {
    items: Vec<Value>,
    shipped: usize,
    planned: usize,
}

/// Whether `source` defines a function called `name`.
fn defines(source: &str, name: &str) -> bool {
    source.contains(&format!("fn {name}("))
}

/// The status record for the repository at `root`.
pub(crate) fn record(root: &Path) -> Result<Record> {
    record_for(root, CAPABILITIES)
}

fn record_for(root: &Path, capabilities: &[Capability]) -> Result<Record> {
    let mut items = Vec::new();
    for capability in capabilities {
        let label = capability.label;
        if !matches!(capability.status, "shipped" | "decided" | "planned") {
            return Err(format!(
                "{label}: status {} is not shipped, decided or planned",
                capability.status
            ));
        }
        if items.iter().any(|item: &Value| item["label"] == label) {
            return Err(format!("{label}: listed twice"));
        }
        match (capability.status, capability.evidence) {
            ("shipped", Some(evidence)) => {
                let (file, function) = evidence
                    .split_once("::")
                    .ok_or_else(|| format!("{label}: {evidence} is not `path::function`"))?;
                let source = fs::read_to_string(root.join(file))
                    .map_err(|error| format!("{label}: reading {file}: {error}"))?;
                if !defines(&source, function) {
                    return Err(format!("{label}: {file} has no test {function}"));
                }
            }
            ("shipped", None) => return Err(format!("{label}: shipped without a test")),
            (_, Some(_)) => return Err(format!("{label}: only a shipped item names a test")),
            (_, None) => {}
        }
        let mut item = json!({
            "area": capability.area,
            "label": label,
            "detail": capability.detail,
            "status": capability.status,
        });
        if let Some(href) = capability.href {
            item["href"] = json!(href);
        }
        items.push(item);
    }
    let shipped = capabilities
        .iter()
        .filter(|capability| capability.status == "shipped")
        .count();
    Ok(Record {
        planned: capabilities.len() - shipped,
        shipped,
        items,
    })
}

/// `website/data/status.json`, a `b10x-status/1` document.
pub(crate) fn data(record: &Record) -> String {
    let document = json!({
        "format": "b10x-status/1",
        "asOf": AS_OF,
        "source": "llm-gateway-docs, from its capability list; every shipped item names the test that holds it",
        "items": record.items,
    });
    serde_json::to_string_pretty(&document).expect("a JSON value serializes") + "\n"
}

/// `website/docs/status.mdx`.
pub(crate) fn page(record: &Record) -> String {
    let (shipped, planned) = (record.shipped, record.planned);
    let mut page = format!(
        "---\ntitle: Status\nsidebar_position: 3\ndescription: {}\nlede: {}\nsource: {}\ncustom_edit_url: null\n---\n\n{MDX_HEADER}\n\n",
        yaml_string(
            "What llm-gateway ships on main and what is planned, capability by capability."
        ),
        yaml_string(&format!(
            "{shipped} capabilities ship; {planned} are planned. Nothing is deployed or qualified against a live client."
        )),
        yaml_string(
            "crates/llm-gateway-docs/src/status.rs, generated into website/data/status.json; each shipped item names its test"
        ),
    );
    page.push_str("import status from '@site/data/status.json';\n\n");
    let _ = write!(
        page,
        "**Shipped** means built on `main` and held by a named test in the repository gate. \
         **Planned** means on the plan and not built. The gate runs against in-process fakes, \
         loopback sockets, the Runpod emulator and a fixture of the `connectors` CLI, and makes \
         no paid call, so *shipped* is not *deployed*: the shipped binary relays no request to a \
         Runpod pod yet, and no client has been qualified against it. The \
         [release notes](https://github.com/beyond10x/llm-gateway/releases) say which release \
         brought each change.\n\n"
    );
    page.push_str("<StatusTable data={status} />\n");
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> &'static Path {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
    }

    fn capability(status: &'static str, evidence: Option<&'static str>) -> Capability {
        Capability {
            area: "A",
            label: "L",
            detail: "D",
            status,
            evidence,
            href: None,
        }
    }

    const REAL: &str = "crates/llm-gateway-cli/tests/binary.rs::d2_sigterm_stops_the_server";

    #[test]
    fn every_shipped_capability_rests_on_a_test_that_exists() {
        let record = record(root()).unwrap();
        assert!(record.shipped > 0 && record.planned > 0);
        assert!(data(&record).contains("\"format\": \"b10x-status/1\""));
    }

    #[test]
    fn a_vanished_test_or_a_shipped_item_without_one_is_refused() {
        let gone = capability(
            "shipped",
            Some("crates/llm-gateway-cli/tests/binary.rs::no_such_test"),
        );
        let error = record_for(root(), &[gone]).err().unwrap();
        assert!(error.contains("has no test no_such_test"), "{error}");
        assert!(record_for(root(), &[capability("shipped", None)]).is_err());
        assert!(record_for(root(), &[capability("planned", Some(REAL))]).is_err());
        assert!(record_for(root(), &[capability("released", None)]).is_err());
        assert!(
            record_for(
                root(),
                &[capability("planned", None), capability("planned", None)]
            )
            .is_err(),
            "a label listed twice"
        );
        assert!(record_for(root(), &[capability("planned", None)]).is_ok());
        assert!(record_for(root(), &[capability("shipped", Some(REAL))]).is_ok());
    }
}
