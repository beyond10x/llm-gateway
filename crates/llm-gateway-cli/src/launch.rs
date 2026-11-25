//! `b10x-llm-gateway claude [claude args…]`: start Claude Code against the running gateway.
//!
//! It reads the gateway's address and `[launch.claude]` from the deployment document, asks the
//! gateway for `GET /v1/launch`, writes a settings overlay (mode 0600, under
//! `$XDG_RUNTIME_DIR`, else `~/.cache/llm-gateway/`), runs `claude --settings <overlay> …` as a
//! child on the same terminal, forwards SIGTERM and SIGHUP to it, deletes the overlay when it
//! exits and exits with its status. SIGINT from the terminal reaches `claude` directly, as the
//! terminal sends it to the whole foreground process group; the launcher ignores it rather than
//! forwarding a second one. The overlay never names a credential and nothing under `~/.claude/`
//! is touched.

use std::{
    ffi::OsString,
    fs,
    io::{Read as _, Write as _},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    os::unix::{
        fs::{DirBuilderExt as _, OpenOptionsExt as _},
        process::ExitStatusExt as _,
    },
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    time::Duration,
};

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    LaunchClaude, LaunchCodex,
    cli::{ClaudeCli, CodexCli},
    serve::report,
};

/// The gateway's launch route and the format it answers in.
const LAUNCH_PATH: &str = "/v1/launch";
const LAUNCH_FORMAT: &str = "llm-gateway.launch/1";
/// How long the gateway may take to answer the launch route.
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
/// The largest launch document read.
const MAX_LAUNCH_BYTES: u64 = 1 << 20;
/// Claude Code's accepted `autoCompactWindow` range.
const COMPACT_MIN: u64 = 100_000;
const COMPACT_MAX: u64 = 1_000_000;

/// The launch document, `llm-gateway.launch/1`.
#[derive(Debug, Deserialize)]
pub struct LaunchDocument {
    pub format: String,
    pub base_url: String,
    pub routes: Vec<LaunchRoute>,
}

/// One relayed model of the launch document. Its flags mirror the document's members one to
/// one, so they stay separate booleans.
#[derive(Debug, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct LaunchRoute {
    pub alias: String,
    pub client_credentials: bool,
    pub messages: bool,
    #[serde(default)]
    pub responses: bool,
    pub tool_calling: bool,
    pub context_window: Option<u64>,
    pub compact_window: Option<u64>,
    pub label: String,
    pub phase: Option<String>,
}

/// The overlay for `document` and `launch`, and the notes the launcher prints about it.
pub fn overlay(document: &LaunchDocument, launch: &LaunchClaude) -> (Value, Vec<String>) {
    let mut notes = Vec::new();
    let mut env = Map::new();
    env.insert("ANTHROPIC_BASE_URL".into(), json!(document.base_url));
    let implementer = document
        .routes
        .iter()
        .find(|route| route.alias == launch.implementer_route);
    match implementer {
        Some(route) if route.messages && route.tool_calling => {
            env.insert("CLAUDE_CODE_SUBAGENT_MODEL".into(), json!(route.alias));
            if let (false, Some(window)) = (route.alias.starts_with("claude-"), route.context_window)
            {
                env.insert(
                    "CLAUDE_CODE_MAX_CONTEXT_TOKENS".into(),
                    json!(window.to_string()),
                );
            }
            if let Some(phase) = route.phase.as_deref().filter(|phase| *phase != "ready") {
                notes.push(format!(
                    "route {} is {phase}; its first call may wait for it",
                    route.alias
                ));
            }
        }
        Some(route) => notes.push(format!(
            "route {} does not serve Claude Code (messages wire with tool calling); subagents keep their default model",
            route.alias
        )),
        None => notes.push(format!(
            "the gateway relays no route {}; subagents keep their default model",
            launch.implementer_route
        )),
    }
    let mut settings = Map::new();
    let mut options = Vec::new();
    for route in &document.routes {
        if let Some(window) = route.compact_window
            && (COMPACT_MIN..=COMPACT_MAX).contains(&window)
        {
            settings.insert(route.alias.clone(), json!({ "autoCompactWindow": window }));
        }
        if route.messages && route.tool_calling && !route.client_credentials {
            options.push(json!({ "model": route.alias, "label": route.label }));
        }
    }
    let mut overlay = Map::new();
    overlay.insert("env".into(), Value::Object(env));
    if !settings.is_empty() {
        overlay.insert("modelSettings".into(), Value::Object(settings));
    }
    if !options.is_empty() {
        overlay.insert("modelPicker".into(), json!({ "options": options }));
    }
    (Value::Object(overlay), notes)
}

/// `claude`'s arguments: the overlay, `--model` unless `args` names one, `--agents` when
/// configured, then `args` unchanged.
pub fn claude_args(
    launch: &LaunchClaude,
    instruction: Option<&str>,
    overlay: &Path,
    args: &[OsString],
) -> Vec<OsString> {
    let names_model = args.iter().any(|arg| {
        arg.to_str()
            .is_some_and(|arg| arg == "--model" || arg.starts_with("--model="))
    });
    let mut out: Vec<OsString> = vec!["--settings".into(), overlay.as_os_str().to_owned()];
    if !names_model {
        out.push("--model".into());
        out.push(launch.main_model.clone().into());
    }
    if let Some(agents) = &launch.agents {
        out.push("--agents".into());
        out.push(agents.clone().into());
    }
    let Some(instruction) = instruction else {
        out.extend(args.iter().cloned());
        return out;
    };
    let mut user = args.iter().cloned();
    let mut joined = false;
    while let Some(arg) = user.next() {
        if joined {
            out.push(arg);
            continue;
        }
        let text = arg.to_str();
        if text == Some(APPEND_PROMPT) {
            out.push(arg);
            if let Some(theirs) = user.next() {
                let mut both = theirs;
                both.push("\n\n");
                both.push(instruction);
                out.push(both);
                joined = true;
            }
        } else if let Some(theirs) = text.and_then(|text| {
            text.strip_prefix(APPEND_PROMPT)
                .and_then(|rest| rest.strip_prefix('='))
        }) {
            out.push(format!("{APPEND_PROMPT}={theirs}\n\n{instruction}").into());
            joined = true;
        } else {
            out.push(arg);
        }
    }
    if !joined {
        out.push(APPEND_PROMPT.into());
        out.push(instruction.into());
    }
    out
}

/// Claude Code's flag for text appended to its system prompt.
const APPEND_PROMPT: &str = "--append-system-prompt";

/// The subagent instruction for `launch`, its placeholders filled from the implementer route of
/// `document`; none when it is empty or the gateway does not serve the route.
pub fn subagent_instruction(document: &LaunchDocument, launch: &LaunchClaude) -> Option<String> {
    if launch.subagent_instruction.trim().is_empty() {
        return None;
    }
    let route = document
        .routes
        .iter()
        .find(|route| route.alias == launch.implementer_route)?;
    let model = route
        .label
        .strip_prefix(&route.alias)
        .and_then(|rest| rest.strip_prefix(" ("))
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or(&route.alias);
    Some(
        launch
            .subagent_instruction
            .replace("{route}", &route.alias)
            .replace("{model}", model),
    )
}

/// The address to reach a gateway listening on `listen` at.
fn reach(listen: SocketAddr) -> SocketAddr {
    let ip = match listen.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, listen.port())
}

/// `GET /v1/launch` of the gateway at `address`.
fn fetch(address: SocketAddr) -> Result<LaunchDocument, String> {
    let unreachable = |error: std::io::Error| {
        format!("no gateway answers at {address} ({error}); start it with `b10x-llm-gateway serve`")
    };
    let mut stream = TcpStream::connect_timeout(&address, FETCH_TIMEOUT).map_err(unreachable)?;
    stream
        .set_read_timeout(Some(FETCH_TIMEOUT))
        .map_err(unreachable)?;
    stream
        .set_write_timeout(Some(FETCH_TIMEOUT))
        .map_err(unreachable)?;
    write!(
        stream,
        "GET {LAUNCH_PATH} HTTP/1.0\r\nhost: {address}\r\naccept: application/json\r\nconnection: close\r\n\r\n"
    )
    .map_err(unreachable)?;
    let mut answer = Vec::new();
    stream
        .take(MAX_LAUNCH_BYTES)
        .read_to_end(&mut answer)
        .map_err(unreachable)?;
    let split = answer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| format!("the gateway at {address} sent no complete answer"))?;
    let head = String::from_utf8_lossy(&answer[..split]);
    let status = head.split_whitespace().nth(1).unwrap_or("");
    if status != "200" {
        return Err(format!(
            "the gateway at {address} answered {LAUNCH_PATH} with {status}; it must be a gateway of this release, open on loopback (ingress_auth = \"none\")"
        ));
    }
    let chunked = head.lines().any(|line| {
        let line = line.to_ascii_lowercase();
        line.starts_with("transfer-encoding:") && line.contains("chunked")
    });
    let body = &answer[split + 4..];
    let body = if chunked {
        dechunk(body)
    } else {
        body.to_vec()
    };
    let document: LaunchDocument = serde_json::from_slice(&body)
        .map_err(|error| format!("the gateway's launch document is unreadable: {error}"))?;
    if document.format != LAUNCH_FORMAT {
        return Err(format!(
            "the gateway's launch document is {}, not {LAUNCH_FORMAT}",
            document.format
        ));
    }
    Ok(document)
}

/// A chunked body's data.
fn dechunk(mut body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let Some(end) = body.windows(2).position(|window| window == b"\r\n") else {
            return out;
        };
        let size = std::str::from_utf8(&body[..end])
            .ok()
            .and_then(|line| usize::from_str_radix(line.split(';').next()?.trim(), 16).ok())
            .unwrap_or(0);
        if size == 0 || body.len() < end + 2 + size {
            return out;
        }
        out.extend_from_slice(&body[end + 2..end + 2 + size]);
        body = body.get(end + 4 + size..).unwrap_or_default();
    }
}

/// The directory the overlay goes to: `$XDG_RUNTIME_DIR` when absolute, else
/// `~/.cache/llm-gateway/` (created 0700).
fn overlay_directory() -> Result<PathBuf, String> {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)
        && runtime.is_absolute()
        && runtime.is_dir()
    {
        return Ok(runtime);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
        .ok_or("neither XDG_RUNTIME_DIR nor HOME is set")?;
    let directory = home.join(".cache").join("llm-gateway");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    Ok(directory)
}

/// The overlay file, deleted when dropped.
struct Overlay(PathBuf);

impl Overlay {
    fn write(overlay: &Value) -> Result<Self, String> {
        let directory = overlay_directory()?;
        let path = directory.join(format!(
            "llm-gateway-claude-{}.settings.json",
            std::process::id()
        ));
        drop(fs::remove_file(&path));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        let file_guard = Self(path);
        let text = serde_json::to_vec_pretty(overlay).map_err(|error| error.to_string())?;
        file.write_all(&text)
            .map_err(|error| format!("cannot write {}: {error}", file_guard.0.display()))?;
        Ok(file_guard)
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        drop(fs::remove_file(&self.0));
    }
}

/// `codex`'s arguments: the gateway as a `-c` provider override, and, unless `args` names a
/// model, `-m` with the `[launch.codex]` model and its context and auto-compact windows; then
/// `args` unchanged.
///
/// # Errors
/// The model the launcher would choose is not one Codex reaches through this gateway.
pub fn codex_args(
    document: &LaunchDocument,
    launch: &LaunchCodex,
    args: &[OsString],
) -> Result<Vec<OsString>, String> {
    let names_model = args.iter().any(|arg| {
        arg.to_str().is_some_and(|arg| {
            arg == "-m" || arg == "--model" || arg.starts_with("--model=") || arg.starts_with("-m=")
        })
    });
    let base = serde_json::to_string(&format!("{}/v1", document.base_url))
        .map_err(|error| error.to_string())?;
    let mut out: Vec<OsString> = vec![
        "-c".into(),
        format!(
            "model_providers.llm-gateway={{ name = \"llm-gateway\", base_url = {base}, wire_api = \"responses\" }}"
        )
        .into(),
        "-c".into(),
        "model_provider=llm-gateway".into(),
    ];
    if !names_model {
        let route = document
            .routes
            .iter()
            .find(|route| route.alias == launch.model)
            .filter(|route| route.responses && route.tool_calling && !route.client_credentials)
            .ok_or_else(|| {
                format!(
                    "the gateway relays no route {} Codex can use (responses wire with tool calling); set [launch.codex].model or pass -m",
                    launch.model
                )
            })?;
        if let Some(window) = route.context_window {
            out.push("-c".into());
            out.push(format!("model_context_window={window}").into());
        }
        if let Some(window) = route.compact_window {
            out.push("-c".into());
            out.push(format!("model_auto_compact_token_limit={window}").into());
        }
        out.push("-m".into());
        out.push(route.alias.clone().into());
    }
    out.extend(args.iter().cloned());
    Ok(out)
}

/// `b10x-llm-gateway claude`.
pub fn main(cli: ClaudeCli) -> ExitCode {
    finish("claude", run(cli))
}

/// `b10x-llm-gateway codex`.
pub fn codex_main(cli: CodexCli) -> ExitCode {
    finish("codex", run_codex(cli))
}

fn finish(command: &str, outcome: Result<ExitCode, String>) -> ExitCode {
    match outcome {
        Ok(code) => code,
        Err(message) => {
            report(format_args!("{command}: {message}"));
            ExitCode::FAILURE
        }
    }
}

/// The deployment at `config` (else the default document), refused unless it is open on
/// loopback, and its launch document.
fn discover(config: Option<PathBuf>) -> Result<(crate::Deployment, LaunchDocument), String> {
    let path = crate::serve::document_path(config).map_err(|refusal| refusal.to_string())?;
    let deployment = crate::load(&path).map_err(|refusal| format!("refused {refusal}"))?;
    if deployment.gateway.ingress_auth != llm_gateway::IngressAuth::None {
        return Err(
            "the gateway takes an owner token (ingress_auth is not \"none\"); the launchers start an agent only against a gateway open on loopback"
                .to_owned(),
        );
    }
    let document = fetch(reach(deployment.gateway.bind))?;
    Ok((deployment, document))
}

fn run(cli: ClaudeCli) -> Result<ExitCode, String> {
    let (deployment, document) = discover(cli.config)?;
    let launch = deployment.launch_claude;
    let (overlay, notes) = overlay(&document, &launch);
    for note in notes {
        report(format_args!("claude: {note}"));
    }
    for name in ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"] {
        if std::env::var_os(name).is_some() {
            report(format_args!(
                "claude: {name} is set in this environment; Claude Code sends it instead of your login"
            ));
        }
    }
    let file = Overlay::write(&overlay)?;
    let program = cli.claude_bin.unwrap_or_else(|| PathBuf::from("claude"));
    let instruction = subagent_instruction(&document, &launch);
    let args = claude_args(&launch, instruction.as_deref(), &file.0, &cli.args);
    let code = run_child(&program, &args);
    drop(file);
    code
}

fn run_codex(cli: CodexCli) -> Result<ExitCode, String> {
    let (deployment, document) = discover(cli.config)?;
    let args = codex_args(&document, &deployment.launch_codex, &cli.args)?;
    let program = cli.codex_bin.unwrap_or_else(|| PathBuf::from("codex"));
    run_child(&program, &args)
}

/// Runs `program` with `args` on this terminal, forwarding SIGTERM and SIGHUP to it and
/// ignoring the terminal's SIGINT and SIGQUIT, which reach it directly; its exit status, or
/// 128 + the signal that ended it.
fn run_child(program: &Path, args: &[OsString]) -> Result<ExitCode, String> {
    let signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
        signal_hook::consts::SIGQUIT,
    ])
    .map_err(|error| format!("cannot install signal handlers: {error}"))?;
    let mut child = Command::new(program)
        .args(args)
        .spawn()
        .map_err(|error| format!("cannot start {}: {error}", program.display()))?;
    let pid = rustix::process::Pid::from_child(&child);
    let handle = signals.handle();
    let forwarder = std::thread::spawn(move || {
        let mut signals = signals;
        for signal in signals.forever() {
            let forwarded = match signal {
                signal_hook::consts::SIGTERM => rustix::process::Signal::TERM,
                signal_hook::consts::SIGHUP => rustix::process::Signal::HUP,
                _ => continue,
            };
            let _ = rustix::process::kill_process(pid, forwarded);
        }
    });
    let status = child.wait();
    handle.close();
    drop(forwarder.join());
    let status = status.map_err(|error| format!("lost {}: {error}", program.display()))?;
    let code = status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1);
    Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
}

#[cfg(test)]
mod tests {
    use super::{LaunchDocument, claude_args, dechunk, overlay};
    use crate::LaunchClaude;
    use serde_json::json;
    use std::{ffi::OsString, path::Path};

    const FIXTURE: &str = r#"{"format":"llm-gateway.launch/1","base_url":"http://127.0.0.1:8091",
        "routes":[
          {"alias":"claude","model_prefix":"claude-","client_credentials":true,"messages":true,
           "tool_calling":true,"context_window":null,"compact_window":null,"label":"claude","phase":null},
          {"alias":"qwen","model_prefix":null,"client_credentials":false,"messages":true,"responses":true,
           "tool_calling":true,"context_window":131072,"compact_window":120000,
           "label":"qwen (Qwen3-Coder on an H100)","phase":"ready"},
          {"alias":"embed","model_prefix":null,"client_credentials":false,"messages":false,
           "tool_calling":false,"context_window":8192,"compact_window":null,"label":"embed","phase":null}
        ]}"#;

    fn document() -> LaunchDocument {
        serde_json::from_str(FIXTURE).unwrap()
    }

    #[test]
    fn the_overlay_points_claude_code_at_the_gateway_and_the_implementer_route() {
        let (overlay, notes) = overlay(&document(), &LaunchClaude::default());
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(
            overlay,
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "http://127.0.0.1:8091",
                    "CLAUDE_CODE_SUBAGENT_MODEL": "qwen",
                    "CLAUDE_CODE_MAX_CONTEXT_TOKENS": "131072"
                },
                "modelSettings": { "qwen": { "autoCompactWindow": 120_000 } },
                "modelPicker": { "options": [
                    { "model": "qwen", "label": "qwen (Qwen3-Coder on an H100)" }
                ] }
            })
        );
        let text = overlay.to_string();
        for credential in ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "_FORCE"] {
            assert!(!text.contains(credential), "{credential} in {text}");
        }
    }

    #[test]
    fn a_missing_implementer_route_is_a_note() {
        let launch = LaunchClaude {
            implementer_route: "absent".to_owned(),
            ..LaunchClaude::default()
        };
        let (missing, notes) = overlay(&document(), &launch);
        assert_eq!(notes.len(), 1);
        assert_eq!(
            missing["env"],
            json!({ "ANTHROPIC_BASE_URL": "http://127.0.0.1:8091" })
        );
    }

    #[test]
    fn the_main_model_and_agents_are_passed_only_when_due() {
        let args = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
        let path = Path::new("/run/o.json");
        let launch = LaunchClaude::default();
        assert_eq!(
            claude_args(&launch, None, path, &args(&["-c"])),
            args(&["--settings", "/run/o.json", "--model", "opus", "-c"])
        );
        assert_eq!(
            claude_args(&launch, None, path, &args(&["--model=sonnet"])),
            args(&["--settings", "/run/o.json", "--model=sonnet"])
        );
        let launch = LaunchClaude {
            agents: Some("{}".to_owned()),
            ..LaunchClaude::default()
        };
        assert_eq!(
            claude_args(&launch, None, path, &args(&["--model", "x"])),
            args(&[
                "--settings",
                "/run/o.json",
                "--agents",
                "{}",
                "--model",
                "x"
            ])
        );
    }

    #[test]
    fn a_chunked_body_is_joined() {
        assert_eq!(dechunk(b"3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n"), b"abcde");
    }
}
