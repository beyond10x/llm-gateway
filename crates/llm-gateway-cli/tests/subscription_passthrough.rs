//! story:subscription-passthrough in the deployment document: `auth_kind = "client"` is a hosted
//! route to `https://api.anthropic.com` alone, on the messages wire alone, behind a gateway that
//! authenticates nobody, with no credential of its own; `model_prefix` serves every name with it.

use llm_gateway::{AuthKind, IngressAuth};
use llm_gateway_cli::parse;

fn document(ingress: &str, entry: &str) -> String {
    format!(
        "listen = \"127.0.0.1:8090\"\n{ingress}\n\n[hosted.claude]\n{entry}\n",
        ingress = if ingress == "none" {
            "ingress_auth = \"none\"".to_owned()
        } else {
            "owner_secret_file = \"owner-secret\"".to_owned()
        }
    )
}

const CLIENT: &str = "wires = [\"messages\"]\nbase_url = \"https://api.anthropic.com\"\n\
                      auth_kind = \"client\"\nbilling_kind = \"subscription\"\n\
                      tool_calling = \"parsed\"\nmodel_prefix = \"claude-\"";

#[test]
fn a_client_route_to_anthropic_behind_an_open_gateway_is_accepted() {
    let deployment = parse(&document("none", CLIENT)).unwrap();
    assert_eq!(deployment.gateway.ingress_auth, IngressAuth::None);
    let claude = &deployment.hosted["claude"];
    assert_eq!(claude.auth_kind, AuthKind::Client);
    assert_eq!(claude.credential_file, None);
    assert_eq!(claude.model_prefix.as_deref(), Some("claude-"));
}

#[test]
fn each_broken_rule_is_a_value_refusal_naming_its_key() {
    let cases = [
        (
            "none",
            CLIENT.replace("https://api.anthropic.com", "https://example.com"),
            "hosted.claude.base_url",
        ),
        (
            "none",
            CLIENT.replace("[\"messages\"]", "[\"messages\", \"chat\"]"),
            "hosted.claude.wires",
        ),
        (
            "none",
            format!("{CLIENT}\ncredential_file = \"key\""),
            "hosted.claude.credential_file",
        ),
        (
            "none",
            format!("{CLIENT}\nreasoning_effort = \"low\""),
            "hosted.claude.reasoning_effort",
        ),
        (
            "none",
            format!("{CLIENT}\n[hosted.claude.headers]\nx-api-key = \"k\""),
            "hosted.claude.headers",
        ),
        ("bearer", CLIENT.to_owned(), "hosted.claude.auth_kind"),
        (
            "none",
            CLIENT.replace("model_prefix = \"claude-\"", "model_prefix = \"\""),
            "hosted.claude.model_prefix",
        ),
    ];
    for (ingress, entry, key) in cases {
        let refusal = parse(&document(ingress, &entry)).unwrap_err();
        assert_eq!(refusal.code(), "config:value", "{key}: {refusal}");
        assert!(refusal.message().contains(key), "{key}: {refusal}");
    }
}

#[test]
fn a_prefix_on_another_route_is_accepted_and_a_bearer_route_still_needs_its_key() {
    let entry = "wires = [\"chat\"]\nbase_url = \"https://example.com/v1\"\n\
                 auth_kind = \"bearer\"\nmodel_prefix = \"vendor/\"";
    let refusal = parse(&document("none", entry)).unwrap_err();
    assert!(
        refusal.message().contains("hosted.claude.credential_file"),
        "{refusal}"
    );
    let deployment = parse(&document(
        "none",
        &format!("{entry}\ncredential_file = \"key\""),
    ))
    .unwrap();
    assert_eq!(
        deployment.hosted["claude"].model_prefix.as_deref(),
        Some("vendor/")
    );
}

#[test]
fn a_client_route_takes_a_newest_family_an_output_cap_beta_extras_and_an_anthropic_header() {
    let entry = format!(
        "{CLIENT}\nnewest = \"opus\"\nmax_output_tokens = 32000\n\
         anthropic_beta = [\"context-1m-2025-08-07\"]\n\
         [hosted.claude.headers]\nanthropic-version = \"2023-06-01\""
    );
    let deployment = parse(&document("none", &entry)).unwrap();
    let claude = &deployment.hosted["claude"];
    assert_eq!(claude.newest.as_deref(), Some("opus"));
    assert_eq!(claude.max_tokens_cap, Some(32_000));
    assert_eq!(claude.anthropic_beta, ["context-1m-2025-08-07"]);
    assert_eq!(claude.headers.len(), 1);
}

#[test]
fn newest_and_beta_extras_belong_to_a_client_route_and_are_checked() {
    let bearer = "wires = [\"messages\"]\nbase_url = \"https://example.com/v1\"\n\
                  auth_kind = \"bearer\"\ncredential_file = \"key\"";
    let cases = [
        (
            format!("{bearer}\nnewest = \"opus\""),
            "hosted.claude.newest",
        ),
        (
            format!("{bearer}\nanthropic_beta = [\"b1\"]"),
            "hosted.claude.anthropic_beta",
        ),
        (
            format!("{CLIENT}\nnewest = \"Opus\""),
            "hosted.claude.newest",
        ),
        (format!("{CLIENT}\nnewest = \"\""), "hosted.claude.newest"),
        (
            format!("{CLIENT}\nanthropic_beta = [\"a b\"]"),
            "hosted.claude.anthropic_beta",
        ),
        (
            format!("{CLIENT}\nanthropic_beta = [\"b1\", \"b1\"]"),
            "hosted.claude.anthropic_beta",
        ),
        (
            format!("{CLIENT}\nmax_output_tokens = 0"),
            "hosted.claude.max_output_tokens",
        ),
    ];
    for (entry, key) in cases {
        let refusal = parse(&document("none", &entry)).unwrap_err();
        assert_eq!(refusal.code(), "config:value", "{key}: {refusal}");
        assert!(refusal.message().contains(key), "{key}: {refusal}");
    }
    let capped = parse(&document(
        "none",
        &format!("{bearer}\nmax_output_tokens = 4096"),
    ))
    .unwrap();
    assert_eq!(capped.hosted["claude"].max_tokens_cap, Some(4096));
}
