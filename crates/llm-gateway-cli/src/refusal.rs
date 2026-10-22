//! Why startup is refused, `llm-gateway.deployment.StartupRefusal`, and why `credential set`
//! is, `llm-gateway.deployment.CredentialRefusal`.

use std::fmt;

/// Which file a trusted-file rule refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Config,
    OwnerSecret,
    /// A file a model's `vllm_api_key_file` names.
    VllmApiKey,
    /// A file a hosted model's `credential_file` names.
    EndpointCredential,
    /// The file `metrics_bearer_file` names.
    MetricsBearer,
    /// The container authorization token file `AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE` names,
    /// for a Bedrock model.
    ContainerToken,
}

macro_rules! refusals {
    ($(#[$doc:meta])* $name:ident { $($variant:ident => $code:literal,)+ }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $($variant,)+
        }

        impl $name {
            /// Every refusal, in the specification's order.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            /// The variant's name as the specification spells it.
            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => stringify!($variant),)+
                }
            }

            /// The code written after `refused` on standard error.
            pub const fn code(self) -> &'static str {
                match self {
                    $(Self::$variant => $code,)+
                }
            }
        }
    };
}

refusals! {
    /// The closed set of startup refusals, each with its stable `<source>:<rule>` code.
    StartupRefusal {
    ConfigUnreadable => "config:unreadable",
    ConfigSymlink => "config:symlink",
    ConfigNotRegular => "config:not-regular",
    ConfigUntrustedOwner => "config:untrusted-owner",
    ConfigUnsafeMode => "config:unsafe-mode",
    ConfigTooLarge => "config:too-large",
    ConfigNotUtf8 => "config:not-utf8",
    ConfigSchema => "config:schema",
    ConfigValue => "config:value",
    OwnerSecretUnreadable => "owner-secret:unreadable",
    OwnerSecretSymlink => "owner-secret:symlink",
    OwnerSecretNotRegular => "owner-secret:not-regular",
    OwnerSecretUntrustedOwner => "owner-secret:untrusted-owner",
    OwnerSecretUnsafeMode => "owner-secret:unsafe-mode",
    OwnerSecretTooLarge => "owner-secret:too-large",
    OwnerSecretNotUtf8 => "owner-secret:not-utf8",
    OwnerSecretNotAToken => "owner-secret:not-a-token",
    OwnerSecretTooShort => "owner-secret:too-short",
    VllmApiKeyUnreadable => "vllm-api-key:unreadable",
    VllmApiKeySymlink => "vllm-api-key:symlink",
    VllmApiKeyNotRegular => "vllm-api-key:not-regular",
    VllmApiKeyUntrustedOwner => "vllm-api-key:untrusted-owner",
    VllmApiKeyUnsafeMode => "vllm-api-key:unsafe-mode",
    VllmApiKeyTooLarge => "vllm-api-key:too-large",
    VllmApiKeyNotUtf8 => "vllm-api-key:not-utf8",
    VllmApiKeyNotAToken => "vllm-api-key:not-a-token",
    EndpointCredentialUnreadable => "endpoint-credential:unreadable",
    EndpointCredentialSymlink => "endpoint-credential:symlink",
    EndpointCredentialNotRegular => "endpoint-credential:not-regular",
    EndpointCredentialUntrustedOwner => "endpoint-credential:untrusted-owner",
    EndpointCredentialUnsafeMode => "endpoint-credential:unsafe-mode",
    EndpointCredentialTooLarge => "endpoint-credential:too-large",
    EndpointCredentialNotUtf8 => "endpoint-credential:not-utf8",
    EndpointCredentialNotAToken => "endpoint-credential:not-a-token",
    MetricsBearerUnreadable => "metrics-bearer:unreadable",
    MetricsBearerSymlink => "metrics-bearer:symlink",
    MetricsBearerNotRegular => "metrics-bearer:not-regular",
    MetricsBearerUntrustedOwner => "metrics-bearer:untrusted-owner",
    MetricsBearerUnsafeMode => "metrics-bearer:unsafe-mode",
    MetricsBearerTooLarge => "metrics-bearer:too-large",
    MetricsBearerNotUtf8 => "metrics-bearer:not-utf8",
    MetricsBearerNotAToken => "metrics-bearer:not-a-token",
    ContainerCredentialsUnset => "container-credentials:unset",
    ContainerCredentialsUri => "container-credentials:uri",
    ContainerTokenUnreadable => "container-token:unreadable",
    ContainerTokenSymlink => "container-token:symlink",
    ContainerTokenNotRegular => "container-token:not-regular",
    ContainerTokenUntrustedOwner => "container-token:untrusted-owner",
    ContainerTokenUnsafeMode => "container-token:unsafe-mode",
    ContainerTokenTooLarge => "container-token:too-large",
    ContainerTokenNotUtf8 => "container-token:not-utf8",
    ContainerTokenNotAToken => "container-token:not-a-token",
    ListenBind => "listen:bind",
    SignalInstall => "signal:install",
    }
}

refusals! {
    /// The closed set of `credential set` refusals, each with its stable `credential:<rule>`
    /// code. The command also refuses with the [`StartupRefusal`] the server's own loader or
    /// reader gives (`config:*`, `endpoint-credential:*`).
    CredentialRefusal {
        UnknownTarget => "credential:unknown-target",
        NotHosted => "credential:not-hosted",
        Anonymous => "credential:anonymous",
        NoTerminal => "credential:no-terminal",
        InputCancelled => "credential:input-cancelled",
        InputUnreadable => "credential:input-unreadable",
        InputEmpty => "credential:input-empty",
        InputTooLarge => "credential:input-too-large",
        InputNotAToken => "credential:input-not-a-token",
        ProbeRejected => "credential:probe-rejected",
        ProbeUnreachable => "credential:probe-unreachable",
        DirectoryMissing => "credential:directory-missing",
        DirectoryUnsafe => "credential:directory-unsafe",
        Write => "credential:write",
    }
}

/// The trusted-file rules, shared by every source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileRule {
    Unreadable,
    Symlink,
    NotRegular,
    UntrustedOwner,
    UnsafeMode,
    TooLarge,
    NotUtf8,
}

impl FileRule {
    pub(crate) const fn refusal(self, source: Source) -> StartupRefusal {
        match (source, self) {
            (Source::Config, Self::Unreadable) => StartupRefusal::ConfigUnreadable,
            (Source::Config, Self::Symlink) => StartupRefusal::ConfigSymlink,
            (Source::Config, Self::NotRegular) => StartupRefusal::ConfigNotRegular,
            (Source::Config, Self::UntrustedOwner) => StartupRefusal::ConfigUntrustedOwner,
            (Source::Config, Self::UnsafeMode) => StartupRefusal::ConfigUnsafeMode,
            (Source::Config, Self::TooLarge) => StartupRefusal::ConfigTooLarge,
            (Source::Config, Self::NotUtf8) => StartupRefusal::ConfigNotUtf8,
            (Source::OwnerSecret, Self::Unreadable) => StartupRefusal::OwnerSecretUnreadable,
            (Source::OwnerSecret, Self::Symlink) => StartupRefusal::OwnerSecretSymlink,
            (Source::OwnerSecret, Self::NotRegular) => StartupRefusal::OwnerSecretNotRegular,
            (Source::OwnerSecret, Self::UntrustedOwner) => {
                StartupRefusal::OwnerSecretUntrustedOwner
            }
            (Source::OwnerSecret, Self::UnsafeMode) => StartupRefusal::OwnerSecretUnsafeMode,
            (Source::OwnerSecret, Self::TooLarge) => StartupRefusal::OwnerSecretTooLarge,
            (Source::OwnerSecret, Self::NotUtf8) => StartupRefusal::OwnerSecretNotUtf8,
            (Source::VllmApiKey, Self::Unreadable) => StartupRefusal::VllmApiKeyUnreadable,
            (Source::VllmApiKey, Self::Symlink) => StartupRefusal::VllmApiKeySymlink,
            (Source::VllmApiKey, Self::NotRegular) => StartupRefusal::VllmApiKeyNotRegular,
            (Source::VllmApiKey, Self::UntrustedOwner) => StartupRefusal::VllmApiKeyUntrustedOwner,
            (Source::VllmApiKey, Self::UnsafeMode) => StartupRefusal::VllmApiKeyUnsafeMode,
            (Source::VllmApiKey, Self::TooLarge) => StartupRefusal::VllmApiKeyTooLarge,
            (Source::VllmApiKey, Self::NotUtf8) => StartupRefusal::VllmApiKeyNotUtf8,
            (Source::EndpointCredential, Self::Unreadable) => {
                StartupRefusal::EndpointCredentialUnreadable
            }
            (Source::EndpointCredential, Self::Symlink) => {
                StartupRefusal::EndpointCredentialSymlink
            }
            (Source::EndpointCredential, Self::NotRegular) => {
                StartupRefusal::EndpointCredentialNotRegular
            }
            (Source::EndpointCredential, Self::UntrustedOwner) => {
                StartupRefusal::EndpointCredentialUntrustedOwner
            }
            (Source::EndpointCredential, Self::UnsafeMode) => {
                StartupRefusal::EndpointCredentialUnsafeMode
            }
            (Source::EndpointCredential, Self::TooLarge) => {
                StartupRefusal::EndpointCredentialTooLarge
            }
            (Source::EndpointCredential, Self::NotUtf8) => {
                StartupRefusal::EndpointCredentialNotUtf8
            }
            (Source::MetricsBearer, Self::Unreadable) => StartupRefusal::MetricsBearerUnreadable,
            (Source::MetricsBearer, Self::Symlink) => StartupRefusal::MetricsBearerSymlink,
            (Source::MetricsBearer, Self::NotRegular) => StartupRefusal::MetricsBearerNotRegular,
            (Source::MetricsBearer, Self::UntrustedOwner) => {
                StartupRefusal::MetricsBearerUntrustedOwner
            }
            (Source::MetricsBearer, Self::UnsafeMode) => StartupRefusal::MetricsBearerUnsafeMode,
            (Source::MetricsBearer, Self::TooLarge) => StartupRefusal::MetricsBearerTooLarge,
            (Source::MetricsBearer, Self::NotUtf8) => StartupRefusal::MetricsBearerNotUtf8,
            (Source::ContainerToken, Self::Unreadable) => StartupRefusal::ContainerTokenUnreadable,
            (Source::ContainerToken, Self::Symlink) => StartupRefusal::ContainerTokenSymlink,
            (Source::ContainerToken, Self::NotRegular) => StartupRefusal::ContainerTokenNotRegular,
            (Source::ContainerToken, Self::UntrustedOwner) => {
                StartupRefusal::ContainerTokenUntrustedOwner
            }
            (Source::ContainerToken, Self::UnsafeMode) => StartupRefusal::ContainerTokenUnsafeMode,
            (Source::ContainerToken, Self::TooLarge) => StartupRefusal::ContainerTokenTooLarge,
            (Source::ContainerToken, Self::NotUtf8) => StartupRefusal::ContainerTokenNotUtf8,
        }
    }
}

/// A refused start: the closed code and a one-line message. The message never quotes the
/// owner secret or a vLLM key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    kind: StartupRefusal,
    message: String,
}

impl Refusal {
    pub(crate) fn new(kind: StartupRefusal, message: impl Into<String>) -> Self {
        // One line on standard error, whatever the message's source wrote.
        let message = message.into().replace(['\n', '\r'], " ");
        Self { kind, message }
    }

    pub fn kind(&self) -> StartupRefusal {
        self.kind
    }

    pub fn code(&self) -> &'static str {
        self.kind.code()
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code(), self.message)
    }
}

impl std::error::Error for Refusal {}

#[cfg(test)]
mod tests {
    use super::{CredentialRefusal, FileRule, Source, StartupRefusal};
    use std::{collections::BTreeSet, fs, path::Path};

    /// The variants `deployment.yaml` declares for the enum `name`.
    fn declared(name: &str) -> Vec<String> {
        let spec = Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("../../spec/domains/deployment.yaml");
        let text = fs::read_to_string(spec).unwrap();
        let declared = text
            .split(&format!("- name: llm-gateway.deployment.{name}\n"))
            .nth(1)
            .and_then(|rest| rest.split("variants: [").nth(1))
            .and_then(|rest| rest.split(']').next())
            .expect("the specification declares the variants");
        declared.split(',').map(|v| v.trim().to_owned()).collect()
    }

    /// The Rust set and the specification's set are the same list: a variant added on one side
    /// only fails here, whichever side it is.
    #[test]
    fn the_refusals_are_exactly_the_specified_variants() {
        let implemented: Vec<&str> = StartupRefusal::ALL.iter().map(|r| r.name()).collect();
        assert_eq!(implemented, declared("StartupRefusal"));
        let implemented: Vec<&str> = CredentialRefusal::ALL.iter().map(|r| r.name()).collect();
        assert_eq!(implemented, declared("CredentialRefusal"));
    }

    #[test]
    fn every_credential_code_is_unique_and_a_credential_rule() {
        let codes: BTreeSet<&str> = CredentialRefusal::ALL.iter().map(|r| r.code()).collect();
        assert_eq!(codes.len(), CredentialRefusal::ALL.len());
        let startup: BTreeSet<&str> = StartupRefusal::ALL.iter().map(|r| r.code()).collect();
        for code in codes {
            let rule = code.strip_prefix("credential:").expect(code);
            assert!(!rule.is_empty() && !startup.contains(code), "{code}");
        }
    }

    #[test]
    fn every_code_is_unique_and_names_its_source() {
        let codes: BTreeSet<&str> = StartupRefusal::ALL.iter().map(|r| r.code()).collect();
        assert_eq!(codes.len(), StartupRefusal::ALL.len());
        for refusal in StartupRefusal::ALL {
            let (source, rule) = refusal.code().split_once(':').unwrap();
            assert!(
                [
                    "config",
                    "owner-secret",
                    "vllm-api-key",
                    "endpoint-credential",
                    "metrics-bearer",
                    "container-credentials",
                    "container-token",
                    "listen",
                    "signal"
                ]
                .contains(&source),
                "{refusal:?}"
            );
            assert!(!rule.is_empty(), "{refusal:?}");
        }
    }

    #[test]
    fn every_file_rule_maps_into_its_own_source() {
        let rules = [
            FileRule::Unreadable,
            FileRule::Symlink,
            FileRule::NotRegular,
            FileRule::UntrustedOwner,
            FileRule::UnsafeMode,
            FileRule::TooLarge,
            FileRule::NotUtf8,
        ];
        for (source, prefix) in [
            (Source::Config, "config:"),
            (Source::OwnerSecret, "owner-secret:"),
            (Source::VllmApiKey, "vllm-api-key:"),
            (Source::EndpointCredential, "endpoint-credential:"),
            (Source::MetricsBearer, "metrics-bearer:"),
            (Source::ContainerToken, "container-token:"),
        ] {
            let codes: BTreeSet<&str> = rules.iter().map(|r| r.refusal(source).code()).collect();
            assert_eq!(codes.len(), rules.len());
            assert!(
                codes.iter().all(|code| code.starts_with(prefix)),
                "{codes:?}"
            );
        }
    }
}
