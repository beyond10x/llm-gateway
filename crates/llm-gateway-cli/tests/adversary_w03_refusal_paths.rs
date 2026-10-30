//! Adversary, wave 2026-10-30-w03: story:trusted-refusal-names-no-path and
//! story:translation-refusal-names-path, attacked through the shipped binary.
//!
//! Only loopback sockets and files under `CARGO_TARGET_TMPDIR` are used.

mod support;

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use support::{Fixture, OWNER_SECRET, bearer_keys, hosted_document, hosted_table};

const LISTENING: &str = "b10x-llm-gateway: listening on ";
const STARTUP: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(10);

/// What a secret written by mistake as a path looks like.
const PATH_MARKER: &str = "sk-adv-path-marker-5e1b";

/// The key `credential set` is given. Not a secret.
const KEY: &str = "sk-test-adversary-upstream-key-not-a-secret";

// --- The binary --------------------------------------------------------------------------------

struct Server {
    child: Child,
    address: SocketAddr,
    lines: mpsc::Receiver<String>,
}

impl Server {
    fn start(config: &Path, environment: &[(&str, String)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway"));
        command
            .args(["--config", config.to_str().unwrap()])
            .env_remove("AWS_CONTAINER_CREDENTIALS_FULL_URI")
            .env_remove("AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE")
            .env_remove("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI")
            .env_remove("AWS_CONTAINER_AUTHORIZATION_TOKEN")
            .env("RUST_LOG", "info")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        for (name, value) in environment {
            command.env(name, value);
        }
        let mut child = command.spawn().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + STARTUP;
        let mut before = Vec::new();
        let address = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(line) = lines.recv_timeout(left) else {
                drop(child.kill());
                panic!("the binary never reported {LISTENING:?}: {before:?}");
            };
            if let Some(address) = line.strip_prefix(LISTENING) {
                break address.parse().unwrap();
            }
            before.push(line);
        };
        Self {
            child,
            address,
            lines,
        }
    }

    fn log(mut self) -> Vec<String> {
        drop(self.child.kill());
        drop(self.child.wait());
        let mut lines = Vec::new();
        while let Ok(line) = self.lines.recv_timeout(WAIT) {
            lines.push(line);
        }
        lines
    }

    fn post(&self, path: &str, body: &str) -> String {
        let mut stream = TcpStream::connect(self.address).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        write!(
            stream,
            "POST {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut raw = Vec::new();
        drop(stream.read_to_end(&mut raw));
        String::from_utf8_lossy(&raw).into_owned()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

fn refusal_lines(log: &[String]) -> Vec<&String> {
    log.iter()
        .filter(|line| line.contains("translation refused"))
        .collect()
}

// --- Translation: a refused request on a Bedrock route --------------------------------------

const PROMPT_MARKER: &str = "zqAdvBedrockPromptMarker";

/// A Bedrock route's refusals, one by llm's chat decoder and one by Converse's own check, each
/// write exactly one "translation refused" line naming the code and wire, and no value.
#[test]
fn adv_w03_a_bedrock_routes_refusals_each_write_one_line_without_a_value() {
    let fixture = Fixture::new("adv-w03-bedrock-refusal");
    let token = fixture.write(
        "container-token",
        b"fake-container-authorization-token-0123456789\n",
        0o644,
    );
    let credentials = TcpListener::bind("127.0.0.1:0").unwrap();
    let bedrock = TcpListener::bind("127.0.0.1:0").unwrap();
    let table = format!(
        "[hosted.nova]\nkind = \"bedrock\"\nregion = \"eu-central-1\"\nmodel_id = \"eu.amazon.nova-fixture-v1:0\"\ncredential_source = \"container\"\nendpoint = \"http://{}\"\n",
        bedrock.local_addr().unwrap()
    );
    let secret = fixture.owner_secret();
    let config = fixture.config(&hosted_document("127.0.0.1:0", &secret, &table));
    let server = Server::start(
        &config,
        &[
            (
                "AWS_CONTAINER_CREDENTIALS_FULL_URI",
                format!(
                    "http://{}/v1/credentials",
                    credentials.local_addr().unwrap()
                ),
            ),
            (
                "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
                token.display().to_string(),
            ),
        ],
    );
    let unknown_field = serde_json::json!({
        "model": "nova",
        "messages": [{"role": "user", "content": PROMPT_MARKER}],
        "safeguards": {"note": PROMPT_MARKER},
    });
    let assistant_first = serde_json::json!({
        "model": "nova",
        "messages": [
            {"role": "assistant", "content": PROMPT_MARKER},
            {"role": "user", "content": PROMPT_MARKER},
        ],
    });
    for body in [&unknown_field, &assistant_first] {
        let answer = server.post("/v1/chat/completions", &body.to_string());
        assert!(answer.starts_with("HTTP/1.1 400 "), "{answer}");
        assert!(answer.contains("translation-unsupported"), "{answer}");
    }
    let log = server.log();
    let refusals = refusal_lines(&log);
    assert_eq!(refusals.len(), 2, "{log:#?}");
    assert!(
        refusals[0].contains("outside the published subset: ?\""),
        "{}",
        refusals[0]
    );
    assert!(!refusals[0].contains("safeguards"), "{}", refusals[0]);
    for line in &refusals {
        assert!(line.contains("refusal=translation-unsupported"), "{line}");
        assert!(line.contains("ingress=chat"), "{line}");
        assert!(line.contains("model=\"nova\""), "{line}");
    }
    for line in &log {
        assert!(!line.contains(PROMPT_MARKER), "a value was logged: {line}");
    }
}

// --- Trusted paths: `credential set` -----------------------------------------------------------

/// `credential set --stdin --no-probe` for the hosted model `remote`, its `credential_file` at
/// `target`.
fn credential_set(fixture: &Fixture, target: &Path) -> (Option<i32>, String) {
    let secret = fixture.owner_secret();
    let config = fixture.config(&hosted_document(
        "127.0.0.1:0",
        &secret,
        &hosted_table("remote", "http://127.0.0.1:9/v1", &bearer_keys(target)),
    ));
    let mut child = Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
        .args([
            "credential",
            "set",
            "--config",
            config.to_str().unwrap(),
            "--model",
            "remote",
            "--no-probe",
            "--stdin",
        ])
        .env("HOME", &fixture.dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(format!("{KEY}\n").as_bytes()).unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.code(), text)
}

/// The CHANGELOG entry for story:trusted-refusal-names-no-path says a secret written into a
/// `*_file` setting by mistake "is not printed or logged". `credential set` reads that setting
/// and still prints it in its `credential:directory-missing` refusal.
#[test]
fn adv_w03_credential_set_refusal_does_not_print_the_credential_file_path() {
    let fixture = Fixture::new("adv-w03-credential-missing-dir");
    let target = fixture.path(PATH_MARKER).join("key");
    let (code, text) = credential_set(&fixture, &target);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("refused credential:directory-missing: "),
        "{text}"
    );
    assert!(!text.contains(PATH_MARKER), "the path was printed: {text}");
}

/// The same setting, stored: the success line prints the path the setting holds.
#[test]
fn adv_w03_credential_set_success_does_not_print_the_credential_file_path() {
    let fixture = Fixture::new("adv-w03-credential-stored");
    let dir: PathBuf = fixture.path(PATH_MARKER);
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let (code, text) = credential_set(&fixture, &dir.join("key"));
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("credential stored "), "{text}");
    assert!(!text.contains(PATH_MARKER), "the path was printed: {text}");
}
