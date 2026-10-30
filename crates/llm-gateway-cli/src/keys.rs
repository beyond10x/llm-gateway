//! Each model's vLLM key (row B9), read once at startup through the trusted-file reader and held
//! redacted.

use crate::{
    config::Deployment,
    refusal::{Refusal, StartupRefusal},
    trusted::{self, Wiped},
};
use std::{collections::BTreeMap, fmt, path::Path};

/// The largest vLLM key, after trailing ASCII whitespace is trimmed: the owner token's bound.
const MAX_VLLM_KEY_BYTES: usize = 4096;

/// One model's vLLM key. It is redacted in `Debug`, has no `Display` and no `Clone`, and its
/// bytes are overwritten when it is dropped.
pub struct VllmKey(Vec<u8>);

impl VllmKey {
    /// The key as the pod's vLLM server expects it: one printable ASCII token.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for VllmKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VllmKey([REDACTED])")
    }
}

impl Drop for VllmKey {
    fn drop(&mut self) {
        // Best effort without a `zeroize` dependency, as `OwnerToken` does.
        self.0.fill(0);
        std::hint::black_box(&self.0);
    }
}

/// Every model's vLLM key, by alias. A model whose declaration names no `vllm_api_key_file` has
/// none. `Debug` prints the aliases and no value.
#[derive(Default)]
pub struct VllmKeys(BTreeMap<String, VllmKey>);

impl VllmKeys {
    pub fn get(&self, alias: &str) -> Option<&VllmKey> {
        self.0.get(alias)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for VllmKeys {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_map().entries(self.0.iter()).finish()
    }
}

/// Reads each model's `vllm_api_key_file` once through the trusted-file reader.
///
/// # Errors
/// A `vllm-api-key:*` [`Refusal`] for the first file, in alias order, that breaks a rule. Every
/// message starts `models.<alias>.vllm_api_key_file:` and quotes neither the path nor the bytes.
pub fn vllm_keys(deployment: &Deployment) -> Result<VllmKeys, Refusal> {
    let mut keys = BTreeMap::new();
    for (alias, model) in &deployment.models {
        let Some(path) = &model.vllm_api_key_file else {
            continue;
        };
        let material = read_token(
            path,
            &format!("models.{alias}.vllm_api_key_file"),
            &trusted::VLLM_API_KEY,
            (
                StartupRefusal::VllmApiKeyTooLarge,
                StartupRefusal::VllmApiKeyNotAToken,
            ),
        )?;
        keys.insert(alias.clone(), VllmKey(material));
    }
    Ok(VllmKeys(keys))
}

/// Reads one credential file through the trusted-file reader as `policy` admits it, and returns
/// its one printable ASCII token of at most 4096 bytes once trailing ASCII whitespace is
/// trimmed: refused as the `rules`' `(too_large, not_a_token)` otherwise. Every message starts
/// `<key>:` and quotes neither the path nor the bytes, and the bytes read are overwritten
/// whichever way the rules went: they stay in the reader's one allocation, taken at its bound,
/// until it is dropped. The token returned is a copy of exactly its length, which the caller
/// wraps. A vLLM key (row B9) and a hosted endpoint's credential are read so.
pub(crate) fn read_token(
    path: &Path,
    key: &str,
    policy: &trusted::Policy,
    (too_large, not_a_token): (StartupRefusal, StartupRefusal),
) -> Result<Vec<u8>, Refusal> {
    let read = Wiped::adopt(trusted::read_named(path, key, policy)?.into_bytes());
    let refuse = |kind, rule: &str| Refusal::new(kind, format!("{key}: {rule}"));
    // ASCII whitespace only: a trailing newline or CRLF, never a non-ASCII character.
    let material = read.as_slice().trim_ascii_end();
    let held = if material.len() > MAX_VLLM_KEY_BYTES {
        Err(refuse(
            too_large,
            &format!("the credential exceeds {MAX_VLLM_KEY_BYTES} bytes"),
        ))
    } else if material.is_empty() || !material.iter().all(u8::is_ascii_graphic) {
        Err(refuse(
            not_a_token,
            "the credential must be one non-empty printable ASCII token",
        ))
    } else {
        Ok(material.to_vec())
    };
    drop(read);
    held
}

#[cfg(test)]
mod tests {
    use super::read_token;
    use crate::{
        refusal::StartupRefusal,
        trusted::{self, ledger},
    };
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };

    /// `bytes` in a file of mode 0600, in a directory of its own under the build directory.
    fn file(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!(
                "gateway-cli-unit-key-{name}-{}",
                std::process::id()
            ));
        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("key");
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }

    /// What this proves: every allocation a key file's bytes were held in while `read_token`
    /// read them, the reader's included, read zero in full as it was let go, whether the rules
    /// took the key or refused it, and none was freed by a reallocation behind its back; the key
    /// it returns is a copy of exactly its length. That freed memory stays zero is not
    /// observable from safe Rust.
    #[test]
    fn a_key_file_read_is_overwritten_whichever_way_the_rules_go() {
        let mut long = vec![b'k'; 4097];
        long.push(b'\n');
        for (name, bytes, outcome) in [
            (
                "token",
                &b"sk-key-0123456789\r\n"[..],
                Ok(&b"sk-key-0123456789"[..]),
            ),
            ("spaced", b"sk key\n", Err("vllm-api-key:not-a-token")),
            ("blank", b" \n", Err("vllm-api-key:not-a-token")),
            ("long", &long, Err("vllm-api-key:too-large")),
        ] {
            let path = file(name, bytes);
            ledger::drain();
            let read = read_token(
                &path,
                "models.code.vllm_api_key_file",
                &trusted::VLLM_API_KEY,
                (
                    StartupRefusal::VllmApiKeyTooLarge,
                    StartupRefusal::VllmApiKeyNotAToken,
                ),
            );
            match (read, outcome) {
                (Ok(key), Ok(expected)) => {
                    assert_eq!(key, expected, "{name}");
                    assert_eq!(key.capacity(), key.len(), "{name}");
                }
                (Err(refusal), Err(code)) => assert_eq!(refusal.code(), code, "{name}"),
                (read, _) => panic!("{name}: {:?}", read.map(|_| "a key")),
            }
            ledger::assert_every_allocation_overwritten(name);
        }
    }
}
