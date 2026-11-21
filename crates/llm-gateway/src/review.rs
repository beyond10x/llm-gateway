//! The caller-review port (`IngressAuth::ServiceAccountToken`).
//!
//! A gateway whose `ingress_auth` is `ServiceAccountToken` admits a model call whose bearer is
//! the owner's, as `Bearer` does, or one that an injected [`CallerReview`] admits as a named
//! [`Principal`]. The embedding implements the review (the binary asks the cluster's TokenReview
//! API); this crate holds only the port, so it opens no connection and reads no file for it.
//! A review that cannot be made is [`Review::Unavailable`], refused
//! `credential-review-unavailable`; a gateway composed without a review refuses every caller but
//! the owner that way. The review never sees the owner's bearer.

use crate::{auth::check_token, error::PrincipalError, error::TokenError};
use std::fmt;

/// The longest principal, `system:serviceaccount:<namespace>:<name>`, in bytes.
pub const MAX_PRINCIPAL_BYTES: usize = 128;
const SERVICE_ACCOUNT_PREFIX: &str = "system:serviceaccount:";
/// The longest Kubernetes namespace, a DNS label.
const MAX_NAMESPACE_BYTES: usize = 63;

/// A caller's bearer material, handed to a [`CallerReview`]. It is redacted in `Debug`, has no
/// `Display`, no `Clone` and no serialisation, and its bytes are overwritten when it is dropped.
/// Unlike [`crate::OwnerToken`] it hands its bytes out ([`Self::expose`]): a review has to send
/// them to whoever reviews them.
pub struct CallerToken(Vec<u8>);

impl CallerToken {
    /// Accepts what [`crate::OwnerToken::new`] accepts: printable US-ASCII of 1..=4096 bytes.
    ///
    /// # Errors
    /// Returns [`TokenError`] for empty, oversized or non-printable material.
    pub fn new(material: Vec<u8>) -> Result<Self, TokenError> {
        let token = Self(material);
        check_token(&token.0)?;
        Ok(token)
    }

    /// The material, for the review to send. Never log it.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for CallerToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CallerToken([REDACTED])")
    }
}

impl Drop for CallerToken {
    fn drop(&mut self) {
        self.0.fill(0);
        std::hint::black_box(&self.0);
    }
}

/// A reviewed caller: a Kubernetes service account, `system:serviceaccount:<namespace>:<name>`,
/// at most [`MAX_PRINCIPAL_BYTES`] bytes, the namespace a DNS label and the name a DNS
/// subdomain (lowercase letters, digits, `-` and `.`, beginning and ending with a letter or a
/// digit). It names an identity, never a credential, so it may be logged and recorded.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Principal(String);

impl Principal {
    /// # Errors
    /// [`PrincipalError::TooLarge`] past [`MAX_PRINCIPAL_BYTES`], and
    /// [`PrincipalError::NotAServiceAccount`] for anything else that is not one.
    pub fn new(text: &str) -> Result<Self, PrincipalError> {
        if text.len() > MAX_PRINCIPAL_BYTES {
            return Err(PrincipalError::TooLarge);
        }
        let (namespace, name) = text
            .strip_prefix(SERVICE_ACCOUNT_PREFIX)
            .and_then(|rest| rest.split_once(':'))
            .ok_or(PrincipalError::NotAServiceAccount)?;
        let edges = |part: &str| {
            part.bytes()
                .next()
                .is_some_and(|b| b.is_ascii_alphanumeric())
                && part
                    .bytes()
                    .last()
                    .is_some_and(|b| b.is_ascii_alphanumeric())
        };
        let lower = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-';
        let namespace_holds = namespace.len() <= MAX_NAMESPACE_BYTES
            && edges(namespace)
            && namespace.bytes().all(lower);
        let name_holds = edges(name) && name.bytes().all(|byte| lower(byte) || byte == b'.');
        if namespace_holds && name_holds {
            Ok(Self(text.to_owned()))
        } else {
            Err(PrincipalError::NotAServiceAccount)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a [`CallerReview`] decided. A refusal carries no reason: the caller is told only that it
/// failed, and the embedding logs why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Review {
    /// The token authenticates this principal, and the principal is admitted.
    Admitted(Principal),
    /// The token is not admitted: unauthenticated, expired, for another audience, or a principal
    /// the embedding does not admit. Refused `credential-rejected`.
    Rejected,
    /// No answer could be had: refused `credential-review-unavailable`, never admitted.
    Unavailable,
}

/// Reviews a caller's bearer token. It is asked on the connection's own thread, after the head
/// and before the body is read, so it should answer within its own bound.
///
/// An implementation must redact its `Debug`.
pub trait CallerReview: fmt::Debug + Send + Sync {
    fn review(&self, presented: &CallerToken) -> Review;
}

#[cfg(test)]
mod tests {
    use super::{CallerToken, MAX_PRINCIPAL_BYTES, Principal};
    use crate::error::{PrincipalError, TokenError};

    #[test]
    fn every_principal_error_has_text_that_provokes_it() {
        for expected in PrincipalError::ALL {
            let provocation = match expected {
                PrincipalError::TooLarge => {
                    format!("system:serviceaccount:agents:{}", "a".repeat(100))
                }
                PrincipalError::NotAServiceAccount => "system:node:agents".to_owned(),
            };
            assert_eq!(Principal::new(&provocation).unwrap_err(), *expected);
        }
    }

    #[test]
    fn a_principal_is_a_service_account_and_nothing_else() {
        for admitted in [
            "system:serviceaccount:agents:coding-agent",
            "system:serviceaccount:a:b",
            "system:serviceaccount:agents-2:agent.v1",
        ] {
            assert_eq!(Principal::new(admitted).unwrap().as_str(), admitted);
        }
        for refused in [
            "",
            "system:serviceaccount:",
            "system:serviceaccount:agents",
            "system:serviceaccount::agent",
            "system:serviceaccount:agents:",
            "system:serviceaccount:Agents:agent",
            "system:serviceaccount:agents:agent:extra",
            "system:serviceaccount:-agents:agent",
            "system:serviceaccount:agents:agent-",
            "system:serviceaccount:a.b:agent",
            "system:serviceaccount:agents:a gent",
            "user:agents:agent",
        ] {
            assert_eq!(
                Principal::new(refused).unwrap_err(),
                PrincipalError::NotAServiceAccount,
                "{refused:?}"
            );
        }
        assert_eq!(
            MAX_PRINCIPAL_BYTES, 128,
            "docs/gateway.md publishes this number"
        );
    }

    #[test]
    fn a_caller_token_follows_the_token_rules_and_is_redacted() {
        assert_eq!(CallerToken::new(Vec::new()).unwrap_err(), TokenError::Empty);
        assert_eq!(
            CallerToken::new(vec![b'a'; 4097]).unwrap_err(),
            TokenError::TooLarge
        );
        let token = CallerToken::new(b"header.payload.signature".to_vec()).unwrap();
        assert_eq!(token.expose(), b"header.payload.signature");
        assert_eq!(format!("{token:?}"), "CallerToken([REDACTED])");
    }
}
