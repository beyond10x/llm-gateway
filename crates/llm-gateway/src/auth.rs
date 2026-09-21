//! Owner authentication.
//!
//! The gateway serves exactly one owner. It is given a verifier, never a credential source: the
//! embedding resolves its material once, from whatever source it chose (a `llm-credentials`
//! file or keychain resolver, an environment-injected value, a hardware token), and hands the
//! result over. The crate's source names no filesystem, subprocess, environment or outbound
//! socket API and no logging macro — `the_crate_reaches_no_io_beyond_the_listener_it_was_given`
//! checks that against the source rather than asserting it — so the only I/O it performs is the
//! listener it was told to bind, and serving a request cannot cause a secret to be resolved.
//!
//! Multi-tenant accounts and quotas are outside this milestone. There is one owner.

use crate::error::{TokenError, VerifierError};
use std::fmt;

const MAX_TOKEN_BYTES: usize = 4096;
const MIN_SHARED_SECRET_BYTES: usize = 32;

/// Owner credential material. It is redacted in `Debug`, has no `Display`, no `Clone` and no
/// serialisation, and its bytes are overwritten when it is dropped.
pub struct OwnerToken(Vec<u8>);

impl OwnerToken {
    /// Accepts printable US-ASCII credential material of 1..=4096 bytes.
    ///
    /// # Errors
    /// Returns [`TokenError`] for empty, oversized or non-printable material, naming the rule
    /// and never the material.
    pub fn new(material: Vec<u8>) -> Result<Self, TokenError> {
        let token = Self(material);
        if token.0.is_empty() {
            return Err(TokenError::Empty);
        }
        if token.0.len() > MAX_TOKEN_BYTES {
            return Err(TokenError::TooLarge);
        }
        if !token.0.iter().all(u8::is_ascii_graphic) {
            return Err(TokenError::NotPrintableAscii);
        }
        Ok(token)
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn matches(&self, other: &Self) -> bool {
        constant_time_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for OwnerToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OwnerToken([REDACTED])")
    }
}

impl Drop for OwnerToken {
    fn drop(&mut self) {
        // Best effort without a `zeroize` dependency: the black box keeps the writes observable.
        self.0.fill(0);
        std::hint::black_box(&self.0);
    }
}

/// Compares in time independent of where the first differing byte is. The byte *length* of the
/// presented credential is still observable, which is the same exposure a length-checked
/// constant-time comparison has.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = u64::from(left.len() != right.len());
    for (a, b) in left.iter().zip(right.iter()) {
        difference |= u64::from(a ^ b);
    }
    std::hint::black_box(difference) == 0
}

/// The result of verifying presented material. It carries no reason, because a caller who failed
/// is told only that it failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The single owner.
    Owner,
    /// Anything else.
    Rejected,
}

impl Verdict {
    /// Turns an accepted verdict into the proof every inspection entry point demands.
    pub fn into_authenticated(self) -> Option<Authenticated> {
        match self {
            Self::Owner => Some(Authenticated(())),
            Self::Rejected => None,
        }
    }
}

/// Proof that an owner was authenticated.
///
/// It is constructed only from [`Verdict::into_authenticated`], which is what makes the
/// gateway's own request path check authentication before it decodes anything further. It is
/// **not** a capability: [`Verdict::Owner`] is a public variant, so any caller that can decide
/// who the owner is can mint this — which is the point of the injection seam, and is how an
/// embedding builds its own surface on [`crate::RouteInventory::inspect`]. Nor does it gate the
/// data: a caller holding a [`crate::RouteInventory`] reads the same identifier bytes through
/// [`crate::RouteInventory::route`] with no proof at all, and built those bytes in the first
/// place. What holds is the statement about the HTTP surface: no request reaching this gateway
/// is served an inspection unless the verifier accepted its credential.
/// `proof_of_authentication_is_a_marker_on_the_request_path_not_a_capability` pins that.
#[derive(Debug)]
pub struct Authenticated(());

/// Verifies presented owner material.
///
/// An implementation must redact its `Debug`: the gateway prints it when reporting composition.
pub trait OwnerVerifier: fmt::Debug + Send + Sync {
    /// Decides whether the presented material is the owner's.
    fn verify(&self, presented: &OwnerToken) -> Verdict;
}

/// Compares against one injected shared secret in constant time.
pub struct SharedSecretVerifier {
    expected: OwnerToken,
}

impl SharedSecretVerifier {
    /// Composes a verifier from material the embedding already resolved.
    ///
    /// # Errors
    /// Returns [`VerifierError::SecretTooShort`] below 32 bytes rather than serving a guessable
    /// owner credential.
    pub fn new(expected: OwnerToken) -> Result<Self, VerifierError> {
        if expected.len() < MIN_SHARED_SECRET_BYTES {
            return Err(VerifierError::SecretTooShort);
        }
        Ok(Self { expected })
    }
}

impl fmt::Debug for SharedSecretVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SharedSecretVerifier")
            .field("expected", &self.expected)
            .finish()
    }
}

impl OwnerVerifier for SharedSecretVerifier {
    fn verify(&self, presented: &OwnerToken) -> Verdict {
        if self.expected.matches(presented) {
            Verdict::Owner
        } else {
            Verdict::Rejected
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_TOKEN_BYTES, MIN_SHARED_SECRET_BYTES, OwnerToken, OwnerVerifier, SharedSecretVerifier,
        Verdict, constant_time_eq,
    };
    use crate::error::{TokenError, VerifierError};

    const SECRET: &str = "owner-token-0123456789abcdef0123456789abcdef";

    fn token(value: &str) -> OwnerToken {
        OwnerToken::new(value.as_bytes().to_vec()).unwrap()
    }

    #[test]
    fn every_token_error_has_material_that_provokes_it() {
        for expected in TokenError::ALL {
            let provocation = match expected {
                TokenError::Empty => Vec::new(),
                // Literal, not `MAX_TOKEN_BYTES + 1`: a bound compared with itself
                // says nothing about the bound.
                TokenError::TooLarge => vec![b'a'; 4097],
                TokenError::NotPrintableAscii => b"has space".to_vec(),
            };
            assert_eq!(OwnerToken::new(provocation).unwrap_err(), *expected);
            assert!(!expected.to_string().contains("has space"));
        }
        assert_eq!(
            OwnerToken::new(vec![0x7f]).unwrap_err(),
            TokenError::NotPrintableAscii
        );
        assert!(OwnerToken::new(vec![b'a'; 4096]).is_ok());
        assert_eq!(
            MAX_TOKEN_BYTES, 4096,
            "docs/gateway.md publishes this number"
        );
    }

    #[test]
    fn every_verifier_error_has_a_composition_that_provokes_it() {
        for expected in VerifierError::ALL {
            let observed = match expected {
                VerifierError::SecretTooShort => {
                    SharedSecretVerifier::new(token("short")).unwrap_err()
                }
            };
            assert_eq!(observed, *expected);
        }
    }

    #[test]
    fn the_shared_secret_bound_is_measured_at_the_byte_it_sits_on() {
        let of = |length: usize| OwnerToken::new(vec![b'a'; length]).unwrap();
        // Literals: writing `MIN_SHARED_SECRET_BYTES - 1` here would let the constant be
        // lowered to 8 with this case still green, which is what happened.
        assert_eq!(
            SharedSecretVerifier::new(of(31)).unwrap_err(),
            VerifierError::SecretTooShort,
            "31 bytes must be refused"
        );
        assert!(
            SharedSecretVerifier::new(of(32)).is_ok(),
            "32 bytes must be admitted"
        );
        assert_eq!(
            MIN_SHARED_SECRET_BYTES, 32,
            "docs/gateway.md publishes this number"
        );
        assert!(SharedSecretVerifier::new(token(SECRET)).is_ok());
    }

    #[test]
    fn only_the_exact_owner_secret_is_accepted() {
        let verifier = SharedSecretVerifier::new(token(SECRET)).unwrap();
        assert_eq!(verifier.verify(&token(SECRET)), Verdict::Owner);
        assert_eq!(
            verifier.verify(&token("owner-token-0123456789abcdef0123456789abcdeg")),
            Verdict::Rejected
        );
        assert_eq!(
            verifier.verify(&token("owner-token-0123456789abcdef0123456789abcde")),
            Verdict::Rejected
        );
        assert_eq!(
            verifier.verify(&token("owner-token-0123456789abcdef0123456789abcdeff")),
            Verdict::Rejected
        );
    }

    #[test]
    fn comparison_covers_length_and_content() {
        assert!(constant_time_eq(b"abcd", b"abcd"));
        assert!(!constant_time_eq(b"abcd", b"abce"));
        assert!(!constant_time_eq(b"abcd", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn proof_of_authentication_exists_only_for_the_owner() {
        assert!(Verdict::Owner.into_authenticated().is_some());
        assert!(Verdict::Rejected.into_authenticated().is_none());
    }
}
