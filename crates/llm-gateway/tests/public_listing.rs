//! Acceptance for `story:public-model-listing`, over a real loopback socket: `GET /v1/models`
//! (row R5) and `GET /` (row R1) answer without a credential, ask the target source for
//! nothing, and render no owner-only fact. Each case is named after the matrix row it holds.

use llm_gateway::{
    AuthKind, BillingKind, Gateway, GatewayConfig, GatewayHandle, Label, OwnerToken, Relay,
    RelayModel, RelayStream, RelayTarget, RelayTargets, RouteInventory, RouteSummary,
    SharedSecretVerifier, TargetBearer, TargetLimits, TargetProvenance, TargetRefusal,
    TargetSummary, ToolCalling, Wire,
};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

const OWNER_SECRET: &str = "owner-token-public-listing-0123456789abcdef";
const SPEC: &str = include_str!("../../../spec/domains/gateway.yaml");
const CONTRACT: &str = include_str!("../../../docs/gateway.md");
const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
/// Every owner-only identifier the fixture composes: provenance labels, the target's authority,
/// the upstream model names and the target bearer. None may reach a public answer.
const OWNER_ONLY: &[&str] = &[
    "route-id-private",
    "target-id-private",
    "protocol-label-private",
    "provider-label-private",
    "account-label-private",
    "endpoint-label-private",
    "model-label-private",
    "binding-revision-private",
    "pod-authority-private.invalid:8000",
    "upstream-name-private",
    "vllm-bearer-private-0123456789",
    DIGEST,
    "config_digest",
    OWNER_SECRET,
];
const CODEX: &str = "codex-cli/1.0";
const CLAUDE: &str = "claude-cli/2.0";
const BROWSER: &str = "Mozilla/5.0";
const CURL: &str = "curl/8.0";

fn label(value: &str) -> Label {
    Label::new(value).unwrap()
}

/// A target source that counts every acquisition and hands out a target that carries every
/// owner-only fact the relay knows: its authority and its bearer.
struct CountingTargets {
    acquired: Arc<AtomicUsize>,
}

struct PrivateTarget {
    bearer: TargetBearer,
}

impl RelayTarget for PrivateTarget {
    fn authority(&self) -> &'static str {
        "pod-authority-private.invalid:8000"
    }

    fn bearer(&self) -> Option<&TargetBearer> {
        Some(&self.bearer)
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        Err(io::ErrorKind::ConnectionRefused.into())
    }
}

impl RelayTargets for CountingTargets {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        self.acquired.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(PrivateTarget {
            bearer: TargetBearer::new(b"vllm-bearer-private-0123456789".to_vec()).unwrap(),
        }))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

/// One route whose every provenance label is a distinct owner-only identifier.
fn private_inventory() -> RouteInventory {
    let target = TargetSummary {
        target_id: label("target-id-private"),
        position: 0,
        provenance: TargetProvenance {
            protocol: label("protocol-label-private"),
            provider: label("provider-label-private"),
            account: label("account-label-private"),
            endpoint: label("endpoint-label-private"),
            model: label("model-label-private"),
            binding_revision: label("binding-revision-private"),
        },
        auth_kind: AuthKind::Bearer,
        billing_kind: BillingKind::SelfHosted,
        limits: TargetLimits {
            context_window: Some(65_536),
            max_output_tokens: None,
        },
    };
    let route = RouteSummary::new(label("route-id-private"), label("code"), false, vec![target])
        .unwrap();
    RouteInventory::new(vec![route])
        .unwrap()
        .with_config_digest(DIGEST)
        .unwrap()
}

/// What a relayed model declares.
struct Declared {
    alias: &'static str,
    wires: Vec<Wire>,
    tool_calling: ToolCalling,
    max_model_len: Option<u64>,
    context_window: Option<u64>,
}

fn declared(alias: &'static str, wires: Vec<Wire>, context_window: u64) -> Declared {
    Declared {
        alias,
        wires,
        tool_calling: ToolCalling::Parsed,
        max_model_len: Some(context_window / 2),
        context_window: Some(context_window),
    }
}

/// Two models on every wire with tool calling parsed: `code` (65536) and `small` (32768).
fn two_models() -> Vec<Declared> {
    vec![
        declared("code", Wire::ALL.to_vec(), 65_536),
        declared("small", Wire::ALL.to_vec(), 32_768),
    ]
}

fn relay(models: Vec<Declared>, acquired: &Arc<AtomicUsize>) -> Relay {
    let models = models
        .into_iter()
        .map(|model| {
            let mut relayed =
                RelayModel::new(label(model.alias), label("upstream-name-private"), model.wires)
                    .unwrap()
                    .with_tool_calling(model.tool_calling);
            if let Some(length) = model.max_model_len {
                relayed = relayed.with_max_model_len(length);
            }
            if let Some(window) = model.context_window {
                relayed = relayed.with_context_window(window);
            }
            relayed
        })
        .collect();
    Relay::new(
        models,
        Arc::new(CountingTargets {
            acquired: Arc::clone(acquired),
        }),
    )
    .unwrap()
}

struct Fixture {
    handle: Option<GatewayHandle>,
    addr: SocketAddr,
    acquired: Arc<AtomicUsize>,
}

impl Fixture {
    fn start(models: Vec<Declared>) -> Self {
        Self::composed(Some(models), true)
    }

    fn composed(models: Option<Vec<Declared>>, ready: bool) -> Self {
        let acquired = Arc::new(AtomicUsize::new(0));
        let verifier = Arc::new(
            SharedSecretVerifier::new(OwnerToken::new(OWNER_SECRET.as_bytes().to_vec()).unwrap())
                .unwrap(),
        );
        let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
        config.read_timeout = Duration::from_secs(5);
        let handle = match models {
            Some(models) => Gateway::bind_with_relay(
                config,
                verifier,
                private_inventory(),
                relay(models, &acquired),
            ),
            None => Gateway::bind(config, verifier, private_inventory()),
        }
        .unwrap();
        if ready {
            handle.mark_ready();
        }
        let addr = handle.local_addr();
        Self {
            handle: Some(handle),
            addr,
            acquired,
        }
    }

    fn acquired(&self) -> usize {
        self.acquired.load(Ordering::SeqCst)
    }

    /// A request with `host: gateway.test:8080`, the given `user-agent` (none for `None`) and
    /// no credential unless `credential` says so.
    fn ask(&self, method: &str, path: &str, user_agent: Option<&str>, credential: bool) -> Answer {
        let agent = user_agent.map_or(String::new(), |agent| format!("user-agent: {agent}\r\n"));
        let auth = if credential {
            format!("authorization: Bearer {OWNER_SECRET}\r\n")
        } else {
            String::new()
        };
        self.send(&format!(
            "{method} {path} HTTP/1.1\r\nhost: gateway.test:8080\r\n{agent}{auth}\r\n"
        ))
    }

    fn get(&self, path: &str, user_agent: Option<&str>) -> Answer {
        self.ask("GET", path, user_agent, false)
    }

    fn send(&self, raw: &str) -> Answer {
        exchange(self.addr, raw.as_bytes())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.shutdown();
        }
    }
}

#[derive(Debug)]
struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
    raw: String,
}

impl Answer {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn code(&self) -> Option<String> {
        let marker = "\"code\":\"";
        let start = self.body.find(marker)? + marker.len();
        let rest = &self.body[start..];
        Some(rest[..rest.find('"')?].to_string())
    }
}

fn exchange(addr: SocketAddr, request: &[u8]) -> Answer {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(request).unwrap();
    stream.flush().unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let raw = String::from_utf8(raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_str(), ""));
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|line| line.split_once(": "))
        .map(|(key, value)| (key.to_ascii_lowercase(), value.to_string()))
        .collect();
    Answer {
        status,
        headers,
        body: body.to_string(),
        raw: raw.clone(),
    }
}

const TEXT: &str = "text/plain; charset=utf-8";
const HTML: &str = "text/html; charset=utf-8";

// --- R5: GET /v1/models ------------------------------------------------------------------------

#[test]
fn r5_the_listing_answers_without_a_credential_one_entry_per_model_with_its_wires() {
    let fixture = Fixture::start(vec![
        declared("small", vec![Wire::Messages, Wire::Chat], 32_768),
        declared("code", Wire::ALL.to_vec(), 65_536),
    ]);
    let answer = fixture.get("/v1/models", None);
    assert_eq!(answer.status, 200, "{}", answer.raw);
    assert_eq!(answer.header("content-type"), Some("application/json"));
    assert_eq!(answer.header("cache-control"), Some("no-store"));
    assert_eq!(
        answer.body,
        "{\"object\":\"list\",\"data\":[\
         {\"id\":\"code\",\"object\":\"model\",\"owned_by\":\"llm-gateway\",\"max_model_len\":32768,\
         \"wires\":[\"chat\",\"responses\",\"messages\"]},\
         {\"id\":\"small\",\"object\":\"model\",\"owned_by\":\"llm-gateway\",\"max_model_len\":16384,\
         \"wires\":[\"messages\",\"chat\"]}]}"
    );
    assert_eq!(fixture.acquired(), 0);
}

#[test]
fn r5_a_model_that_declares_no_max_model_len_lists_none_and_never_zero() {
    let mut model = declared("bare", vec![Wire::Chat], 8192);
    model.max_model_len = None;
    let fixture = Fixture::start(vec![model]);
    let answer = fixture.get("/v1/models", None);
    assert_eq!(
        answer.body,
        "{\"object\":\"list\",\"data\":[{\"id\":\"bare\",\"object\":\"model\",\
         \"owned_by\":\"llm-gateway\",\"wires\":[\"chat\"]}]}"
    );
}

#[test]
fn r5_a_gateway_composed_without_a_relay_lists_no_model() {
    let fixture = Fixture::composed(None, true);
    let answer = fixture.get("/v1/models", None);
    assert_eq!(answer.status, 200, "{}", answer.raw);
    assert_eq!(answer.body, "{\"object\":\"list\",\"data\":[]}");
}

// --- R1: GET / ---------------------------------------------------------------------------------

#[test]
fn r1_a_codex_user_agent_gets_the_codex_config_toml_profile_per_model() {
    let fixture = Fixture::start(two_models());
    let answer = fixture.get("/", Some(CODEX));
    assert_eq!(answer.status, 200, "{}", answer.raw);
    assert_eq!(answer.header("content-type"), Some(TEXT));
    for (alias, window) in [("code", 65_536), ("small", 32_768)] {
        let profile = format!(
            "model = \"{alias}\"\nmodel_provider = \"b10x-gateway\"\n\
             model_context_window = {window}\n\n[model_providers.b10x-gateway]\n\
             name = \"b10x llm-gateway\"\nbase_url = \"http://gateway.test:8080/v1\"\n\
             env_key = \"B10X_GATEWAY_TOKEN\"\nwire_api = \"responses\"\n"
        );
        assert!(answer.body.contains(&profile), "{profile}\nin\n{}", answer.body);
        assert!(
            answer
                .body
                .contains(&format!("# ~/.codex/{alias}.config.toml\n")),
            "{}",
            answer.body
        );
    }
    assert!(!answer.body.contains("ANTHROPIC_"), "{}", answer.body);
}

#[test]
fn r1_a_claude_user_agent_gets_the_claude_code_environment_variables_per_model() {
    let fixture = Fixture::start(two_models());
    for agent in [CLAUDE, "Anthropic-SDK/0.1", "my-CLAUDE-wrapper"] {
        let answer = fixture.get("/", Some(agent));
        assert_eq!(answer.status, 200, "{}", answer.raw);
        assert_eq!(answer.header("content-type"), Some(TEXT));
        for (alias, window) in [("code", 65_536), ("small", 32_768)] {
            let block = format!(
                "# model {alias}\n\
                 export ANTHROPIC_BASE_URL='http://gateway.test:8080'\n\
                 export ANTHROPIC_MODEL='{alias}'\n\
                 export ANTHROPIC_DEFAULT_OPUS_MODEL='{alias}'\n\
                 export ANTHROPIC_DEFAULT_SONNET_MODEL='{alias}'\n\
                 export ANTHROPIC_DEFAULT_HAIKU_MODEL='{alias}'\n\
                 export CLAUDE_CODE_SUBAGENT_MODEL='{alias}'\n\
                 export CLAUDE_CODE_MAX_CONTEXT_TOKENS='{window}'\n"
            );
            assert!(answer.body.contains(&block), "{agent}: {block}\nin\n{}", answer.body);
        }
        // The credential setting is named, never given a value.
        assert!(answer.body.contains("ANTHROPIC_AUTH_TOKEN"), "{}", answer.body);
        assert!(!answer.body.contains("export ANTHROPIC_AUTH_TOKEN"), "{}", answer.body);
        assert!(!answer.body.contains("wire_api"), "{}", answer.body);
    }
}

/// The Loom profile of design § 5 step 7, for one model on the chat wire.
fn loom_lines(alias: &str, window: u64) -> String {
    format!(
        "[[endpoints]]\nbase_url = \"http://gateway.test:8080/v1\"\n\n\
         [[models]]\nupstream_name = \"{alias}\"\n\n\
         [[serving_models]]\nprotocol = \"chat-completions\"\n\
         [serving_models.capabilities]\ntools = true\ncontext_window = {window}\n"
    )
}

#[test]
fn r1_any_other_user_agent_or_none_gets_plain_text_with_the_loom_profile_per_model() {
    let fixture = Fixture::start(two_models());
    for agent in [Some(CURL), None, Some("python-requests/2.32")] {
        let answer = fixture.get("/", agent);
        assert_eq!(answer.status, 200, "{}", answer.raw);
        assert_eq!(answer.header("content-type"), Some(TEXT), "{agent:?}");
        assert!(answer.body.contains("auth_kind = \"bearer\""), "{}", answer.body);
        for (alias, window) in [("code", 65_536), ("small", 32_768)] {
            assert!(
                answer.body.contains(&loom_lines(alias, window)),
                "{agent:?}: {}\nin\n{}",
                loom_lines(alias, window),
                answer.body
            );
        }
        assert!(!answer.body.contains("<html"), "{}", answer.body);
    }
}

#[test]
fn r1_a_browser_gets_an_html_page_with_the_loom_profile_per_model() {
    let fixture = Fixture::start(two_models());
    let answer = fixture.get("/", Some(BROWSER));
    assert_eq!(answer.status, 200, "{}", answer.raw);
    assert_eq!(answer.header("content-type"), Some(HTML));
    assert!(answer.body.starts_with("<!DOCTYPE html>\n"), "{}", answer.body);
    assert!(answer.body.contains("<pre>"), "{}", answer.body);
    for (alias, window) in [("code", 65_536), ("small", 32_768)] {
        let escaped = loom_lines(alias, window).replace('"', "&quot;");
        assert!(answer.body.contains(&escaped), "{escaped}\nin\n{}", answer.body);
    }
    // The same page for any agent that starts with `mozilla/`, whatever its case.
    let lower = fixture.get("/", Some("mozilla/5.0 (X11; Linux x86_64)"));
    assert_eq!(lower.header("content-type"), Some(HTML));
    assert_eq!(lower.body, answer.body);
}

#[test]
fn r1_the_rules_are_tried_in_order_codex_then_claude_then_a_browser() {
    let fixture = Fixture::start(two_models());
    let content = |agent: &str| {
        let answer = fixture.get("/", Some(agent));
        (answer.header("content-type").map(str::to_owned), answer.body)
    };
    let (_, codex) = content(CODEX);
    let (_, claude) = content(CLAUDE);
    assert_eq!(content("Mozilla/5.0 codex").1, codex);
    assert_eq!(content("Mozilla/5.0 claude").1, claude);
    assert_eq!(content("claude codex").1, codex);
    // `mozilla/` counts only at the start.
    assert_eq!(content("not mozilla/5.0").0.as_deref(), Some(TEXT));
}

#[test]
fn r1_a_model_that_does_not_declare_the_clients_wire_gets_no_profile_for_that_client() {
    let fixture = Fixture::start(vec![
        declared("code", Wire::ALL.to_vec(), 65_536),
        declared("chatonly", vec![Wire::Chat], 32_768),
        declared("textwire", vec![Wire::Responses, Wire::Messages], 32_768),
    ]);
    let codex = fixture.get("/", Some(CODEX)).body;
    let claude = fixture.get("/", Some(CLAUDE)).body;
    let plain = fixture.get("/", Some(CURL)).body;
    assert!(codex.contains("model = \"code\""), "{codex}");
    assert!(!codex.contains("chatonly"), "{codex}");
    assert!(codex.contains("model = \"textwire\""), "{codex}");
    assert!(claude.contains("ANTHROPIC_MODEL='code'"), "{claude}");
    assert!(!claude.contains("chatonly"), "{claude}");
    assert!(claude.contains("ANTHROPIC_MODEL='textwire'"), "{claude}");
    assert!(plain.contains("upstream_name = \"chatonly\""), "{plain}");
    assert!(!plain.contains("textwire"), "{plain}");
}

#[test]
fn r1_a_model_whose_tool_calling_is_absent_gets_no_profile_for_any_client() {
    let mut text_only = declared("textonly", Wire::ALL.to_vec(), 32_768);
    text_only.tool_calling = ToolCalling::Absent;
    let fixture = Fixture::start(vec![declared("code", Wire::ALL.to_vec(), 65_536), text_only]);
    for agent in [CODEX, CLAUDE, CURL, BROWSER] {
        let body = fixture.get("/", Some(agent)).body;
        assert!(body.contains("code"), "{agent}: {body}");
        assert!(!body.contains("textonly"), "{agent}: {body}");
    }
    // The listing still names it: it serves text.
    assert!(fixture.get("/v1/models", None).body.contains("\"id\":\"textonly\""));
}

#[test]
fn r1_a_model_without_a_context_window_gets_no_context_setting() {
    let mut model = declared("bare", Wire::ALL.to_vec(), 8192);
    model.context_window = None;
    let fixture = Fixture::start(vec![model]);
    let codex = fixture.get("/", Some(CODEX)).body;
    let claude = fixture.get("/", Some(CLAUDE)).body;
    let plain = fixture.get("/", Some(CURL)).body;
    assert!(codex.contains("model = \"bare\""), "{codex}");
    assert!(!codex.contains("model_context_window"), "{codex}");
    assert!(claude.contains("ANTHROPIC_MODEL='bare'"), "{claude}");
    assert!(!claude.contains("CLAUDE_CODE_MAX_CONTEXT_TOKENS='"), "{claude}");
    assert!(plain.contains("upstream_name = \"bare\""), "{plain}");
    assert!(!plain.contains("context_window = "), "{plain}");
}

#[test]
fn r1_with_no_model_for_a_client_the_answer_says_so() {
    let fixture = Fixture::composed(None, true);
    for agent in [CODEX, CLAUDE, CURL, BROWSER] {
        let answer = fixture.get("/", Some(agent));
        assert_eq!(answer.status, 200, "{}", answer.raw);
        assert!(answer.body.contains("No model of this gateway"), "{agent}: {}", answer.body);
    }
}

#[test]
fn r1_the_origin_is_the_host_header_when_it_is_a_plain_authority_and_the_listener_otherwise() {
    let fixture = Fixture::start(two_models());
    let ask = |host: &str| {
        fixture
            .send(&format!(
                "GET / HTTP/1.1\r\nhost: {host}\r\nuser-agent: {CLAUDE}\r\n\r\n"
            ))
            .body
    };
    assert!(ask("[::1]:8080").contains("ANTHROPIC_BASE_URL='http://[::1]:8080'"));
    assert!(ask("models_1.example-host").contains("ANTHROPIC_BASE_URL='http://models_1.example-host'"));
    let listener = format!("ANTHROPIC_BASE_URL='http://{}'", fixture.addr);
    for hostile in ["evil'$(id)", "a\"<script>", "a/b", "a@b", "evil host"] {
        let body = ask(hostile);
        assert!(body.contains(&listener), "{hostile}: {body}");
        assert!(!body.contains(hostile), "{hostile}: {body}");
    }
    let absent = fixture
        .send(&format!("GET / HTTP/1.1\r\nuser-agent: {CLAUDE}\r\n\r\n"))
        .body;
    assert!(absent.contains(&listener), "{absent}");
}

#[test]
fn r1_aliases_are_escaped_for_the_text_they_are_written_into() {
    let fixture = Fixture::start(vec![declared("o'k\"<b>&\\", Wire::ALL.to_vec(), 8192)]);
    let codex = fixture.get("/", Some(CODEX)).body;
    assert!(codex.contains("model = \"o'k\\\"<b>&\\\\\""), "{codex}");
    let claude = fixture.get("/", Some(CLAUDE)).body;
    assert!(claude.contains("ANTHROPIC_MODEL='o'\\''k\"<b>&\\'"), "{claude}");
    let html = fixture.get("/", Some(BROWSER)).body;
    assert!(
        html.contains("upstream_name = &quot;o&#39;k\\&quot;&lt;b&gt;&amp;\\\\&quot;"),
        "{html}"
    );
    assert!(!html.contains("<b>"), "{html}");
}

#[test]
fn r1_each_served_instructions_answer_counts_one_instruction_view() {
    let fixture = Fixture::start(two_models());
    fixture.get("/", Some(CODEX));
    fixture.get("/", None);
    fixture.ask("HEAD", "/", Some(BROWSER), false);
    fixture.get("/v1/models", None);
    fixture.ask("POST", "/", None, false);
    let scrape = fixture.ask("GET", "/metrics", None, true);
    assert!(
        scrape.body.contains("\nllmgw_instruction_views_total 3\n"),
        "{}",
        scrape.body
    );
}

// --- R1, R5: the properties both routes hold -----------------------------------------------------

#[test]
fn r1_r5_neither_route_asks_the_target_source_or_renders_an_owner_only_fact() {
    let fixture = Fixture::start(two_models());
    let mut answers = Vec::new();
    for method in ["GET", "HEAD"] {
        answers.push(fixture.ask(method, "/v1/models", None, false));
        for agent in [Some(CODEX), Some(CLAUDE), Some(BROWSER), Some(CURL), None] {
            answers.push(fixture.ask(method, "/", agent, false));
        }
    }
    // The owner's own credential changes nothing about what a public route renders.
    answers.push(fixture.ask("GET", "/v1/models", None, true));
    answers.push(fixture.ask("GET", "/", Some(CLAUDE), true));
    for answer in &answers {
        assert_eq!(answer.status, 200, "{}", answer.raw);
        for fact in OWNER_ONLY {
            assert!(
                !answer.raw.contains(fact),
                "a public answer renders the owner-only {fact:?}: {}",
                answer.raw
            );
        }
    }
    // Positive control: the facts are composed and the owner sees them.
    let routes = fixture.ask("GET", "/v1/routes", None, true);
    assert!(routes.body.contains("endpoint-label-private"), "{}", routes.body);
    assert!(routes.body.contains(DIGEST), "{}", routes.body);
    assert_eq!(fixture.acquired(), 0, "a public route asked for a target");
    // And the counter counts: a relayed request does acquire.
    fixture.send(&format!(
        "POST /v1/chat/completions HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER_SECRET}\r\n\
         content-length: 16\r\n\r\n{{\"model\":\"code\"}}"
    ));
    assert_eq!(fixture.acquired(), 1);
}

#[test]
fn r1_r5_only_get_and_head_are_public_another_method_is_credential_absent_then_not_allowed() {
    let fixture = Fixture::start(two_models());
    for path in ["/v1/models", "/"] {
        for method in ["POST", "PUT", "DELETE", "OPTIONS"] {
            let anonymous = fixture.ask(method, path, None, false);
            assert_eq!(anonymous.status, 401, "{method} {path}: {}", anonymous.raw);
            assert_eq!(anonymous.code().as_deref(), Some("credential-absent"));
            assert_eq!(anonymous.header("www-authenticate"), Some("Bearer"));
            let owner = fixture.ask(method, path, None, true);
            assert_eq!(owner.status, 405, "{method} {path}: {}", owner.raw);
            assert_eq!(owner.code().as_deref(), Some("method-not-allowed"));
            assert_eq!(owner.header("allow"), Some("GET, HEAD"));
        }
        let head = fixture.ask("HEAD", path, Some(CODEX), false);
        assert_eq!(head.status, 200, "{}", head.raw);
        assert_eq!(head.body, "");
    }
    // Only the two literal paths: a neighbour still takes the credential.
    for path in ["/v1/models/code", "/v1/model", "/index.html", "//"] {
        let answer = fixture.get(path, None);
        assert_eq!(answer.code().as_deref(), Some("credential-absent"), "{path}");
    }
    assert_eq!(fixture.acquired(), 0);
}

#[test]
fn r1_r5_the_route_inventory_still_takes_the_owner_credential() {
    let fixture = Fixture::start(two_models());
    for path in ["/v1/routes", "/v1/routes/code", "/metrics"] {
        let answer = fixture.get(path, None);
        assert_eq!(answer.status, 401, "{path}: {}", answer.raw);
        assert_eq!(answer.code().as_deref(), Some("credential-absent"));
    }
}

#[test]
fn r1_r5_before_readiness_the_public_routes_are_unavailable_and_take_no_body() {
    let fixture = Fixture::composed(Some(two_models()), false);
    for path in ["/v1/models", "/"] {
        let answer = fixture.get(path, Some(CODEX));
        assert_eq!(answer.status, 503, "{path}: {}", answer.raw);
        assert_eq!(answer.code().as_deref(), Some("unavailable"));
    }
    fixture.handle.as_ref().unwrap().mark_ready();
    for path in ["/v1/models", "/"] {
        let answer = fixture.send(&format!(
            "GET {path} HTTP/1.1\r\nhost: h\r\ncontent-length: 2\r\n\r\n{{}}"
        ));
        assert_eq!(answer.status, 400, "{path}: {}", answer.raw);
        assert_eq!(answer.code().as_deref(), Some("body-not-allowed"));
    }
    // A draining gateway keeps answering them, as it keeps serving inspection.
    fixture.handle.as_ref().unwrap().begin_drain();
    assert_eq!(fixture.get("/v1/models", None).status, 200);
}

// --- The specification and the contract ----------------------------------------------------------

/// The attribute `name` of `variant` in the enum `declaration` of spec/domains/gateway.yaml.
fn variant_attribute(declaration: &str, variant: &str, name: &str) -> String {
    let start = SPEC
        .find(&format!("  - name: {declaration}\n"))
        .unwrap_or_else(|| panic!("spec/domains/gateway.yaml declares no {declaration}"));
    let line = SPEC[start..]
        .lines()
        .find(|line| line.trim_start().starts_with(&format!("- {{name: {variant}, ")))
        .unwrap_or_else(|| panic!("{declaration} declares no {variant}"));
    let marker = format!("{name}: ");
    let value = &line[line.find(&marker).unwrap() + marker.len()..];
    let value = value.strip_prefix('"').map_or_else(
        || value[..value.find([',', '}']).unwrap()].to_string(),
        |quoted| quoted[..quoted.find('"').unwrap()].to_string(),
    );
    value
}

#[test]
fn r1_r5_every_public_answer_has_the_content_type_and_path_the_specification_declares() {
    let fixture = Fixture::start(two_models());
    assert_eq!(variant_attribute("llm-gateway.gateway.PublicRoute", "Models", "path"), "/v1/models");
    assert_eq!(variant_attribute("llm-gateway.gateway.PublicRoute", "Instructions", "path"), "/");
    let models = fixture.get("/v1/models", None);
    assert_eq!(
        models.header("content-type"),
        Some(variant_attribute("llm-gateway.gateway.PublicRoute", "Models", "content_type").as_str())
    );
    for (variant, agent) in [
        ("Codex", Some(CODEX)),
        ("ClaudeCode", Some(CLAUDE)),
        ("Html", Some(BROWSER)),
        ("Plain", Some(CURL)),
        ("Plain", None),
    ] {
        let declared =
            variant_attribute("llm-gateway.gateway.InstructionAnswer", variant, "content_type");
        let answer = fixture.get("/", agent);
        assert_eq!(answer.header("content-type"), Some(declared.as_str()), "{variant}");
    }
}

#[test]
fn r1_r5_the_contract_names_four_unauthenticated_paths_and_the_text_answers_of_get_root() {
    assert!(
        CONTRACT.contains(
            "The unauthenticated surface is a closed set of four literal paths with two literal \
             methods"
        ),
        "docs/gateway.md still describes a two-path unauthenticated surface"
    );
    for row in [
        "| `GET`, `HEAD` | `/v1/models` | none |",
        "| `GET`, `HEAD` | `/` | none |",
    ] {
        assert!(CONTRACT.contains(row), "docs/gateway.md has no row {row}");
    }
    assert!(
        CONTRACT.contains("except the counters at `/metrics`, which are\nPrometheus text, and the text and HTML answers of `GET /`"),
        "docs/gateway.md does not name the answers of GET / as an exception to application/json"
    );
}
