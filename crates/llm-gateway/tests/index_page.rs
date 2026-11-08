//! `GET /llms.txt` and the browser's `GET /`: one set of instructions. The page is the dashboard
//! bundle with the Markdown of `/llms.txt` inside it, so every command block of the text is in
//! the page; and on a loopback gateway that reads no credential, Claude Code is told to set
//! only `ANTHROPIC_BASE_URL`.

use llm_gateway::{
    Gateway, GatewayConfig, IngressAuth, Label, NoOwnerVerifier, Relay, RelayModel, RelayTarget,
    RelayTargets, RouteInventory, TargetRefusal, ToolCalling, Wire,
};
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::Arc,
    time::Duration,
};

struct Nowhere;

impl RelayTargets for Nowhere {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Err(TargetRefusal::Unavailable)
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

fn gateway(bind: &str) -> llm_gateway::GatewayHandle {
    let claude = RelayModel::new(
        Label::new("claude").unwrap(),
        Label::new("claude-sonnet-4-5").unwrap(),
        vec![Wire::Messages],
    )
    .unwrap()
    .with_client_credentials()
    .with_model_prefix("claude-")
    .unwrap()
    .with_tool_calling(ToolCalling::Parsed);
    let opus = RelayModel::new(
        Label::new("opus").unwrap(),
        Label::new("claude-opus-4-5").unwrap(),
        vec![Wire::Messages],
    )
    .unwrap()
    .with_client_credentials()
    .with_newest("opus")
    .unwrap()
    .with_tool_calling(ToolCalling::Parsed);
    let qwen = RelayModel::new(
        Label::new("qwen").unwrap(),
        Label::new("Served/Qwen").unwrap(),
        vec![Wire::Chat, Wire::Responses, Wire::Messages],
    )
    .unwrap()
    .with_tool_calling(ToolCalling::Parsed);
    let relay = Relay::new(vec![claude, opus, qwen], Arc::new(Nowhere)).unwrap();
    let mut config = GatewayConfig::new(bind.parse().unwrap());
    config.ingress_auth = IngressAuth::None;
    let handle = Gateway::bind_with_relay(
        config,
        Arc::new(NoOwnerVerifier),
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    handle.mark_ready();
    handle
}

fn get(handle: &llm_gateway::GatewayHandle, path: &str, agent: &str) -> (String, String) {
    let port = handle.local_addr().port();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\nuser-agent: {agent}\r\nconnection: close\r\n\r\n"
    )
    .unwrap();
    let mut answer = String::new();
    drop(stream.read_to_string(&mut answer));
    let (head, body) = answer.split_once("\r\n\r\n").unwrap_or((&answer, ""));
    (head.to_owned(), body.to_owned())
}

/// Every fenced code block of `markdown`, as its lines.
fn commands(markdown: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in markdown.lines() {
        if line.starts_with("```") {
            match current.take() {
                Some(lines) => blocks.push(lines.join("\n")),
                None => current = Some(Vec::new()),
            }
        } else if let Some(lines) = current.as_mut() {
            lines.push(line);
        }
    }
    blocks
}

fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[test]
fn the_page_holds_every_command_of_llms_txt() {
    let handle = gateway("127.0.0.1:0");
    let (head, markdown) = get(&handle, "/llms.txt", "curl/8.0");
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    assert!(
        head.contains("content-type: text/markdown; charset=utf-8"),
        "{head}"
    );
    let (head, page) = get(&handle, "/", "Mozilla/5.0 (X11; Linux x86_64)");
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    assert!(head.contains("text/html"), "{head}");
    assert!(
        page.contains(&format!("<pre id=\"llms\">{}</pre>", escaped(&markdown))),
        "the page embeds the very text /llms.txt answers"
    );
    let blocks = commands(&markdown);
    assert_eq!(blocks.len(), 2, "{markdown}");
    for block in &blocks {
        assert!(page.contains(&escaped(block)), "{block:?} is on the page");
    }
    assert!(page.contains("href=\\\"/dashboard\\\"") || page.contains("/dashboard"));
    handle.shutdown();
}

#[test]
fn a_local_gateway_shows_only_the_launchers() {
    let handle = gateway("127.0.0.1:0");
    let (_, markdown) = get(&handle, "/llms.txt", "curl/8.0");
    let blocks = commands(&markdown);
    assert_eq!(
        blocks,
        ["b10x-llm-gateway claude", "b10x-llm-gateway codex"],
        "{markdown}"
    );
    for manual in [
        "modelPicker",
        "settings.json",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_CUSTOM_MODEL_OPTION",
        "~/.codex/config.toml",
        "merge",
        "B10X_GATEWAY_TOKEN",
    ] {
        assert!(!markdown.contains(manual), "{manual} in {markdown}");
    }
    assert!(
        markdown.contains("| `claude`, `claude-*` | messages |"),
        "{markdown}"
    );
    assert!(
        markdown.contains("`opus` (the newest `opus`)"),
        "{markdown}"
    );
    handle.shutdown();
}

#[test]
fn a_gateway_off_loopback_gives_one_command_per_agent_with_the_owner_token() {
    let handle = gateway("0.0.0.0:0");
    let (head, markdown) = get(&handle, "/llms.txt", "curl/8.0");
    assert!(
        head.starts_with("HTTP/1.1 200 "),
        "the guide is public: {head}"
    );
    let blocks = commands(&markdown);
    assert_eq!(blocks.len(), 2, "{markdown}");
    assert!(
        blocks[0].ends_with("ANTHROPIC_AUTH_TOKEN=\"$B10X_GATEWAY_TOKEN\" claude"),
        "{markdown}"
    );
    assert!(
        blocks[1].starts_with(
            "codex -c 'model_providers.llm-gateway={ name = \"llm-gateway\", base_url = "
        ) && blocks[1].contains("env_key = \"B10X_GATEWAY_TOKEN\" }'")
            && blocks[1].ends_with("-c model_provider=llm-gateway -m 'qwen'"),
        "{markdown}"
    );
    assert!(!markdown.contains("merge"), "{markdown}");
    handle.shutdown();
}
