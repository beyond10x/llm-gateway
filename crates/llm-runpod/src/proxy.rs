//! Where a running pod serves inference, when its embedding declares it (story:runpod-rest-host).
//!
//! By default a pod's endpoint is llm's Runpod description filled with the pod id. A deployment
//! may instead declare a proxy URL: `https://` and a host with an optional port, in which the
//! placeholders `{pod_id}` and `{port}` stand for the pod's id and the port vLLM serves on
//! ([`VLLM_PORT`]). The endpoint is that URL filled and followed by `/v1/`.

use std::fmt;

/// The port vLLM serves on inside every pod this crate starts.
pub const VLLM_PORT: u16 = 8000;

/// The longest proxy URL, in bytes.
pub const MAX_PROXY_URL_BYTES: usize = 512;

const POD_ID: &str = "{pod_id}";
const PORT: &str = "{port}";

/// A declared proxy URL: `https://` and a host with an optional port, whose only placeholders
/// are `{pod_id}` and `{port}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyUrl(String);

/// Why a proxy URL was refused. Its text names the rule and quotes nothing of the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyUrlError {
    /// Longer than [`MAX_PROXY_URL_BYTES`].
    Length,
    /// Not an `https://` URL.
    NotHttps,
    /// More than a host and an optional port: a path, a query, user information, a character
    /// no host has, or a port that is not a number from 1 to 65535.
    NotAHost,
    /// A brace that is not part of `{pod_id}` or `{port}`.
    Placeholder,
}

impl fmt::Display for ProxyUrlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Length => "is longer than 512 bytes",
            Self::NotHttps => "is not an https:// URL",
            Self::NotAHost => "is more than https://, a host and an optional port",
            Self::Placeholder => "holds a placeholder other than {pod_id} and {port}",
        })
    }
}

impl std::error::Error for ProxyUrlError {}

impl ProxyUrl {
    /// The proxy URL `value`, when it keeps the rule.
    ///
    /// # Errors
    /// The first rule `value` breaks, in the order of [`ProxyUrlError`]'s variants.
    pub fn new(value: &str) -> Result<Self, ProxyUrlError> {
        if value.len() > MAX_PROXY_URL_BYTES {
            return Err(ProxyUrlError::Length);
        }
        let authority = value
            .strip_prefix("https://")
            .ok_or(ProxyUrlError::NotHttps)?;
        let filled = authority.replace(POD_ID, "p").replace(PORT, "8000");
        if filled.contains(['{', '}']) {
            return Err(ProxyUrlError::Placeholder);
        }
        let (host, port) = filled
            .split_once(':')
            .map_or((filled.as_str(), None), |(host, port)| (host, Some(port)));
        let host_ok = !host.is_empty()
            && host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'));
        let port_ok = port.is_none_or(|port| {
            port.bytes().all(|byte| byte.is_ascii_digit())
                && port.parse::<u16>().is_ok_and(|port| port > 0)
        });
        if !host_ok || !port_ok {
            return Err(ProxyUrlError::NotAHost);
        }
        Ok(Self(value.to_owned()))
    }

    /// The URL as declared, placeholders unfilled.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The endpoint of the pod `pod_id`: the URL with `{pod_id}` and `{port}` filled, then
    /// `/v1/`. The id is filled as given; the provider fills only an id llm's Runpod description
    /// accepts.
    pub fn endpoint(&self, pod_id: &str) -> String {
        let filled = self
            .0
            .replace(POD_ID, pod_id)
            .replace(PORT, &VLLM_PORT.to_string());
        format!("{filled}/v1/")
    }
}

impl fmt::Display for ProxyUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
