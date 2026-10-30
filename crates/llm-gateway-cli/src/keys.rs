//! Each model's vLLM key (row B9), read once at startup through the trusted-file reader and held
//! redacted.

use crate::{
    config::Deployment,
    refusal::{Refusal, StartupRefusal},
    trusted,
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
/// whichever way the rules went. A vLLM key (row B9) and a hosted endpoint's credential are
/// read so.
pub(crate) fn read_token(
    path: &Path,
    key: &str,
    policy: &trusted::Policy,
    (too_large, not_a_token): (StartupRefusal, StartupRefusal),
) -> Result<Vec<u8>, Refusal> {
    let mut read = trusted::read_named(path, key, policy)?.into_bytes();
    let refuse = |kind, rule: &str| Refusal::new(kind, format!("{key}: {rule}"));
    // ASCII whitespace only: a trailing newline or CRLF, never a non-ASCII character.
    let end = read
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(0, |last| last + 1);
    let material = &read[..end];
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
    read.fill(0);
    std::hint::black_box(&read);
    held
}
