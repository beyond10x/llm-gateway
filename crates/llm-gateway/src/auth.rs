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

    /// The SHA-256 digest of the material: always 32 bytes, whatever its length.
    pub(crate) fn digest(&self) -> [u8; 32] {
        sha256(&self.0)
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

/// Compares in time independent of where the first differing byte is. Over inputs of unequal
/// length the loop runs over the shorter one, so the owner comparison never calls it on raw
/// secrets: [`SharedSecretVerifier`] passes two SHA-256 digests, which are always 32 bytes.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = u64::from(left.len() != right.len());
    for (a, b) in left.iter().zip(right.iter()) {
        difference |= u64::from(a ^ b);
    }
    std::hint::black_box(difference) == 0
}

const SHA256_K: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

/// SHA-256 (FIPS 180-4), written here because the gateway links no crate at all
/// (`tests/dependency_boundary.rs`). It hashes owner credentials for comparison only; its
/// output never leaves this module.
// The working variables keep FIPS 180-4's own names, a to h, so the code reads against the standard.
#[allow(clippy::many_single_char_names)]
fn sha256(message: &[u8]) -> [u8; 32] {
    let mut state: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];
    let bit_length = (message.len() as u64).wrapping_mul(8);
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_length.to_be_bytes());
    for block in padded.as_chunks::<64>().0 {
        let mut schedule = [0_u32; 64];
        for (word, bytes) in schedule.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*bytes);
        }
        for index in 16..64 {
            let early = schedule[index - 15];
            let late = schedule[index - 2];
            let sigma0 = early.rotate_right(7) ^ early.rotate_right(18) ^ (early >> 3);
            let sigma1 = late.rotate_right(17) ^ late.rotate_right(19) ^ (late >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(sigma0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(sigma1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for (constant, word) in SHA256_K.iter().zip(schedule) {
            let sum1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ (!e & g);
            let first = h
                .wrapping_add(sum1)
                .wrapping_add(choice)
                .wrapping_add(*constant)
                .wrapping_add(word);
            let sum0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let second = sum0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(first);
            d = c;
            c = b;
            b = a;
            a = first.wrapping_add(second);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
    padded.fill(0);
    std::hint::black_box(&padded);
    let mut digest = [0_u8; 32];
    for (bytes, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        *bytes = word.to_be_bytes();
    }
    digest
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
///
/// Only the secret's SHA-256 digest is kept, computed once at composition. Each verification
/// hashes the presented value and compares two 32-byte digests in constant time, so the work
/// depends on neither the content nor the length of the expected secret. Hashing the presented
/// value takes time that depends only on its own length, which its sender already knows.
pub struct SharedSecretVerifier {
    expected: [u8; 32],
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
        let digest = expected.digest();
        // The material itself is not kept: dropping it overwrites its bytes.
        drop(expected);
        Ok(Self { expected: digest })
    }
}

impl fmt::Debug for SharedSecretVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SharedSecretVerifier")
            .field("expected", &"[REDACTED]")
            .finish()
    }
}

impl OwnerVerifier for SharedSecretVerifier {
    fn verify(&self, presented: &OwnerToken) -> Verdict {
        if constant_time_eq(&self.expected, &presented.digest()) {
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
        Verdict, constant_time_eq, sha256,
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

    fn hex(digest: [u8; 32]) -> String {
        use std::fmt::Write;
        digest.iter().fold(String::new(), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
    }

    /// FIPS 180-4 examples, plus the padding edges: 55 bytes fits one block, 56 needs two.
    #[test]
    fn the_digest_is_sha_256() {
        let cases: [(&[u8], &str); 5] = [
            (
                b"",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                b"abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
            (
                &[b'a'; 55],
                "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318",
            ),
            (
                &[b'a'; 56],
                "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(hex(sha256(input)), expected, "{} bytes", input.len());
        }
        let million = vec![b'a'; 1_000_000];
        assert_eq!(
            hex(sha256(&million)),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    /// The owner comparison runs over two digests of one fixed length, whatever the length of
    /// either secret, so neither length decides where the comparison stops.
    #[test]
    fn the_owner_comparison_covers_secrets_of_every_length() {
        let owner = SharedSecretVerifier::new(token(SECRET)).unwrap();
        assert_eq!(owner.verify(&token(SECRET)), Verdict::Owner);
        for other in [
            "o",
            "owner-token-0123456789abcdef0123456789abcde",
            &"a".repeat(4096),
        ] {
            assert_eq!(
                owner.verify(&token(other)),
                Verdict::Rejected,
                "{} bytes",
                other.len()
            );
        }
    }

    #[test]
    fn proof_of_authentication_exists_only_for_the_owner() {
        assert!(Verdict::Owner.into_authenticated().is_some());
        assert!(Verdict::Rejected.into_authenticated().is_none());
    }
}
