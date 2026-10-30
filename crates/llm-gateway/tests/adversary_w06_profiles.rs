//! Adversary cases, wave 2026-10-30-w06 pass 1, story:client-profiles-translated: what `GET /`
//! says to a client it profiles for no model, now that a model declaring the client's wire can
//! be left out (a translated-only model, as a Bedrock model is, whose translator does not admit
//! the client).

use llm_gateway::{
    Gateway, GatewayConfig, GatewayHandle, Label, OwnerToken, RefusalCode, Relay, RelayModel,
    RelayTarget, RelayTargets, RouteInventory, SharedSecretVerifier, TargetRefusal, ToolCalling,
    Translation, TranslationRequest, Translator, Wire,
};
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::Arc,
    time::Duration,
};

const OWNER_SECRET: &str = "owner-token-adversary-w06-profiles-0123456";

struct NoTargets;

impl RelayTargets for NoTargets {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        panic!("a public route asked the target source for a target");
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

/// A translator that translates nothing and keeps the port's default `admits`: `false`.
struct AdmitsNothing;

impl Translator for AdmitsNothing {
    fn prepare(
        &self,
        _request: &TranslationRequest<'_>,
    ) -> Result<Box<dyn Translation>, RefusalCode> {
        Err(RefusalCode::TranslationUnsupported)
    }
}

fn gateway(models: Vec<RelayModel>, translator: Arc<dyn Translator>) -> GatewayHandle {
    let relay = Relay::new(models, Arc::new(NoTargets))
        .unwrap()
        .with_translator(translator);
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER_SECRET.as_bytes().to_vec()).unwrap())
            .unwrap(),
    );
    let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
    config.read_timeout = Duration::from_secs(5);
    let handle = Gateway::bind_with_relay(
        config,
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    handle.mark_ready();
    handle
}

fn root(handle: &GatewayHandle, user_agent: &str) -> String {
    let mut stream = TcpStream::connect(handle.local_addr()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "GET / HTTP/1.1\r\nhost: gateway\r\nuser-agent: {user_agent}\r\n\r\n"
    )
    .unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    String::from_utf8_lossy(&raw).into_owned()
}

/// A Bedrock-shaped model (translated-only, declaring the messages wire, tool calling parsed)
/// whose translator does not admit Claude Code is left out of Claude Code's answer, as the story
/// decided. The answer then gives its reason as "none declares the messages wire with tool
/// calling parsed", which is false: `bedrock` declares exactly that. An operator reading it looks
/// for a configuration error the document does not have. The reason the binary reaches: a
/// Bedrock model whose `max_output_tokens` is under Claude Code's 32000, which llm refuses
/// (`translation_a_client_is_not_admitted_where_llm_refuses_its_first_turn`, 16384 output tokens).
#[test]
fn adversary_w06_the_no_profile_answer_does_not_deny_a_wire_a_model_declares() {
    let bedrock = RelayModel::new(
        Label::new("bedrock").unwrap(),
        Label::new("bedrock").unwrap(),
        vec![Wire::Messages],
    )
    .unwrap()
    .with_translated_only()
    .with_tool_calling(ToolCalling::Parsed)
    .with_context_window(200_000);
    let handle = gateway(vec![bedrock], Arc::new(AdmitsNothing));
    let answer = root(&handle, "claude-cli/2.1.293 (external, cli)");
    handle.shutdown();
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    assert!(
        !answer.contains("ANTHROPIC_MODEL='bedrock'"),
        "precondition, Claude Code is not profiled for `bedrock`: {answer}"
    );
    assert!(
        !answer.contains("none declares the messages wire"),
        "`bedrock` declares the messages wire with tool calling parsed, yet the answer says none does:\n{answer}"
    );
}
