//! A stand-in for Claude Code in the launcher's process tests (`tests/launch_claude.rs`). It
//! records its arguments, the environment variables it was started with, the settings overlay
//! `--settings` names and that file's mode into the directory `FAKE_CLAUDE_OUT` names, then
//! exits with `FAKE_CLAUDE_EXIT` (default 0). It makes no call.

use std::{fs, os::unix::fs::PermissionsExt as _, path::PathBuf};

/// The variables the launcher must not set in the child's environment: the overlay carries
/// the gateway's, and the credentials stay unset.
const WATCHED: [&str; 6] = [
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
];

fn main() {
    let out = PathBuf::from(std::env::var_os("FAKE_CLAUDE_OUT").expect("FAKE_CLAUDE_OUT"));
    let args: Vec<String> = std::env::args().skip(1).collect();
    fs::write(out.join("argv"), args.join("\n")).expect("argv");
    let mut env: Vec<String> = std::env::vars()
        .filter(|(name, _)| WATCHED.contains(&name.as_str()))
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    env.sort();
    fs::write(out.join("env"), env.join("\n")).expect("env");
    if let Some(index) = args.iter().position(|arg| arg == "--settings") {
        let path = &args[index + 1];
        let mode = fs::metadata(path).expect("overlay").permissions().mode() & 0o777;
        fs::write(out.join("overlay"), fs::read(path).expect("overlay")).expect("overlay copy");
        fs::write(out.join("overlay_mode"), format!("{mode:o}")).expect("mode");
        fs::write(out.join("overlay_path"), path).expect("path");
    }
    let code = std::env::var("FAKE_CLAUDE_EXIT")
        .ok()
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    std::process::exit(code);
}
