//! Who sent a model call: the session reader and the session hash the dashboard and the
//! finished-call value share (story:live-dashboard, story:finished-call-facts).
//!
//! A session is named by a hash of the header that identifies it (or of the user agent and
//! client address), never by the header's value; a user agent is kept as printable ASCII of at
//! most [`MAX_USER_AGENT_BYTES`]. agenticspec computes the same hash from a transcript to join
//! its records to the gateway's (`wire::session_hash` in `agenticspec-records`).

use crate::dashboard::MAX_USER_AGENT_BYTES;
use std::{fmt::Write as _, net::IpAddr};

/// The bytes of a session hash shown, as hex.
const SESSION_HEX: usize = 12;
/// The product Claude Code names in its `user-agent` (`claude-cli/<version> (…)`).
const CLAUDE_CODE_PRODUCT: &str = "claude-cli";

/// A user agent's product: what comes before its first `/` or space.
fn product(agent: &str) -> &str {
    agent.split(['/', ' ']).next().unwrap_or_default()
}

/// Who sent a call: a session hash and a label, the user agent, and the client's own retry
/// count. Read from the request head.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Observed {
    pub(crate) session: String,
    pub(crate) label: String,
    pub(crate) user_agent: Option<String>,
    pub(crate) client_retry: Option<u64>,
    /// No header named the session: it is keyed by user agent and client address, and
    /// [`Observed::at`] supplies the address.
    by_address: bool,
}

impl Observed {
    /// The session of a request: Claude Code's `x-claude-code-session-id`, else Codex's
    /// `session-id` (or `session_id`), else the `x-opencode-session` line, else the user agent
    /// and the client's address ([`Observed::at`]); hashed, never kept as sent. The label is the
    /// harness the header or the user agent names.
    pub(crate) fn read<'h>(header: impl Fn(&str) -> Option<&'h str>) -> Self {
        let user_agent = header("user-agent").map(printable);
        let product = user_agent
            .as_deref()
            .map(product)
            .filter(|product| !product.is_empty());
        let label = match product {
            Some(CLAUDE_CODE_PRODUCT) => "claude-code".to_owned(),
            Some(product) if product.starts_with("codex") => "codex".to_owned(),
            Some(product) => product.chars().take(32).collect(),
            None => "unknown".to_owned(),
        };
        let client_retry = header("x-stainless-retry-count").and_then(|count| count.parse().ok());
        let named = if let Some(id) = header("x-claude-code-session-id") {
            Some(("claude-code", id, "claude-code".to_owned()))
        } else {
            header("session-id")
                .or_else(|| header("session_id"))
                .or_else(|| header(crate::relay::CALLER_SESSION_HEADER))
                .map(|id| {
                    let label = if label == "unknown" {
                        "codex".to_owned()
                    } else {
                        label.clone()
                    };
                    ("session-id", id, label)
                })
        };
        match named {
            Some((source, id, label)) => Self {
                session: session_hash(source, id),
                label,
                user_agent,
                client_retry,
                by_address: false,
            },
            None => Self {
                session: String::new(),
                label,
                user_agent,
                client_retry,
                by_address: true,
            }
            .at(None),
        }
    }

    /// Whether the call's `user-agent` names Claude Code: its product, before the first `/` or
    /// space, is `claude-cli` (story:hosted-status-probe). A Claude Code session header without
    /// that user agent does not count.
    pub(crate) fn claude_code_agent(&self) -> bool {
        self.user_agent
            .as_deref()
            .is_some_and(|agent| product(agent) == CLAUDE_CODE_PRODUCT)
    }

    /// The session keyed by user agent and `peer`, when no header named it.
    pub(crate) fn at(mut self, peer: Option<IpAddr>) -> Self {
        if self.by_address {
            let peer = peer.map(|peer| peer.to_string()).unwrap_or_default();
            let agent = self.user_agent.as_deref().unwrap_or_default();
            self.session = session_hash("agent-address", &format!("{agent}\0{peer}"));
        }
        self
    }
}

/// The first [`SESSION_HEX`] hex digits of a SHA-256 over `llm-gateway.session/1`, the source
/// and the value, each pair separated by a NUL.
fn session_hash(source: &str, value: &str) -> String {
    let digest =
        crate::auth::sha256(format!("llm-gateway.session/1\0{source}\0{value}").as_bytes());
    let mut session = String::with_capacity(SESSION_HEX);
    for byte in &digest[..SESSION_HEX / 2] {
        let _ = write!(session, "{byte:02x}");
    }
    session
}

/// `value` as printable ASCII of at most [`MAX_USER_AGENT_BYTES`], every other byte a `?`.
fn printable(value: &str) -> String {
    value
        .bytes()
        .take(MAX_USER_AGENT_BYTES)
        .map(|byte| {
            if (0x20..0x7f).contains(&byte) {
                char::from(byte)
            } else {
                '?'
            }
        })
        .collect()
}
