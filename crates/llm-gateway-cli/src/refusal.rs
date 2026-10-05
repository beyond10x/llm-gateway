//! Why startup is refused: `llm-gateway.deployment.StartupRefusal`.

use std::fmt;

/// Which file a trusted-file rule refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Config,
    OwnerSecret,
}

macro_rules! startup_refusals {
    ($($variant:ident => $code:literal,)+) => {
        /// The closed set of startup refusals, each with its stable `<source>:<rule>` code.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum StartupRefusal {
            $($variant,)+
        }

        impl StartupRefusal {
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

startup_refusals! {
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
    ListenBind => "listen:bind",
    SignalInstall => "signal:install",
}

/// The trusted-file rules, shared by both sources.
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
        }
    }
}

/// A refused start: the closed code and a one-line message. The message never quotes the
/// owner secret.
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
    use super::{FileRule, Source, StartupRefusal};
    use std::{collections::BTreeSet, fs, path::Path};

    /// The Rust set and the specification's set are the same list: a variant added on one side
    /// only fails here, whichever side it is.
    #[test]
    fn the_refusals_are_exactly_the_specified_variants() {
        let spec = Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("../../spec/domains/deployment.yaml");
        let text = fs::read_to_string(spec).unwrap();
        let declared = text
            .split("- name: llm-gateway.deployment.StartupRefusal")
            .nth(1)
            .and_then(|rest| rest.split("variants: [").nth(1))
            .and_then(|rest| rest.split(']').next())
            .expect("the specification declares StartupRefusal variants");
        let declared: Vec<&str> = declared.split(',').map(str::trim).collect();
        let implemented: Vec<&str> = StartupRefusal::ALL.iter().map(|r| r.name()).collect();
        assert_eq!(implemented, declared);
    }

    #[test]
    fn every_code_is_unique_and_names_its_source() {
        let codes: BTreeSet<&str> = StartupRefusal::ALL.iter().map(|r| r.code()).collect();
        assert_eq!(codes.len(), StartupRefusal::ALL.len());
        for refusal in StartupRefusal::ALL {
            let (source, rule) = refusal.code().split_once(':').unwrap();
            assert!(
                ["config", "owner-secret", "listen", "signal"].contains(&source),
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
