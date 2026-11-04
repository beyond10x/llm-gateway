//! `GET /llms.txt`: ready-to-run instructions for Claude Code and Codex against this gateway,
//! as Markdown, rendered from the relayed models and the gateway's own origin. `GET /` for a
//! browser is the same text inside the page the dashboard bundle renders, so the two cannot
//! hold different commands.
//!
//! Like the other public routes it reads the relayed models and nothing else: no target, no
//! credential. A model's upstream name is owner-only, so it is written only for a gateway that
//! is open on loopback, whose reader is its owner. A model without tool calling is left out:
//! both agents call tools on every turn.

use crate::relay::{Relay, RelayModel, ToolCalling, Wire};
use std::fmt::Write as _;

/// `GET /llms.txt`.
pub(crate) const PATH: &str = "/llms.txt";
/// Its content type.
pub(crate) const MARKDOWN: &str = "text/markdown; charset=utf-8";
/// Where the page bundle is mounted; the instructions go just before it.
const APP_MOUNT: &str = "<div id=\"app\"></div>";

/// A POSIX shell single-quoted word.
fn shell(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// A TOML basic string.
fn toml(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// How a model's calls are authorised upstream.
fn credential(model: &RelayModel) -> &'static str {
    if model.client_credentials() {
        "your own Claude login, forwarded"
    } else {
        "the gateway's own"
    }
}

fn names(model: &RelayModel) -> String {
    let alias = model.alias().as_str();
    match (model.model_prefix(), model.newest()) {
        (Some(prefix), _) => format!("`{alias}`, `{prefix}*`"),
        (None, Some(family)) => format!("`{alias}` (the newest `{family}`)"),
        (None, None) => format!("`{alias}`"),
    }
}

/// The `/model` picker label of a model: its alias, with its upstream name for the owner.
fn label(model: &RelayModel, open: bool) -> String {
    let alias = model.alias().as_str();
    let upstream = model.upstream_model().as_str();
    if open && upstream != alias {
        format!("{alias} ({upstream})")
    } else {
        alias.to_owned()
    }
}

/// The instructions for the models of `relay` (none without one), every URL under `origin`.
/// `open` is a gateway that reads no credential at all (loopback, ingress auth off).
pub(crate) fn render(relay: Option<&Relay>, origin: &str, open: bool) -> String {
    let models: Vec<&RelayModel> = relay
        .iter()
        .flat_map(|relay| relay.models())
        .filter(|model| model.tool_calling() != ToolCalling::Absent)
        .collect();
    let reaches = |model: &RelayModel, wire| relay.is_some_and(|relay| relay.reaches(model, wire));
    let mut text = String::from("# llm-gateway\n\n");
    let _ = writeln!(
        text,
        "This gateway listens at <{origin}>. Live calls, sessions and tokens per second: \
         [dashboard]({origin}/dashboard). Prometheus series: [metrics]({origin}/metrics).\n"
    );
    if open {
        text.push_str(
            "It reads no credential: it listens on loopback with `ingress_auth = \"none\"`.\n\n",
        );
    } else {
        text.push_str(
            "Every call carries the gateway's owner token as `Authorization: Bearer`; export it \
             as `B10X_GATEWAY_TOKEN` below.\n\n",
        );
    }
    text.push_str("## Routes\n\n");
    if models.is_empty() {
        text.push_str("This gateway relays no model.\n\n");
    } else {
        text.push_str(
            "| Model | Wires | Context window | Credential upstream |\n| --- | --- | --- | --- |\n",
        );
        for model in &models {
            let wires: Vec<&str> = model.wires().iter().map(|wire| wire.label()).collect();
            let window = model
                .window()
                .map_or_else(|| "-".to_owned(), |window| window.to_string());
            let _ = writeln!(
                text,
                "| {} | {} | {window} | {} |",
                names(model),
                wires.join(", "),
                credential(model)
            );
        }
        text.push('\n');
    }
    claude_code(&mut text, &models, &reaches, origin, open);
    codex(&mut text, &models, &reaches, origin, open);
    text
}

fn claude_code(
    text: &mut String,
    models: &[&RelayModel],
    reaches: &dyn Fn(&RelayModel, Wire) -> bool,
    origin: &str,
    open: bool,
) {
    text.push_str("## Claude Code\n\n");
    if !models.iter().any(|model| reaches(model, Wire::Messages)) {
        text.push_str("No model of this gateway serves the messages wire Claude Code speaks.\n\n");
        return;
    }
    if open {
        text.push_str(
            "```sh\nb10x-llm-gateway claude\n```\n\nIt starts `claude` against this gateway with a \
             temporary settings file: the base URL, the `[launch.claude]` implementer route for \
             subagents, and this gateway's models in `/model`. Your claude.ai \
             login is kept, and every other argument goes to `claude`.\n\n",
        );
    } else {
        let _ = writeln!(
            text,
            "```sh\nANTHROPIC_BASE_URL={} ANTHROPIC_AUTH_TOKEN=\"$B10X_GATEWAY_TOKEN\" claude\n```\n",
            shell(origin)
        );
    }
}

fn codex(
    text: &mut String,
    models: &[&RelayModel],
    reaches: &dyn Fn(&RelayModel, Wire) -> bool,
    origin: &str,
    open: bool,
) {
    text.push_str("## Codex\n\n");
    let served: Vec<&&RelayModel> = models
        .iter()
        .filter(|model| reaches(model, Wire::Responses) && !model.client_credentials())
        .collect();
    let Some(first) = served.first() else {
        text.push_str("No model of this gateway serves the responses wire Codex speaks.\n\n");
        return;
    };
    if open {
        text.push_str(
            "```sh\nb10x-llm-gateway codex\n```\n\nIt starts `codex` against this gateway with \
             the provider, the `[launch.codex]` model and its context window given as `-c` \
             overrides; nothing is written to `~/.codex/`, and every other argument goes to \
             `codex`.\n\n",
        );
        return;
    }
    let provider = format!(
        "model_providers.llm-gateway={{ name = \"llm-gateway\", base_url = {}, wire_api = \"responses\", env_key = \"B10X_GATEWAY_TOKEN\" }}",
        toml(&format!("{origin}/v1"))
    );
    let _ = writeln!(
        text,
        "```sh\ncodex -c {} -c model_provider=llm-gateway -m {}\n```\n",
        shell(&provider),
        shell(first.alias().as_str())
    );
}

/// `GET /v1/launch`, owner-only: what `b10x-llm-gateway claude` writes its settings overlay
/// from and `b10x-llm-gateway codex` its overrides.
pub(crate) const LAUNCH: &str = "/v1/launch";
/// The launch document's format.
pub(crate) const LAUNCH_FORMAT: &str = "llm-gateway.launch/1";

/// The launch document: the gateway's origin and, per relayed model, its alias, prefix, whether
/// it forwards the client's own credential, whether Claude Code reaches it on the messages wire
/// and Codex on the responses wire, its context and auto-compact windows, its `/model` label and its phase where the source knows
/// one. It reads each model's status, so it is never a public route.
pub(crate) fn launch(relay: Option<&Relay>, origin: &str) -> String {
    let mut writer = crate::json::Writer::new();
    writer.begin_object();
    writer.key("format");
    writer.string(LAUNCH_FORMAT);
    writer.key("base_url");
    writer.string(origin);
    writer.key("routes");
    writer.begin_array();
    let models = relay
        .iter()
        .flat_map(|relay| relay.models().map(move |model| (*relay, model)));
    for (relay, model) in models {
        let alias = model.alias().as_str();
        writer.begin_object();
        writer.key("alias");
        writer.string(alias);
        writer.key("model_prefix");
        optional(&mut writer, model.model_prefix());
        writer.key("client_credentials");
        writer.boolean(model.client_credentials());
        writer.key("messages");
        writer.boolean(relay.reaches(model, Wire::Messages));
        writer.key("responses");
        writer.boolean(relay.reaches(model, Wire::Responses));
        writer.key("tool_calling");
        writer.boolean(model.tool_calling() != ToolCalling::Absent);
        writer.key("context_window");
        match model.window() {
            Some(window) => writer.number(window),
            None => writer.null(),
        }
        writer.key("compact_window");
        match model.compact_window() {
            Some(window) => writer.number(window),
            None => writer.null(),
        }
        writer.key("label");
        writer.string(&label(model, true));
        writer.key("phase");
        let phase = relay.status(alias).ok().map(|status| status.phase.code());
        optional(&mut writer, phase);
        writer.end_object();
    }
    writer.end_array();
    writer.end_object();
    writer.finish()
}

fn optional(writer: &mut crate::json::Writer, value: Option<&str>) {
    match value {
        Some(value) => writer.string(value),
        None => writer.null(),
    }
}

/// The page bundle with `markdown` inside it, HTML-escaped in a `<pre id="llms">` the bundle
/// renders and a browser without scripts shows as it is.
pub(crate) fn page(bundle: &str, markdown: &str) -> String {
    let mut block = String::with_capacity(markdown.len() + 64);
    block.push_str("<pre id=\"llms\">");
    for character in markdown.chars() {
        match character {
            '&' => block.push_str("&amp;"),
            '<' => block.push_str("&lt;"),
            '>' => block.push_str("&gt;"),
            '"' => block.push_str("&quot;"),
            '\'' => block.push_str("&#39;"),
            other => block.push(other),
        }
    }
    block.push_str("</pre>");
    block.push_str(APP_MOUNT);
    bundle.replacen(APP_MOUNT, &block, 1)
}

#[cfg(test)]
mod tests {
    use super::{APP_MOUNT, page, shell, toml};

    #[test]
    fn the_page_holds_the_markdown_escaped_before_the_mount() {
        let bundle = format!("<body>{APP_MOUNT}<script>x</script></body>");
        let out = page(&bundle, "a <b> & 'c'");
        assert_eq!(
            out,
            format!(
                "<body><pre id=\"llms\">a &lt;b&gt; &amp; &#39;c&#39;</pre>{APP_MOUNT}<script>x</script></body>"
            )
        );
    }

    #[test]
    fn values_are_quoted_for_the_text_they_are_written_into() {
        assert_eq!(shell("it's"), "'it'\\''s'");
        assert_eq!(toml("a\"b\\"), "\"a\\\"b\\\\\"");
    }
}
