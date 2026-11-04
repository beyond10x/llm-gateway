//! `b10x-llm-gateway claude`: the launcher run against a loopback stand-in for the gateway's
//! `GET /v1/launch` and a stand-in `claude` (tests/fixture/claude.rs) that records what it
//! was started with. No real Claude Code, no network beyond loopback, no file outside
//! `CARGO_TARGET_TMPDIR`.

use std::{
    fs,
    io::{Read as _, Write as _},
    net::TcpListener,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread,
};

const LAUNCH: &str = r#"{"format":"llm-gateway.launch/1","base_url":"http://127.0.0.1:PORT","routes":[
 {"alias":"claude","model_prefix":"claude-","client_credentials":true,"messages":true,"tool_calling":true,
  "context_window":null,"compact_window":null,"label":"claude","phase":null},
 {"alias":"qwen","model_prefix":null,"client_credentials":false,"messages":true,"responses":true,"tool_calling":true,
  "context_window":131072,"compact_window":120000,"label":"qwen (served-model)","phase":"ready"}]}"#;

const CLIENT: &str = "[hosted.claude]\nwires = [\"messages\"]\nbase_url = \"https://api.anthropic.com\"\n\
                      auth_kind = \"client\"\nbilling_kind = \"subscription\"\n\
                      tool_calling = \"parsed\"\nmodel_prefix = \"claude-\"\n";

fn directory(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("launch-claude-{name}"));
    drop(fs::remove_dir_all(&dir));
    fs::create_dir_all(dir.join("out")).unwrap();
    fs::create_dir_all(dir.join("runtime")).unwrap();
    for path in [dir.clone(), dir.join("runtime")] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    dir
}

/// A gateway stand-in answering one request with the launch document; the request head comes
/// back on the channel.
fn gateway() -> (u16, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];
        while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap() == 1 {
            head.push(byte[0]);
        }
        let body = LAUNCH.replace("PORT", &port.to_string());
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        sender.send(String::from_utf8(head).unwrap()).unwrap();
    });
    (port, receiver)
}

fn config(dir: &Path, port: u16, extra: &str) -> PathBuf {
    let path = dir.join("gateway.toml");
    fs::write(
        &path,
        format!("listen = \"127.0.0.1:{port}\"\ningress_auth = \"none\"\n{extra}\n{CLIENT}"),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}

fn launcher(dir: &Path, config: &Path, args: &[&str], exit: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
        .arg("claude")
        .arg("--config")
        .arg(config)
        .args(args)
        .env(
            "B10X_LLM_GATEWAY_CLAUDE_BIN",
            env!("CARGO_BIN_EXE_claude-fixture"),
        )
        .env("FAKE_CLAUDE_OUT", dir.join("out"))
        .env("FAKE_CLAUDE_EXIT", exit)
        .env("XDG_RUNTIME_DIR", dir.join("runtime"))
        .env_remove("ANTHROPIC_BASE_URL")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("CLAUDE_CODE_SUBAGENT_MODEL")
        .env_remove("ANTHROPIC_DEFAULT_HAIKU_MODEL")
        .env_remove("CLAUDE_CODE_MAX_CONTEXT_TOKENS")
        .output()
        .unwrap()
}

fn read(dir: &Path, name: &str) -> String {
    fs::read_to_string(dir.join("out").join(name)).unwrap()
}

#[test]
fn the_launcher_starts_claude_with_the_overlay_and_passes_its_exit_code() {
    let dir = directory("start");
    let (port, requests) = gateway();
    let config = config(&dir, port, "");
    let output = launcher(&dir, &config, &["-c", "--verbose", "--version"], "7");
    assert_eq!(output.status.code(), Some(7), "{output:?}");
    let head = requests.recv().unwrap();
    assert!(head.starts_with("GET /v1/launch HTTP/1."), "{head}");

    let argv = read(&dir, "argv");
    let argv: Vec<&str> = argv.lines().collect();
    let overlay_path = read(&dir, "overlay_path");
    assert_eq!(
        argv,
        [
            "--settings",
            overlay_path.as_str(),
            "--model",
            "opus",
            "-c",
            "--verbose",
            "--version",
            "--append-system-prompt",
            "To run a sub-agent on `qwen` (served-model), call the Agent tool without the `model` parameter: CLAUDE_CODE_SUBAGENT_MODEL is `qwen` in this session. The `model` enum cannot name `qwen`. Do this whenever the user asks for a `qwen` sub-agent."
        ]
    );
    assert!(
        Path::new(&overlay_path).starts_with(dir.join("runtime")),
        "{overlay_path}"
    );
    assert!(
        !Path::new(&overlay_path).exists(),
        "the overlay is deleted when claude exits"
    );
    assert_eq!(read(&dir, "overlay_mode"), "600");
    let overlay: serde_json::Value = serde_json::from_str(&read(&dir, "overlay")).unwrap();
    assert_eq!(
        overlay,
        serde_json::json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://127.0.0.1:{port}"),
                "CLAUDE_CODE_SUBAGENT_MODEL": "qwen",
                "CLAUDE_CODE_MAX_CONTEXT_TOKENS": "131072"
            },
            "modelSettings": { "qwen": { "autoCompactWindow": 120_000 } },
            "modelPicker": { "options": [ { "model": "qwen", "label": "qwen (served-model)" } ] }
        })
    );
    assert_eq!(
        read(&dir, "env"),
        "",
        "the launcher sets no credential and no Claude variable in the child's environment"
    );
}

#[test]
fn launch_claude_settings_are_read_from_the_document() {
    let dir = directory("table");
    let (port, _requests) = gateway();
    let config = config(
        &dir,
        port,
        "[launch.claude]\nmain_model = \"sonnet\"\nsubagent_instruction = \"\"\nagents = '{\"r\":{\"description\":\"d\",\"prompt\":\"p\"}}'\n",
    );
    let output = launcher(&dir, &config, &["--model", "haiku"], "0");
    assert!(output.status.success(), "{output:?}");
    let argv = read(&dir, "argv");
    let argv: Vec<&str> = argv.lines().collect();
    assert_eq!(
        &argv[2..],
        [
            "--agents",
            "{\"r\":{\"description\":\"d\",\"prompt\":\"p\"}}",
            "--model",
            "haiku"
        ],
        "a --model on the command line wins over main_model"
    );
    let overlay: serde_json::Value = serde_json::from_str(&read(&dir, "overlay")).unwrap();
    assert!(
        overlay["env"]
            .get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
            .is_none()
    );
}

#[test]
fn the_subagent_instruction_is_joined_to_the_users_appended_prompt() {
    let dir = directory("append");
    let (port, _requests) = gateway();
    let document = config(
        &dir,
        port,
        "[launch.claude]\nsubagent_instruction = \"use {route} for {model}\"\n",
    );
    let output = launcher(
        &dir,
        &document,
        &["--append-system-prompt", "be brief", "-p"],
        "0",
    );
    assert!(output.status.success(), "{output:?}");
    let argv = read(&dir, "argv");
    assert!(
        argv.ends_with("--append-system-prompt\nbe brief\n\nuse qwen for served-model\n-p"),
        "{argv}"
    );
    assert_eq!(argv.matches("--append-system-prompt").count(), 1, "{argv}");

    let (port, _requests) = gateway();
    let document = config(
        &dir,
        port,
        "[launch.claude]\nsubagent_instruction = \"use {route} for {model}\"\n",
    );
    let output = launcher(&dir, &document, &["--append-system-prompt=mine"], "0");
    assert!(output.status.success(), "{output:?}");
    let argv = read(&dir, "argv");
    assert!(
        argv.ends_with("--append-system-prompt=mine\n\nuse qwen for served-model"),
        "{argv}"
    );
}

#[test]
fn no_agents_are_passed_by_default_and_a_silent_gateway_is_named() {
    let dir = directory("silent");
    let unused = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = unused.local_addr().unwrap().port();
    drop(unused);
    let config = config(&dir, port, "");
    let output = launcher(&dir, &config, &[], "0");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no gateway answers at 127.0.0.1:"),
        "{stderr}"
    );
    assert!(
        !dir.join("out").join("argv").exists(),
        "claude is not started without the gateway"
    );
    assert!(
        fs::read_dir(dir.join("runtime")).unwrap().next().is_none(),
        "no overlay is left behind"
    );
}

#[test]
fn the_codex_launcher_passes_the_gateway_as_overrides_and_writes_nothing() {
    let dir = directory("codex");
    let (port, requests) = gateway();
    let config = config(&dir, port, "");
    let output = Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
        .args(["codex", "--config"])
        .arg(&config)
        .args(["exec", "--json"])
        .env(
            "B10X_LLM_GATEWAY_CODEX_BIN",
            env!("CARGO_BIN_EXE_claude-fixture"),
        )
        .env("FAKE_CLAUDE_OUT", dir.join("out"))
        .env("FAKE_CLAUDE_EXIT", "3")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert!(
        requests
            .recv()
            .unwrap()
            .starts_with("GET /v1/launch HTTP/1.")
    );
    let argv = read(&dir, "argv");
    let argv: Vec<&str> = argv.lines().collect();
    let provider = format!(
        "model_providers.llm-gateway={{ name = \"llm-gateway\", base_url = \"http://127.0.0.1:{port}/v1\", wire_api = \"responses\" }}"
    );
    assert_eq!(
        argv,
        [
            "-c",
            provider.as_str(),
            "-c",
            "model_provider=llm-gateway",
            "-c",
            "model_context_window=131072",
            "-c",
            "model_auto_compact_token_limit=120000",
            "-m",
            "qwen",
            "exec",
            "--json"
        ]
    );
}

#[test]
fn the_removed_remap_haiku_key_is_refused_like_any_unknown_key() {
    let dir = directory("remap");
    let config = config(&dir, 1, "[launch.claude]\nremap_haiku = true\n");
    let output = launcher(&dir, &config, &[], "0");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("config:schema"), "{stderr}");
    assert!(!dir.join("out").join("argv").exists());
}
