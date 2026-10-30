//! The command lines of `b10x-llm-gateway`. They live in the library so that the documentation
//! site's CLI reference is generated from these definitions.
//!
//! The serving command line, [`Cli`], is closed: `--config <CONFIG>`, help and version. The
//! `credential set` command, [`CredentialCli`], is parsed instead only when `credential` is the
//! first argument ([`Invocation::from_args`]), so every serving command line is exactly what it
//! was. A usage error of either names its kind and never echoes an argument, which may be a key.

use std::{ffi::OsString, path::PathBuf};

use clap::{ArgGroup, Args, Parser, Subcommand};

/// The authenticated single-owner llm-gateway, configured by one closed TOML document.
#[derive(Debug, Parser)]
#[command(
    name = "b10x-llm-gateway",
    version,
    after_help = "To store an upstream key in the file the document names: b10x-llm-gateway credential set --help"
)]
pub struct Cli {
    /// Path to the closed TOML deployment document.
    #[arg(long)]
    pub config: PathBuf,
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
    /// `b10x-llm-gateway --config <CONFIG>`, or anything that is not `credential` first.
    Serve,
    /// `b10x-llm-gateway credential …`: the arguments from `credential` on.
    Credential(Vec<OsString>),
}

impl Invocation {
    /// The command line `args` (the program first) selects.
    pub fn from_args(args: Vec<OsString>) -> Self {
        if args.get(1).is_some_and(|first| first == "credential") {
            Self::Credential(args.into_iter().skip(1).collect())
        } else {
            Self::Serve
        }
    }
}
