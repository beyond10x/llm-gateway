//! `keychain:<service>/<account>` references: a credential held in the operator's OS keychain,
//! read once at start (story:keychain-references).
//!
//! A local gateway, run by one operator on their own machine, may name a credential by a
//! keychain entry instead of a trusted file: the Runpod API key (`[providers.<name>] api_key`),
//! a hosted endpoint's credential (`[hosted.<alias>] credential`), a Runpod model's vLLM key
//! (`[models.<alias>] vllm_api_key`) and a routes document endpoint's `auth.secret`. The entry is
//! looked up through the platform's own tool, the Secret Service's `secret-tool` on Linux and
//! `security` on macOS, by its `service` and `account` attributes. Only those two names reach
//! the tool's argument list; the value is read from its standard output into an allocation
//! that is overwritten when dropped, and is never written to a log line, an error or `Debug`.
//! A gateway in a cluster names files: it never assumes a desktop keychain.

use crate::{
    keys::token_of,
    refusal::{Refusal, StartupRefusal},
    trusted::Wiped,
};
use std::{
    fmt,
    io::Read as _,
    process::{Command, Stdio},
    sync::{Arc, OnceLock},
};

/// The prefix of a keychain reference.
pub const KEYCHAIN_PREFIX: &str = "keychain:";

/// The longest service or account name.
pub const MAX_KEYCHAIN_NAME_BYTES: usize = 256;

/// The most of a tool's answer read: a token's 4096 bytes and a line ending, with room to tell
/// a longer one apart.
const MAX_ANSWER_BYTES: usize = 4096 + 64;

/// One keychain entry, named by its service and account. Both are operator metadata, not
/// secrets: each is 1 to 256 printable ASCII bytes without a space, and the service holds no
/// `/`. `Debug` and `Display` print the reference as written.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct KeychainRef {
    service: String,
    account: String,
}

impl KeychainRef {
    /// `keychain:<service>/<account>`, split at the first `/`; `None` for any other text.
    pub fn parse(text: &str) -> Option<Self> {
        let (service, account) = text.strip_prefix(KEYCHAIN_PREFIX)?.split_once('/')?;
        let name = |name: &str| {
            !name.is_empty()
                && name.len() <= MAX_KEYCHAIN_NAME_BYTES
                && name.bytes().all(|byte| byte.is_ascii_graphic())
        };
        (name(service) && name(account)).then(|| Self {
            service: service.to_owned(),
            account: account.to_owned(),
        })
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    pub fn account(&self) -> &str {
        &self.account
    }
}

impl fmt::Display for KeychainRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{KEYCHAIN_PREFIX}{}/{}",
            self.service, self.account
        )
    }
}

impl fmt::Debug for KeychainRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

/// Why an entry was not read. Neither carries the tool's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupFailure {
    /// The keychain holds no such entry.
    Missing,
    /// The keychain could not be asked: no tool, a locked or absent keychain, a refused prompt.
    Unavailable,
}

/// An entry's bytes as a [`SecretSource`] read them: overwritten when dropped, redacted in
/// `Debug`, no `Display` and no `Clone`.
pub struct EntryValue(Wiped);

impl EntryValue {
    /// `bytes`, overwritten from now on when dropped.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Wiped::adopt(bytes))
    }
}

impl fmt::Debug for EntryValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EntryValue([REDACTED])")
    }
}

/// Where keychain entries are read from. The shipped binary reads [`OsKeychain`]; a test
/// installs its own source with [`install_secret_source`] and reads no real keychain.
pub trait SecretSource: Send + Sync {
    /// The entry's bytes as stored, a trailing line ending included when the tool writes one,
    /// in an allocation overwritten when dropped.
    ///
    /// # Errors
    /// [`LookupFailure`].
    fn lookup(&self, reference: &KeychainRef) -> Result<EntryValue, LookupFailure>;
}

/// The platform's keychain through its own command-line tool: `secret-tool lookup service <s>
/// account <a>` on Linux (libsecret, any Secret Service such as GNOME Keyring or the KDE wallet), and
/// `security find-generic-password -s <s> -a <a> -w` on macOS.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsKeychain;

impl OsKeychain {
    /// The tool and its arguments for `reference`: names only, never a value.
    pub fn command(reference: &KeychainRef) -> (&'static str, Vec<String>) {
        let (service, account) = (reference.service.clone(), reference.account.clone());
        if cfg!(target_os = "macos") {
            (
                "security",
                vec![
                    "find-generic-password".to_owned(),
                    "-s".to_owned(),
                    service,
                    "-a".to_owned(),
                    account,
                    "-w".to_owned(),
                ],
            )
        } else {
            (
                "secret-tool",
                vec![
                    "lookup".to_owned(),
                    "service".to_owned(),
                    service,
                    "account".to_owned(),
                    account,
                ],
            )
        }
    }
}

impl SecretSource for OsKeychain {
    fn lookup(&self, reference: &KeychainRef) -> Result<EntryValue, LookupFailure> {
        let (program, arguments) = Self::command(reference);
        let mut child = Command::new(program)
            .args(&arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| LookupFailure::Unavailable)?;
        let mut answer = Wiped::with_capacity(MAX_ANSWER_BYTES);
        let mut chunk = [0_u8; 512];
        let mut overflow = false;
        if let Some(mut stdout) = child.stdout.take() {
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if answer.as_slice().len() + read > MAX_ANSWER_BYTES {
                            overflow = true;
                        } else {
                            answer.extend(&chunk[..read]);
                        }
                    }
                }
            }
        }
        chunk.fill(0);
        std::hint::black_box(&chunk);
        let status = child.wait().map_err(|_| LookupFailure::Unavailable)?;
        if overflow {
            // Longer than any token: refused as too large by the token rules.
            let mut long = Wiped::with_capacity(MAX_ANSWER_BYTES + 1);
            long.extend(&[b'x'; MAX_ANSWER_BYTES + 1]);
            return Ok(EntryValue(long));
        }
        if status.success() {
            return Ok(EntryValue(answer));
        }
        // `secret-tool lookup` exits 1 with no output for an entry it does not hold;
        // `security` exits 44 (errSecItemNotFound).
        match status.code() {
            Some(1) if !cfg!(target_os = "macos") && answer.as_slice().is_empty() => {
                Err(LookupFailure::Missing)
            }
            Some(44) if cfg!(target_os = "macos") => Err(LookupFailure::Missing),
            _ => Err(LookupFailure::Unavailable),
        }
    }
}

static SOURCE: OnceLock<Arc<dyn SecretSource>> = OnceLock::new();

/// Makes `source` the process's keychain in place of [`OsKeychain`], once, before the first
/// lookup: `false` when a source is already in place. For tests, which must read no real
/// keychain; the shipped binary never calls it.
#[doc(hidden)]
pub fn install_secret_source(source: Arc<dyn SecretSource>) -> bool {
    SOURCE.set(source).is_ok()
}

fn source() -> &'static Arc<dyn SecretSource> {
    SOURCE.get_or_init(|| Arc::new(OsKeychain))
}

/// The token the entry `reference` holds, taken as a credential file's is ([`token_of`]):
/// refused as `rules`' `(unreadable, too_large, not_a_token)`. Every message starts `<key>:`
/// and quotes neither the value nor the reference.
///
/// # Errors
/// The `unreadable` refusal for a missing entry or an unavailable keychain, else the token
/// rules'.
pub(crate) fn read_keychain_token(
    reference: &KeychainRef,
    key: &str,
    (unreadable, too_large, not_a_token): (StartupRefusal, StartupRefusal, StartupRefusal),
) -> Result<Vec<u8>, Refusal> {
    read_from(
        source().as_ref(),
        reference,
        key,
        (unreadable, too_large, not_a_token),
    )
}

fn read_from(
    source: &dyn SecretSource,
    reference: &KeychainRef,
    key: &str,
    (unreadable, too_large, not_a_token): (StartupRefusal, StartupRefusal, StartupRefusal),
) -> Result<Vec<u8>, Refusal> {
    let read = source.lookup(reference).map_err(|failure| {
        let rule = match failure {
            LookupFailure::Missing => "the keychain holds no entry for this service and account",
            LookupFailure::Unavailable => {
                "the keychain could not be read: no secret-tool or security command, a locked \
                 keychain, or a refused prompt"
            }
        };
        Refusal::new(unreadable, format!("{key}: {rule}"))
    })?;
    token_of(read.0, key, (too_large, not_a_token))
}

#[cfg(test)]
mod tests {
    use super::{EntryValue, KeychainRef, LookupFailure, OsKeychain, SecretSource, read_from};
    use crate::{
        refusal::StartupRefusal,
        trusted::{Wiped, ledger},
    };

    const RULES: (StartupRefusal, StartupRefusal, StartupRefusal) = (
        StartupRefusal::EndpointCredentialUnreadable,
        StartupRefusal::EndpointCredentialTooLarge,
        StartupRefusal::EndpointCredentialNotAToken,
    );

    /// A keychain of fixed entries; reads no real one.
    struct Fake(Vec<(&'static str, Result<&'static [u8], LookupFailure>)>);

    impl SecretSource for Fake {
        fn lookup(&self, reference: &KeychainRef) -> Result<EntryValue, LookupFailure> {
            let (_, entry) = self
                .0
                .iter()
                .find(|(name, _)| *name == reference.to_string())
                .ok_or(LookupFailure::Missing)?;
            let bytes = (*entry)?;
            let mut held = Wiped::with_capacity(bytes.len());
            held.extend(bytes);
            Ok(EntryValue(held))
        }
    }

    #[test]
    fn a_reference_is_a_service_and_an_account_split_at_the_first_slash() {
        let reference = KeychainRef::parse("keychain:models/pod:abc123").unwrap();
        assert_eq!(
            (reference.service(), reference.account()),
            ("models", "pod:abc123")
        );
        assert_eq!(reference.to_string(), "keychain:models/pod:abc123");
        assert_eq!(
            format!("{reference:?}"),
            "keychain:models/pod:abc123",
            "a reference names an entry, never a value"
        );
        let nested = KeychainRef::parse("keychain:svc/a/b").unwrap();
        assert_eq!(nested.account(), "a/b");
        let long = format!("keychain:s/{}", "a".repeat(257));
        for refused in [
            "keychain:",
            "keychain:svc",
            "keychain:/acct",
            "keychain:svc/",
            "keychain:s v/acct",
            "keychain:svc/a\tb",
            "file:~/key",
            "svc/acct",
            long.as_str(),
        ] {
            assert_eq!(KeychainRef::parse(refused), None, "{refused:?}");
        }
    }

    #[test]
    fn the_os_command_carries_the_two_names_and_nothing_else() {
        let reference = KeychainRef::parse("keychain:llm-gateway/runpod").unwrap();
        let (program, arguments) = OsKeychain::command(&reference);
        if cfg!(target_os = "macos") {
            assert_eq!(program, "security");
            assert_eq!(
                arguments,
                [
                    "find-generic-password",
                    "-s",
                    "llm-gateway",
                    "-a",
                    "runpod",
                    "-w"
                ]
            );
        } else {
            assert_eq!(program, "secret-tool");
            assert_eq!(
                arguments,
                ["lookup", "service", "llm-gateway", "account", "runpod"]
            );
        }
    }

    /// What this proves: an entry is taken as a credential file is, its trailing line ending
    /// trimmed; a missing or unreadable entry and a value breaking the token rules are refused
    /// with codes and messages that name the key and quote neither the value nor the
    /// reference; and every allocation the value was held in read zero as it was let go.
    #[test]
    fn an_entry_is_taken_as_a_token_and_refusals_quote_nothing() {
        let fake = Fake(vec![
            ("keychain:svc/good", Ok(b"pod-key-0123456789\n")),
            ("keychain:svc/spaced", Ok(b"pod key secret\n")),
            ("keychain:svc/locked", Err(LookupFailure::Unavailable)),
        ]);
        for (account, outcome) in [
            ("good", Ok(&b"pod-key-0123456789"[..])),
            ("spaced", Err("endpoint-credential:not-a-token")),
            ("locked", Err("endpoint-credential:unreadable")),
            ("absent", Err("endpoint-credential:unreadable")),
        ] {
            let reference = KeychainRef::parse(&format!("keychain:svc/{account}")).unwrap();
            ledger::drain();
            let read = read_from(&fake, &reference, "hosted.pod.credential", RULES);
            match (read, outcome) {
                (Ok(key), Ok(expected)) => assert_eq!(key, expected, "{account}"),
                (Err(refusal), Err(code)) => {
                    assert_eq!(refusal.code(), code, "{account}");
                    let text = refusal.to_string();
                    assert!(text.contains("hosted.pod.credential:"), "{text}");
                    for leak in ["pod key", "pod-key", "svc/"] {
                        assert!(!text.contains(leak), "{account}: {text}");
                    }
                }
                (read, _) => panic!("{account}: {:?}", read.map(|_| "a key")),
            }
            if matches!(account, "good" | "spaced") {
                ledger::assert_every_allocation_overwritten(account);
            }
        }
    }
}
