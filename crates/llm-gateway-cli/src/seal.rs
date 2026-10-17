//! Reasoning state the gateway minted, sealed and bound per route
//! (story:cross-wire-reasoning-state; the decision on sealed reasoning state in
//! `spec/domains/gateway.yaml`, and docs/gateway.md "Reasoning state on a translated route").
//!
//! A seal is `llmgw-seal-1.`, the state as JSON in unpadded base64url, `.`, and in unpadded
//! base64url the HMAC-SHA256 of a fixed label, the route's alias and upstream model, the client's
//! wire, the upstream wire and those JSON bytes, each prefixed with its length. The key is
//! generated when the translator starts and lives only in this process: it is never written out,
//! logged or rendered, so a seal made before a restart no longer verifies. A seal attributes the
//! state it carries to this route; it does not hide it.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use llm_gateway::Wire;
use ring::{hmac, rand::SystemRandom};
use serde_json::Value;
use std::fmt;

/// The prefix of every seal.
pub const SEAL_PREFIX: &str = "llmgw-seal-1.";

/// Separates this use of the key from any other.
const LABEL: &[u8] = b"llm-gateway reasoning seal 1";

/// What a seal binds: the route and the two wires of the translation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scope<'a> {
    /// The model's alias: the route's name.
    pub alias: &'a str,
    /// The model's name upstream.
    pub upstream_model: &'a str,
    /// The client's wire.
    pub ingress: Wire,
    /// The upstream's wire.
    pub upstream: Wire,
}

/// The process's seal key, or none when the system random source gave none.
pub struct Sealer {
    key: Option<hmac::Key>,
}

impl fmt::Debug for Sealer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Sealer")
            .field("keyed", &self.key.is_some())
            .finish()
    }
}

impl Sealer {
    /// A sealer with a key generated now from the system random source. Without one it seals
    /// and opens nothing, and the translator carries no reasoning state across wires.
    pub fn generate() -> Self {
        let key = hmac::Key::generate(hmac::HMAC_SHA256, &SystemRandom::new()).ok();
        if key.is_none() {
            tracing::error!(
                "the translator could not generate its seal key; no reasoning state is carried to a translated route's next turn"
            );
        }
        Self { key }
    }

    /// `state` sealed for `scope`; `None` without a key.
    pub fn seal(&self, scope: Scope<'_>, state: &Value) -> Option<String> {
        let key = self.key.as_ref()?;
        let bytes = serde_json::to_vec(state).ok()?;
        let tag = hmac::sign(key, &message(scope, &bytes));
        Some(format!(
            "{SEAL_PREFIX}{}.{}",
            URL_SAFE_NO_PAD.encode(&bytes),
            URL_SAFE_NO_PAD.encode(tag.as_ref())
        ))
    }

    /// The state `seal` carries, when it is a seal this sealer made for exactly `scope`. The tag
    /// is checked in constant time before any byte of the state is read as JSON.
    pub fn open(&self, scope: Scope<'_>, seal: &str) -> Option<Value> {
        let key = self.key.as_ref()?;
        let (state, tag) = seal.strip_prefix(SEAL_PREFIX)?.split_once('.')?;
        let bytes = URL_SAFE_NO_PAD.decode(state).ok()?;
        let tag = URL_SAFE_NO_PAD.decode(tag).ok()?;
        hmac::verify(key, &message(scope, &bytes), &tag).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}

/// What the tag is computed over: every part prefixed with its length, so no two scopes and
/// states share one message.
fn message(scope: Scope<'_>, state: &[u8]) -> Vec<u8> {
    let parts: [&[u8]; 6] = [
        LABEL,
        scope.alias.as_bytes(),
        scope.upstream_model.as_bytes(),
        scope.ingress.label().as_bytes(),
        scope.upstream.label().as_bytes(),
        state,
    ];
    let mut message = Vec::with_capacity(parts.iter().map(|part| part.len() + 8).sum());
    for part in parts {
        message.extend_from_slice(&u64::try_from(part.len()).unwrap_or(u64::MAX).to_be_bytes());
        message.extend_from_slice(part);
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SCOPE: Scope<'static> = Scope {
        alias: "code",
        upstream_model: "served",
        ingress: Wire::Responses,
        upstream: Wire::Messages,
    };

    fn state() -> Value {
        json!({"type": "thinking", "thinking": "Read the file.", "signature": "sig-upstream-1"})
    }

    #[test]
    fn seal_opens_on_its_own_scope_only() {
        let sealer = Sealer::generate();
        let seal = sealer.seal(SCOPE, &state()).unwrap();
        assert_eq!(sealer.open(SCOPE, &seal), Some(state()));
        assert_eq!(
            sealer.open(SCOPE, &sealer.seal(SCOPE, &Value::Null).unwrap()),
            Some(Value::Null)
        );
        for other in [
            Scope {
                alias: "other",
                ..SCOPE
            },
            Scope {
                upstream_model: "served-2",
                ..SCOPE
            },
            Scope {
                ingress: Wire::Chat,
                ..SCOPE
            },
            Scope {
                upstream: Wire::Chat,
                ..SCOPE
            },
            // The length prefixes keep a part from moving into its neighbour.
            Scope {
                alias: "cod",
                upstream_model: "eserved",
                ..SCOPE
            },
        ] {
            assert_eq!(sealer.open(other, &seal), None, "{other:?}");
        }
    }

    #[test]
    fn seal_any_altered_character_or_cut_is_refused() {
        let sealer = Sealer::generate();
        let seal = sealer.seal(SCOPE, &state()).unwrap();
        for (index, original) in seal.char_indices() {
            for replacement in ['A', 'B', '.', '-'] {
                if replacement == original {
                    continue;
                }
                let mut altered = seal.clone();
                altered.replace_range(index..=index, &replacement.to_string());
                assert_eq!(sealer.open(SCOPE, &altered), None, "{index}: {altered}");
            }
        }
        for end in 0..seal.len() {
            assert_eq!(sealer.open(SCOPE, &seal[..end]), None, "cut at {end}");
        }
        assert_eq!(sealer.open(SCOPE, &format!("{seal}A")), None);
    }

    #[test]
    fn seal_another_key_and_a_forged_tag_are_refused() {
        let before = Sealer::generate();
        let seal = before.seal(SCOPE, &state()).unwrap();
        // A restart: the key that made the seal is gone.
        let after = Sealer::generate();
        assert_eq!(after.open(SCOPE, &seal), None);
        let state = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&state()).unwrap());
        let forged = format!(
            "{SEAL_PREFIX}{state}.{}",
            URL_SAFE_NO_PAD.encode([0_u8; 32])
        );
        assert_eq!(before.open(SCOPE, &forged), None);
        let unkeyed = Sealer { key: None };
        assert_eq!(unkeyed.seal(SCOPE, &Value::Null), None);
        assert_eq!(unkeyed.open(SCOPE, &seal), None);
    }

    #[test]
    fn seal_is_printable_and_renders_no_key() {
        let sealer = Sealer::generate();
        let seal = sealer.seal(SCOPE, &state()).unwrap();
        assert!(seal.starts_with(SEAL_PREFIX));
        assert!(
            seal[SEAL_PREFIX.len()..]
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        );
        assert_eq!(format!("{sealer:?}"), "Sealer { keyed: true }");
    }

    #[test]
    fn seal_prefix_is_the_published_one() {
        let contract = include_str!("../../../docs/gateway.md");
        assert!(contract.contains(&format!("`{SEAL_PREFIX}`")));
    }
}
