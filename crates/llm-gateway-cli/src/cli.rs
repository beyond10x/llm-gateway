//! The command line of `b10x-llm-gateway`. It lives in the library so that the documentation
//! site's CLI reference is generated from this definition.

use std::path::PathBuf;

use clap::Parser;

/// The authenticated single-owner llm-gateway, configured by one closed TOML document.
#[derive(Debug, Parser)]
#[command(name = "b10x-llm-gateway", version)]
pub struct Cli {
    /// Path to the closed TOML deployment document.
    #[arg(long)]
    pub config: PathBuf,
}
