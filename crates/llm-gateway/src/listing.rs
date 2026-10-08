//! The two public routes (rows R5 and R1; story:public-model-listing): `GET /v1/models`, the
//! models in the `OpenAI` list shape, and `GET /`, setup instructions chosen by `User-Agent`.
//! The specification is `PublicRoute`, `ListedModel` and `InstructionAnswer` in
//! `spec/domains/gateway.yaml`, and `ClientProfile` in `spec/domains/clients.yaml`.
//!
//! Both read the relayed models and nothing else: no route inventory, no target source, no
//! credential. So neither can wake a pod, and neither has an owner-only fact to render.

use crate::{
    json::Writer,
    relay::{RelayModel, ToolCalling, Wire},
};
use std::fmt::Write as _;

/// `GET /v1/models`.
pub(crate) const MODELS: &str = "/v1/models";
/// `GET /`.
pub(crate) const INSTRUCTIONS: &str = "/";
/// The content type of the plain-text answers of `GET /`.
pub(crate) const TEXT: &str = "text/plain; charset=utf-8";
/// The content type of the HTML answer of `GET /`.
pub(crate) const HTML: &str = "text/html; charset=utf-8";

/// The answer `GET /` chooses (`llm-gateway.gateway.InstructionAnswer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstructionAnswer {
    Codex,
    ClaudeCode,
    Html,
    Plain,
}

impl InstructionAnswer {
    /// The first rule that holds for the lowercased `user-agent`, in the declared order (llmgw
    /// `src/lib.rs:231-245`). No `user-agent` is plain text.
    pub(crate) fn choose(user_agent: Option<&str>) -> Self {
        let agent = user_agent.unwrap_or_default().to_ascii_lowercase();
        if agent.contains("codex") {
            Self::Codex
        } else if agent.contains("claude") || agent.contains("anthropic") {
            Self::ClaudeCode
        } else if agent.starts_with("mozilla/") {
            Self::Html
        } else {
            Self::Plain
        }
    }

    pub(crate) const fn content_type(self) -> &'static str {
        match self {
            Self::Html => HTML,
            Self::Codex | Self::ClaudeCode | Self::Plain => TEXT,
        }
    }

    /// The answer for `models`, every base URL under `origin` (`http://<authority>`).
    pub(crate) fn render<'m>(
        self,
        models: impl Iterator<Item = &'m RelayModel>,
        origin: &str,
    ) -> String {
        match self {
            Self::Codex => codex(models, origin),
            Self::ClaudeCode => claude_code(models, origin),
            Self::Html => html(&plain(models, origin)),
            Self::Plain => plain(models, origin),
        }
    }
}

/// `{"object":"list","data":[entry…]}`, one `ListedModel` entry per model in alias order.
pub(crate) fn models<'m>(models: impl Iterator<Item = &'m RelayModel>) -> String {
    let mut writer = Writer::new();
    writer.begin_object();
    writer.key("object");
    writer.string("list");
    writer.key("data");
    writer.begin_array();
    for model in models {
        writer.begin_object();
        writer.key("id");
        writer.string(model.alias().as_str());
        writer.key("object");
        writer.string("model");
        writer.key("owned_by");
        writer.string("llm-gateway");
        // An undeclared length is absent, never zero.
        if let Some(length) = model.max_model_len() {
            writer.key("max_model_len");
            writer.number(length);
        }
        writer.key("wires");
        writer.begin_array();
        for wire in model.wires() {
            writer.string(wire.label());
        }
        writer.end_array();
        writer.end_object();
    }
    writer.end_array();
    writer.end_object();
    writer.finish()
}

/// Whether a client on `wire` gets a profile for `model`: the model declares the wire and
/// parses tool calls (`llm-gateway.clients.ClientProfile`).
fn profiled(model: &RelayModel, wire: Wire) -> bool {
    model.wires().contains(&wire) && model.tool_calling() == ToolCalling::Parsed
}

/// The gateway origin: `http://` and the request's `host` when it is a plain authority, the
/// listener's address otherwise, so no other caller byte is rendered.
pub(crate) fn origin(host: Option<&str>, listener: &std::net::SocketAddr) -> String {
    let plain = host.filter(|host| {
        !host.is_empty()
            && host.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'.' | b'-' | b'_' | b':' | b'[' | b']')
            })
    });
    match plain {
        Some(host) => format!("http://{host}"),
        None => format!("http://{listener}"),
    }
}

/// A TOML basic string.
fn toml(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        if matches!(character, '"' | '\\') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    quoted
}

/// A POSIX shell single-quoted word.
fn shell(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

const CODEX_HEADER: &str = "\
# Codex profiles for this llm-gateway, one per model it serves on the responses wire with
# tool calling parsed. Save one as the file it names, export B10X_GATEWAY_TOKEN as the
# gateway's owner token, and start Codex with `codex --profile <alias>`.
";

const CLAUDE_CODE_HEADER: &str = "\
# Claude Code settings for this llm-gateway, one block per model it serves on the messages
# wire with tool calling parsed. Set ANTHROPIC_AUTH_TOKEN to the gateway's owner token, or
# name an apiKeyHelper that prints it; ANTHROPIC_API_KEY is sent as x-api-key, which this
# gateway does not read.
";

const PLAIN_HEADER: &str = "\
llm-gateway

GET /v1/models lists the models this gateway serves. GET / with a User-Agent naming codex
answers Codex profiles, and one naming claude answers Claude Code settings. Every model call
carries the gateway's owner token as Authorization: Bearer.

# Loom, through an llm catalog (format = \"llm.catalog/1\"), one serving model per model this
# gateway serves on the chat wire with tool calling parsed. Give each table its ids, and the
# account a secret_reference_id that resolves to the owner token.
";

/// What an answer says when no model has a profile for its client.
fn none_for(client: &str, wire: Wire) -> String {
    format!(
        "\n# No model of this gateway serves {client}: none declares the {} wire with tool\n\
         # calling parsed.\n",
        wire.label()
    )
}

/// The Codex `config.toml` profile per model on the responses wire.
fn codex<'m>(models: impl Iterator<Item = &'m RelayModel>, origin: &str) -> String {
    let mut text = CODEX_HEADER.to_owned();
    let mut any = false;
    for model in models.filter(|model| profiled(model, Wire::Responses)) {
        any = true;
        let alias = model.alias().as_str();
        let _ = writeln!(text, "\n# ~/.codex/{alias}.config.toml");
        let _ = writeln!(text, "model = {}", toml(alias));
        text.push_str("model_provider = \"b10x-gateway\"\n");
        if let Some(window) = model.context_window() {
            let _ = writeln!(text, "model_context_window = {window}");
        }
        text.push_str("\n[model_providers.b10x-gateway]\nname = \"b10x llm-gateway\"\n");
        let _ = writeln!(text, "base_url = {}", toml(&format!("{origin}/v1")));
        text.push_str("env_key = \"B10X_GATEWAY_TOKEN\"\nwire_api = \"responses\"\n");
    }
    if !any {
        text.push_str(&none_for("Codex", Wire::Responses));
    }
    text
}

/// Claude Code's environment variables per model on the messages wire. Every model setting
/// names the alias, so no background request names a model the gateway refuses.
fn claude_code<'m>(models: impl Iterator<Item = &'m RelayModel>, origin: &str) -> String {
    const MODEL_SETTINGS: [&str; 5] = [
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "CLAUDE_CODE_SUBAGENT_MODEL",
    ];
    let mut text = CLAUDE_CODE_HEADER.to_owned();
    let mut any = false;
    for model in models.filter(|model| profiled(model, Wire::Messages)) {
        any = true;
        let alias = model.alias().as_str();
        let _ = writeln!(text, "\n# model {alias}");
        let _ = writeln!(text, "export ANTHROPIC_BASE_URL={}", shell(origin));
        for setting in MODEL_SETTINGS {
            let _ = writeln!(text, "export {setting}={}", shell(alias));
        }
        if let Some(window) = model.context_window() {
            let _ = writeln!(
                text,
                "export CLAUDE_CODE_MAX_CONTEXT_TOKENS={}",
                shell(&window.to_string())
            );
        }
    }
    if !any {
        text.push_str(&none_for("Claude Code", Wire::Messages));
    }
    text
}

/// The plain-text answer: where the others are found, and the Loom profile per model on the
/// chat wire, as the llm catalog lines of design § 5 step 7.
fn plain<'m>(models: impl Iterator<Item = &'m RelayModel>, origin: &str) -> String {
    let mut text = PLAIN_HEADER.to_owned();
    let mut any = false;
    for model in models.filter(|model| profiled(model, Wire::Chat)) {
        any = true;
        let alias = model.alias().as_str();
        let _ = writeln!(text, "\n# model {alias}");
        text.push_str("[[accounts]]\nauth_kind = \"bearer\"\n\n[[endpoints]]\n");
        let _ = writeln!(text, "base_url = {}", toml(&format!("{origin}/v1")));
        let _ = writeln!(text, "\n[[models]]\nupstream_name = {}", toml(alias));
        text.push_str(
            "\n[[serving_models]]\nprotocol = \"chat-completions\"\n\
             [serving_models.capabilities]\ntools = true\n",
        );
        if let Some(window) = model.context_window() {
            let _ = writeln!(text, "context_window = {window}");
        }
    }
    if !any {
        text.push_str(&none_for("Loom", Wire::Chat));
    }
    text
}

/// The plain text, HTML-escaped, inside one `<pre>` of a fixed page.
fn html(plain: &str) -> String {
    let mut page = String::from(
        "<!DOCTYPE html>\n<html lang=\"en\">\n\
         <head><meta charset=\"utf-8\"><title>llm-gateway</title></head>\n<body>\n<pre>\n",
    );
    for character in plain.chars() {
        match character {
            '&' => page.push_str("&amp;"),
            '<' => page.push_str("&lt;"),
            '>' => page.push_str("&gt;"),
            '"' => page.push_str("&quot;"),
            '\'' => page.push_str("&#39;"),
            other => page.push(other),
        }
    }
    page.push_str("</pre>\n</body>\n</html>\n");
    page
}

#[cfg(test)]
mod tests {
    use super::{InstructionAnswer, html, shell, toml};

    #[test]
    fn the_first_rule_that_holds_chooses_the_answer() {
        for (agent, answer) in [
            (Some("codex-cli/1.0"), InstructionAnswer::Codex),
            (Some("Mozilla/5.0 Codex"), InstructionAnswer::Codex),
            (Some("claude-cli/2.0"), InstructionAnswer::ClaudeCode),
            (Some("anthropic-sdk"), InstructionAnswer::ClaudeCode),
            (Some("Mozilla/5.0"), InstructionAnswer::Html),
            (Some("a mozilla/5.0"), InstructionAnswer::Plain),
            (Some("curl/8.0"), InstructionAnswer::Plain),
            (Some(""), InstructionAnswer::Plain),
            (None, InstructionAnswer::Plain),
        ] {
            assert_eq!(InstructionAnswer::choose(agent), answer, "{agent:?}");
        }
    }

    #[test]
    fn every_value_is_quoted_for_the_text_it_is_written_into() {
        assert_eq!(toml("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(shell("it's"), "'it'\\''s'");
        assert!(html("<a href=\"x\">&'").contains("&lt;a href=&quot;x&quot;&gt;&amp;&#39;"));
    }
}
