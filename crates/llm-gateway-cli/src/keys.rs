//! Each model's vLLM key (row B9), read once at startup through the trusted-file reader and held
//! redacted.

use crate::{
    config::Deployment,
    refusal::{Refusal, StartupRefusal},
    trusted,
};
use std::{collections::BTreeMap, fmt};

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
/// message starts `models.<alias>.vllm_api_key_file <path>:` and never quotes the bytes.
pub fn vllm_keys(deployment: &Deployment) -> Result<VllmKeys, Refusal> {
    let mut keys = BTreeMap::new();
    for (alias, model) in &deployment.models {
        let Some(path) = &model.vllm_api_key_file else {
            continue;
        };
        let key = format!("models.{alias}.vllm_api_key_file");
        // The reader's own message is `<path>: <detail>`; the model goes in front of it.
        let mut read = trusted::read(path, &trusted::VLLM_API_KEY)
            .map_err(|refusal| {
                Refusal::new(refusal.kind(), format!("{key} {}", refusal.message()))
            })?
            .into_bytes();
        let refuse =
            |kind, rule: &str| Refusal::new(kind, format!("{key} {}: {rule}", path.display()));
        // ASCII whitespace only: a trailing newline or CRLF, never a non-ASCII character.
        let end = read
            .iter()
            .rposition(|byte| !byte.is_ascii_whitespace())
            .map_or(0, |last| last + 1);
        let material = &read[..end];
        let held = if material.len() > MAX_VLLM_KEY_BYTES {
            Err(refuse(
                StartupRefusal::VllmApiKeyTooLarge,
                &format!("the key exceeds {MAX_VLLM_KEY_BYTES} bytes"),
            ))
        } else if material.is_empty() || !material.iter().all(u8::is_ascii_graphic) {
            Err(refuse(
                StartupRefusal::VllmApiKeyNotAToken,
                "the key must be one non-empty printable ASCII token",
            ))
        } else {
            Ok(VllmKey(material.to_vec()))
        };
        // The bytes read are overwritten whichever way the rules went.
        read.fill(0);
        std::hint::black_box(&read);
        keys.insert(alias.clone(), held?);
    }
    Ok(VllmKeys(keys))
}
