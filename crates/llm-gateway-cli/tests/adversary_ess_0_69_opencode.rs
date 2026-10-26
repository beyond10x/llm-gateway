//! Adversary, story:ess-0-69: llm 0.15.0's messages client sends `x-opencode-session` to an
//! `OpenCode` Go base URL. A routes document naming such an endpoint loads, and a chat or responses
//! client's request to it is translated with that client, so `docs/gateway.md` ("The upstream")
//! names that header among llm's protocol headers.

use llm_gateway_cli::{Routes, Wire};
use llm_providers::BaseUrl;

const CONTRACT: &str = include_str!("../../../docs/gateway.md");

/// The base URL `Prepared::reach` builds for the endpoint below: scheme, authority, base path.
const GO_BASE_URL: &str = "https://opencode.ai/inference/go/v1";

const GO_ROUTES: &str = "format: llm.routes/1\nendpoints:\n  go: {base_url: https://opencode.ai/inference/go/v1, protocol: messages, auth: {kind: bearer, secret: \"file:/run/secrets/go-key\"}}\nroutes:\n  glm: go/glm-5\n";

/// The text of `docs/gateway.md` from `### The upstream` to the next `### ` heading.
fn upstream_section() -> &'static str {
    let start = CONTRACT
        .find("### The upstream")
        .expect("docs/gateway.md has a section The upstream");
    let rest = &CONTRACT[start + 1..];
    let end = rest.find("\n### ").map_or(rest.len(), |end| end + 1);
    &CONTRACT[start..start + end]
}

#[test]
fn an_opencode_go_endpoint_loads_and_the_contract_names_the_session_header_llm_sends_it() {
    let routes = Routes::parse(GO_ROUTES).expect("the gateway serves an OpenCode Go endpoint");
    let endpoint = routes.endpoints().next().expect("one endpoint");
    assert_eq!(endpoint.wire(), Wire::Messages);
    let base_url = BaseUrl::new(GO_BASE_URL).expect("a base URL llm takes");
    assert!(
        llm_messages::is_opencode_go(&base_url),
        "llm 0.15.0 treats this endpoint as OpenCode Go and sends {} on every request",
        llm_messages::OPENCODE_SESSION_HEADER
    );
    let section = upstream_section();
    assert!(
        section.contains(llm_messages::OPENCODE_SESSION_HEADER),
        "docs/gateway.md \"The upstream\" lists llm's protocol headers on a translated request \
         and does not name `{}`, which llm 0.15.0's messages client sends to an OpenCode Go \
         endpoint (a session minted per client, and the gateway builds one client per request)",
        llm_messages::OPENCODE_SESSION_HEADER
    );
}
