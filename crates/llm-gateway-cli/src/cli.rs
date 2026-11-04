//! The command lines of `b10x-llm-gateway`. They live in the library so that the documentation
//! site's CLI reference is generated from these definitions.
//!
//! The serving command line, [`Cli`], is closed: `--config <CONFIG>`, help and version. Three
//! other command lines are parsed instead when their name is the first argument
//! ([`Invocation::from_args`]): `serve` ([`ServeCli`]), which reads the default document
//! ([`default_config_path`]) unless `--config` names another, `watchdog` ([`WatchdogCli`]), and
//! `credential set` ([`CredentialCli`]). Every serving command line of earlier releases is
//! exactly what it was. A usage error names its kind and never echoes an argument, which may be a
//! key.

use std::{ffi::OsString, path::PathBuf};

use clap::{ArgGroup, Args, Parser, Subcommand};

/// The authenticated single-owner llm-gateway, configured by one closed TOML document.
#[derive(Debug, Parser)]
#[command(
    name = "b10x-llm-gateway",
    version,
    after_help = "To serve the default document ($XDG_CONFIG_HOME/llm-gateway/config.toml): b10x-llm-gateway serve --help\n\
                  To stop owned pods the gateway cannot: b10x-llm-gateway watchdog --help\n\
                  To store an upstream key in the file the document names: b10x-llm-gateway credential set --help"
)]
pub struct Cli {
    /// Path to the closed TOML deployment document.
    #[arg(long)]
    pub config: PathBuf,
}

/// Serve the deployment document: the one `--config` names, else the default document,
/// `$XDG_CONFIG_HOME/llm-gateway/config.toml` (`~/.config/llm-gateway/config.toml` when
/// `XDG_CONFIG_HOME` is unset).
#[derive(Debug, Parser)]
#[command(name = "serve", bin_name = "b10x-llm-gateway serve", version)]
pub struct ServeCli {
    /// Path to the closed TOML deployment document; the default document when absent.
    #[arg(long)]
    pub config: Option<PathBuf>,
}

/// Watch the pods the gateway owns and terminate one the gateway can no longer stop: its
/// heartbeat stale, the pod idle, not ready, past its maximum runtime, or the balance under its
/// floor. It reads the same document as `serve` and needs its `[watchdog]` table and a provider
/// with `runpod_api_key`.
#[derive(Debug, Parser)]
#[command(name = "watchdog", bin_name = "b10x-llm-gateway watchdog", version)]
pub struct WatchdogCli {
    /// Path to the closed TOML deployment document; the default document when absent.
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Run one poll and exit instead of polling until SIGINT or SIGTERM.
    #[arg(long)]
    pub once: bool,
}

/// The default deployment document: `$XDG_CONFIG_HOME/llm-gateway/config.toml` when
/// `XDG_CONFIG_HOME` is set to an absolute path, else `$HOME/.config/llm-gateway/config.toml`;
/// `None` when neither is set.
pub fn default_config_path(
    xdg_config_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    let base = xdg_config_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(".config"))
        })?;
    Some(base.join("llm-gateway").join("config.toml"))
}

/// [`default_config_path`] of this process's environment.
pub fn default_config() -> Option<PathBuf> {
    default_config_path(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

/// Upstream keys the deployment document names.
#[derive(Debug, Parser)]
#[command(name = "credential", bin_name = "b10x-llm-gateway credential", version)]
pub struct CredentialCli {
    #[command(subcommand)]
    pub command: CredentialCommand,
}

#[derive(Debug, Subcommand)]
pub enum CredentialCommand {
    /// Store one upstream key in the file the deployment document names for it.
    Set(SetArgs),
}

/// Store one upstream key in the file the deployment document names for it, after one probe of
/// `GET <base path>/models` with it. The key is read from a hidden prompt on the terminal, or
/// from standard input with `--stdin`; never from an argument or the environment.
#[derive(Debug, Args)]
#[command(group(ArgGroup::new("target").required(true).args(["model", "route"])))]
pub struct SetArgs {
    /// Path to the closed TOML deployment document.
    #[arg(long)]
    pub config: PathBuf,
    /// A `[hosted.<alias>]` model: its key goes to the entry's `credential_file`.
    #[arg(long, value_name = "ALIAS")]
    pub model: Option<String>,
    /// A route of the routes document the deployment names: its key goes to the route's
    /// endpoint's `file:` secret.
    #[arg(long, value_name = "NAME")]
    pub route: Option<String>,
    /// Read the key from standard input, one line, instead of a hidden prompt.
    #[arg(long)]
    pub stdin: bool,
    /// Store without probing the endpoint first.
    #[arg(long)]
    pub no_probe: bool,
}

/// Which command line a process was started with.
#[derive(Debug, PartialEq, Eq)]
pub enum Invocation {
    /// `b10x-llm-gateway --config <CONFIG>`, or anything that is not a command name first.
    Serve,
    /// `b10x-llm-gateway serve …`: the arguments from `serve` on.
    ServeCommand(Vec<OsString>),
    /// `b10x-llm-gateway watchdog …`: the arguments from `watchdog` on.
    Watchdog(Vec<OsString>),
    /// `b10x-llm-gateway credential …`: the arguments from `credential` on.
    Credential(Vec<OsString>),
}

impl Invocation {
    /// The command line `args` (the program first) selects.
    pub fn from_args(args: Vec<OsString>) -> Self {
        let rest: Vec<OsString> = args.into_iter().skip(1).collect();
        match rest.first().and_then(|first| first.to_str()) {
            Some("credential") => Self::Credential(rest),
            Some("serve") => Self::ServeCommand(rest),
            Some("watchdog") => Self::Watchdog(rest),
            _ => Self::Serve,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Invocation, default_config_path};
    use std::{ffi::OsString, path::PathBuf};

    #[test]
    fn the_default_document_follows_xdg_then_home() {
        let some = |text: &str| Some(OsString::from(text));
        assert_eq!(
            default_config_path(some("/x"), some("/h")),
            Some(PathBuf::from("/x/llm-gateway/config.toml"))
        );
        assert_eq!(
            default_config_path(None, some("/h")),
            Some(PathBuf::from("/h/.config/llm-gateway/config.toml"))
        );
        assert_eq!(
            default_config_path(some("relative"), some("/h")),
            Some(PathBuf::from("/h/.config/llm-gateway/config.toml")),
            "a relative XDG_CONFIG_HOME is ignored, as the XDG base directory specification says"
        );
        assert_eq!(default_config_path(None, None), None);
    }

    #[test]
    fn the_first_argument_selects_the_command_line() {
        let args = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(
            Invocation::from_args(args(&["b", "--config", "c"])),
            Invocation::Serve
        );
        assert_eq!(
            Invocation::from_args(args(&["b", "serve"])),
            Invocation::ServeCommand(args(&["serve"]))
        );
        assert_eq!(
            Invocation::from_args(args(&["b", "watchdog", "--once"])),
            Invocation::Watchdog(args(&["watchdog", "--once"]))
        );
        assert_eq!(
            Invocation::from_args(args(&["b", "credential", "set"])),
            Invocation::Credential(args(&["credential", "set"]))
        );
    }
}
