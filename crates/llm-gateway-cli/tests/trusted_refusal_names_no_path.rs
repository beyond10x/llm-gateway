//! story:trusted-refusal-names-no-path, through the shipped binary: no trusted-file refusal and
//! no log line quotes the path it was given or the file a link resolves to. A value written by
//! mistake into `--config` or a `*_file` setting may be a secret, so every path below holds a
//! marker, and the marker must appear on neither stream while the refusal still names its code,
//! the setting a caller knows and the rule's detail.
//!
//! One case per trusted source: the deployment document, the routes document, the owner secret,
//! a vLLM key, a hosted endpoint's credential, a routes endpoint's secret, the metrics bearer and
//! the container token, at start and at a fetch. Only loopback sockets and files under
//! `CARGO_TARGET_TMPDIR` are used.

mod support;

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use support::{Fixture, OWNER_SECRET, document, mutate};

/// What a secret written by mistake as a path would look like. It is in every path a case gives
/// the binary, and must be in nothing the binary writes.
const MARKER: &str = "sk-path-marker-7f3a9c";

const PREFIX: &str = "b10x-llm-gateway: ";
const LISTENING: &str = "b10x-llm-gateway: listening on ";
const STARTUP: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(10);

const URI_VARIABLE: &str = "AWS_CONTAINER_CREDENTIALS_FULL_URI";
const TOKEN_VARIABLE: &str = "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE";
const CONTAINER_TOKEN: &str = "fake-container-authorization-token-0123456789";
const VALID_TOKEN: &str = "a-valid-test-token-not-a-secret-0123456789";

/// One file that breaks one rule: the path, the `<rule>` half of the code, and a fragment of the
/// rule's detail the message must still carry.
struct Broken {
    path: PathBuf,
    rule: &'static str,
    detail: &'static str,
}

/// A directory whose name holds [`MARKER`], private to this user.
fn marked(fixture: &Fixture, source: &str) -> PathBuf {
    let dir = fixture.path(&format!("{source}-{MARKER}"));
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

fn write(path: &Path, bytes: &[u8], mode: u32) -> PathBuf {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    path.to_owned()
}

/// Files under a [`marked`] directory, each breaking one of the rules a trusted file is held to
/// before it is read: missing, a directory, a final symbolic link, an unsafe mode and over the
/// bound. `valid` is what a good file holds, `limit` the source's bound in bytes and
/// `unsafe_mode` a mode it refuses.
fn broken_files(dir: &Path, valid: &[u8], limit: usize, unsafe_mode: u32) -> Vec<Broken> {
    let linked = write(&dir.join("linked"), valid, 0o600);
    let link = dir.join("link");
    symlink(&linked, &link).unwrap();
    let directory = dir.join("directory");
    std::fs::create_dir(&directory).unwrap();
    vec![
        Broken {
            path: dir.join("absent"),
            rule: "unreadable",
            detail: "No such file or directory",
        },
        Broken {
            path: directory,
            rule: "not-regular",
            detail: "is not a regular file",
        },
        Broken {
            path: link,
            rule: "symlink",
            detail: "is a symbolic link",
        },
        Broken {
            path: write(&dir.join("unsafe"), valid, unsafe_mode),
            rule: "unsafe-mode",
            detail: "has mode",
        },
        Broken {
            path: write(&dir.join("large"), &vec![b'a'; limit + 1], 0o600),
            rule: "too-large",
            detail: "exceeds",
        },
    ]
}

/// A credential file under `dir` that is read, and then refused by its caller's own rule.
fn not_a_token(dir: &Path) -> Broken {
    Broken {
        path: write(&dir.join("two-tokens"), b"one two\n", 0o600),
        rule: "not-a-token",
        detail: "",
    }
}

struct Finished {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn command(args: &[&str], environment: &[(&str, String)]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway"));
    command
        .args(args)
        .env_remove(URI_VARIABLE)
        .env_remove(TOKEN_VARIABLE)
        .env_remove("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI")
        .env_remove("AWS_CONTAINER_AUTHORIZATION_TOKEN")
        .env("RUST_LOG", "debug")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in environment {
        command.env(name, value);
    }
    command
}

/// Runs the binary to its exit; one still running after [`STARTUP`] served where it should
/// have refused.
fn run(config: &Path, environment: &[(&str, String)]) -> Finished {
    let mut child = command(&["--config", config.to_str().unwrap()], environment)
        .spawn()
        .unwrap();
    let drain = |mut reader: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut text = String::new();
            drop(reader.read_to_string(&mut text));
            text
        })
    };
    let stdout = drain(Box::new(child.stdout.take().unwrap()));
    let stderr = drain(Box::new(child.stderr.take().unwrap()));
    let deadline = Instant::now() + STARTUP;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            drop(child.kill());
            drop(child.wait());
            panic!("the gateway served where it should have refused");
        }
        thread::sleep(Duration::from_millis(10));
    };
    Finished {
        code: status.code(),
        stdout: stdout.join().unwrap(),
        stderr: stderr.join().unwrap(),
    }
}

/// Asserts the start was refused `<source>:<rule>`, the refusal names `setting` (when the
/// caller knows one) and the rule's detail, and [`MARKER`] is on neither stream.
fn assert_refused_without_path(
    finished: &Finished,
    source: &str,
    setting: Option<&str>,
    broken: &Broken,
) {
    let code = format!("{source}:{}", broken.rule);
    let start = format!("{PREFIX}refused {code}: ");
    let Some(line) = finished
        .stderr
        .lines()
        .find(|line| line.starts_with(&start))
    else {
        panic!(
            "{}: expected a line starting {start:?}; stderr: {:?}",
            broken.rule, finished.stderr
        );
    };
    assert_eq!(finished.code, Some(1), "{code}: {:?}", finished.stderr);
    if let Some(setting) = setting {
        assert!(line.contains(setting), "{code} names no {setting}: {line}");
    }
    assert!(
        line.contains(broken.detail),
        "{code} carries no {:?}: {line}",
        broken.detail
    );
    for (stream, text) in [("stderr", &finished.stderr), ("stdout", &finished.stdout)] {
        assert!(
            !text.contains(MARKER),
            "{code}: {stream} quotes the path: {text:?}"
        );
    }
}

/// A loopback address nothing will answer on while `_listener` is held.
fn loopback() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    (listener, address)
}

// --- The deployment document (`--config`) ------------------------------------------------------

#[test]
fn the_documents_refusals_name_no_path() {
    let fixture = Fixture::new("no-path-config");
    let dir = marked(&fixture, "config");
    let valid = document("127.0.0.1:0", &fixture.owner_secret());
    let mut cases = broken_files(&dir, valid.as_bytes(), 256 * 1024, 0o666);
    cases.push(Broken {
        path: write(&dir.join("latin1"), b"listen = \"\xff\"\n", 0o600),
        rule: "not-utf8",
        detail: "is not UTF-8",
    });
    for broken in &cases {
        let finished = run(&broken.path, &[]);
        assert_refused_without_path(&finished, "config", None, broken);
    }
}

// --- The routes document (`routes`) ------------------------------------------------------------

#[test]
fn the_routes_documents_refusals_name_no_path() {
    let fixture = Fixture::new("no-path-routes");
    let dir = marked(&fixture, "routes");
    let valid = "format: llm.routes/1\nendpoints:\n  lab: {base_url: http://127.0.0.1:8000/v1, protocol: chat-completions, auth: {kind: anonymous}}\nroutes:\n  routed: lab/small\n";
    let secret = fixture.owner_secret();
    for broken in &broken_files(&dir, valid.as_bytes(), 2 * 1024 * 1024, 0o666) {
        let config = fixture.config(&mutate(
            &document("127.0.0.1:0", &secret),
            "\n\n[providers.runpod]",
            &format!(
                "\nroutes = \"{}\"\n\n[providers.runpod]",
                broken.path.display()
            ),
        ));
        let finished = run(&config, &[]);
        assert_refused_without_path(&finished, "config", Some("routes"), broken);
    }
}

// --- The owner secret (`owner_secret_file`) ----------------------------------------------------

#[test]
fn the_owner_secrets_refusals_name_no_path() {
    let fixture = Fixture::new("no-path-owner-secret");
    let dir = marked(&fixture, "owner-secret");
    let valid = format!("{OWNER_SECRET}\n");
    let mut cases = broken_files(&dir, valid.as_bytes(), 4 * 1024 + 2, 0o640);
    cases.push(not_a_token(&dir));
    cases.push(Broken {
        path: write(
            &dir.join("short"),
            format!("{}\n", "s".repeat(31)).as_bytes(),
            0o600,
        ),
        rule: "too-short",
        detail: "",
    });
    for broken in &cases {
        let config = fixture.config(&document("127.0.0.1:0", &broken.path));
        let finished = run(&config, &[]);
        assert_refused_without_path(&finished, "owner-secret", None, broken);
    }
}

// --- A model's vLLM key (`models.<alias>.vllm_api_key_file`) -----------------------------------

#[test]
fn a_vllm_keys_refusals_name_no_path() {
    let fixture = Fixture::new("no-path-vllm-key");
    let dir = marked(&fixture, "vllm-key");
    let mut cases = broken_files(&dir, format!("{VALID_TOKEN}\n").as_bytes(), 4098, 0o640);
    cases.push(not_a_token(&dir));
    for broken in &cases {
        let config = fixture.config_with_vllm_key("127.0.0.1:0", &broken.path);
        let finished = run(&config, &[]);
        assert_refused_without_path(
            &finished,
            "vllm-api-key",
            Some("models.small.vllm_api_key_file"),
            broken,
        );
    }
}

// --- A hosted endpoint's credential (`hosted.<alias>.credential_file`) -------------------------

#[test]
fn a_hosted_credentials_refusals_name_no_path() {
    let fixture = Fixture::new("no-path-hosted");
    let dir = marked(&fixture, "hosted");
    let mut cases = broken_files(&dir, format!("{VALID_TOKEN}\n").as_bytes(), 4098, 0o640);
    cases.push(not_a_token(&dir));
    let secret = fixture.owner_secret();
    for broken in &cases {
        let config = fixture.config(&format!(
            "listen = \"127.0.0.1:0\"\nowner_secret_file = \"{}\"\n\n[hosted.remote]\nwires = [\"chat\"]\nbase_url = \"http://127.0.0.1:9/\"\nauth_kind = \"bearer\"\ncredential_file = \"{}\"\n",
            secret.display(),
            broken.path.display()
        ));
        let finished = run(&config, &[]);
        assert_refused_without_path(
            &finished,
            "endpoint-credential",
            Some("hosted.remote.credential_file"),
            broken,
        );
    }
}

// --- A routes endpoint's secret (`routes.<endpoint>.auth.secret`) ------------------------------

#[test]
fn a_routes_secrets_refusals_name_no_path() {
    let fixture = Fixture::new("no-path-routes-secret");
    let dir = marked(&fixture, "routes-secret");
    let mut cases = broken_files(&dir, format!("{VALID_TOKEN}\n").as_bytes(), 4098, 0o640);
    cases.push(not_a_token(&dir));
    let secret = fixture.owner_secret();
    for broken in &cases {
        let routes = fixture.write(
            "routes.yaml",
            format!(
                "format: llm.routes/1\nendpoints:\n  lab:\n    base_url: http://127.0.0.1:8000/v1\n    protocol: chat-completions\n    auth: {{kind: bearer, secret: \"file:{}\"}}\nroutes:\n  routed: lab/small\n",
                broken.path.display()
            )
            .as_bytes(),
            0o600,
        );
        let config = fixture.config(&mutate(
            &document("127.0.0.1:0", &secret),
            "\n\n[providers.runpod]",
            &format!("\nroutes = \"{}\"\n\n[providers.runpod]", routes.display()),
        ));
        let finished = run(&config, &[]);
        assert_refused_without_path(
            &finished,
            "endpoint-credential",
            Some("routes.lab.auth.secret"),
            broken,
        );
    }
}

// --- The metrics bearer (`metrics_bearer_file`) ------------------------------------------------

#[test]
fn the_metrics_bearers_refusals_name_no_path() {
    let fixture = Fixture::new("no-path-metrics-bearer");
    let dir = marked(&fixture, "metrics-bearer");
    let mut cases = broken_files(&dir, format!("{VALID_TOKEN}\n").as_bytes(), 4098, 0o640);
    cases.push(not_a_token(&dir));
    cases.push(Broken {
        path: write(&dir.join("short"), b"short-scrape-token\n", 0o600),
        rule: "not-a-token",
        detail: "",
    });
    let secret = fixture.owner_secret();
    let (_held, scrape) = loopback();
    for broken in &cases {
        let config = fixture.config(&mutate(
            &document("127.0.0.1:0", &secret),
            "\n\n[providers.runpod]",
            &format!(
                "\nmetrics_listen = \"{scrape}\"\nmetrics_bearer_file = \"{}\"\n\n[providers.runpod]",
                broken.path.display()
            ),
        ));
        let finished = run(&config, &[]);
        assert_refused_without_path(
            &finished,
            "metrics-bearer",
            Some("metrics_bearer_file"),
            broken,
        );
    }
}

// --- The container token (`AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE`) ----------------------------

/// A document serving one Bedrock model whose endpoint is `bedrock`.
fn bedrock_config(fixture: &Fixture, bedrock: SocketAddr) -> PathBuf {
    let secret = fixture.owner_secret();
    fixture.config(&format!(
        "listen = \"127.0.0.1:0\"\nowner_secret_file = \"{}\"\n\n[hosted.haiku]\nkind = \"bedrock\"\nregion = \"eu-central-1\"\nmodel_id = \"eu.anthropic.claude-haiku-5-5\"\ncredential_source = \"container\"\nendpoint = \"http://{bedrock}\"\n",
        secret.display()
    ))
}

fn container(credentials: SocketAddr, token: &Path) -> Vec<(&'static str, String)> {
    vec![
        (URI_VARIABLE, format!("http://{credentials}/v1/credentials")),
        (TOKEN_VARIABLE, token.display().to_string()),
    ]
}

/// The container token's links are followed, so its symbolic-link rule is a link resolving out
/// of the token path's directory, to a file whose path holds the marker too.
fn outside_link(fixture: &Fixture, dir: &Path, name: &str) -> PathBuf {
    let outside = marked(fixture, &format!("outside-{name}"));
    let target = write(
        &outside.join("token"),
        format!("{CONTAINER_TOKEN}\n").as_bytes(),
        0o644,
    );
    let link = dir.join(name);
    symlink(&target, &link).unwrap();
    link
}

#[test]
fn the_container_tokens_refusals_at_start_name_no_path() {
    let fixture = Fixture::new("no-path-container-token");
    let dir = marked(&fixture, "container-token");
    let (_held_bedrock, bedrock) = loopback();
    let (_held_credentials, credentials) = loopback();
    let config = bedrock_config(&fixture, bedrock);
    let valid = format!("{CONTAINER_TOKEN}\n");
    let mut cases: Vec<Broken> = broken_files(&dir, valid.as_bytes(), 16 * 1024 + 2, 0o666)
        .into_iter()
        .filter(|broken| broken.rule != "symlink")
        .collect();
    cases.push(Broken {
        path: outside_link(&fixture, &dir, "outside-link"),
        rule: "symlink",
        detail: "outside",
    });
    cases.push(Broken {
        path: write(&dir.join("empty"), b"\n", 0o600),
        rule: "not-a-token",
        detail: "",
    });
    for broken in &cases {
        let finished = run(&config, &container(credentials, &broken.path));
        assert_refused_without_path(&finished, "container-token", Some(TOKEN_VARIABLE), broken);
    }
}

/// A served binary: its address, and every line it wrote to standard error before it.
struct Served {
    child: std::process::Child,
    address: SocketAddr,
    lines: mpsc::Receiver<String>,
    log: Vec<String>,
}

impl Served {
    fn start(config: &Path, environment: &[(&str, String)]) -> Self {
        let mut child = command(&["--config", config.to_str().unwrap()], environment)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
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
        let mut log = Vec::new();
        let deadline = Instant::now() + STARTUP;
        let address = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(line) = lines.recv_timeout(left) else {
                drop(child.kill());
                panic!("the binary never reported {LISTENING:?}: {log:?}");
            };
            if let Some(address) = line.strip_prefix(LISTENING) {
                break address.parse().unwrap();
            }
            log.push(line);
        };
        Self {
            child,
            address,
            lines,
            log,
        }
    }

    /// One chat turn for the Bedrock model, which must not be answered 200; the answer.
    fn ask(&self) -> String {
        let body = r#"{"model":"haiku","messages":[{"role":"user","content":"hello"}]}"#;
        let mut stream = TcpStream::connect(self.address).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        stream
            .write_all(
                format!(
                    "POST /v1/chat/completions HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .unwrap();
        let mut answer = Vec::new();
        drop(stream.read_to_end(&mut answer));
        let answer = String::from_utf8_lossy(&answer).into_owned();
        assert!(!answer.starts_with("HTTP/1.1 200 "), "{answer}");
        answer
    }

    /// Stops the binary; every line it wrote to standard error.
    fn log(mut self) -> Vec<String> {
        drop(self.child.kill());
        drop(self.child.wait());
        while let Ok(line) = self.lines.recv_timeout(WAIT) {
            self.log.push(line);
        }
        self.log
    }
}

/// A token file that breaks a rule after the start is refused at the next fetch and logged as a
/// warning: the line names the rule and its detail, never the path or the file a link resolves
/// to.
#[test]
fn the_container_tokens_warnings_at_a_fetch_name_no_path() {
    let fixture = Fixture::new("no-path-container-token-fetch");
    let dir = marked(&fixture, "container-token-fetch");
    let (_held_bedrock, bedrock) = loopback();
    let (_held_credentials, credentials) = loopback();
    let config = bedrock_config(&fixture, bedrock);
    let token = write(
        &dir.join("token"),
        format!("{CONTAINER_TOKEN}\n").as_bytes(),
        0o644,
    );
    let server = Served::start(&config, &container(credentials, &token));
    let mut answers = Vec::new();
    let mut expected = Vec::new();
    // Unsafe mode.
    std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o666)).unwrap();
    answers.push(server.ask());
    expected.push(("container-token:unsafe-mode", "has mode"));
    // Over the bound.
    write(&token, &vec![b'a'; 16 * 1024 + 3], 0o600);
    answers.push(server.ask());
    expected.push(("container-token:too-large", "exceeds"));
    // A directory.
    std::fs::remove_file(&token).unwrap();
    std::fs::create_dir(&token).unwrap();
    answers.push(server.ask());
    expected.push(("container-token:not-regular", "is not a regular file"));
    // A link resolving outside the token's directory.
    std::fs::remove_dir(&token).unwrap();
    let outside = marked(&fixture, "outside-fetch");
    let target = write(
        &outside.join("token"),
        format!("{CONTAINER_TOKEN}\n").as_bytes(),
        0o644,
    );
    symlink(&target, &token).unwrap();
    answers.push(server.ask());
    expected.push(("container-token:symlink", "outside"));
    // Missing.
    std::fs::remove_file(&token).unwrap();
    answers.push(server.ask());
    expected.push(("container-token:unreadable", "No such file or directory"));
    let log = server.log();
    for (code, detail) in expected {
        assert!(
            log.iter()
                .any(|line| line.contains("WARN") && line.contains(code) && line.contains(detail)),
            "no warning names {code} with {detail:?}: {log:?}"
        );
    }
    for line in log.iter().chain(&answers) {
        assert!(!line.contains(MARKER), "the path was written: {line}");
        assert!(
            !line.contains(CONTAINER_TOKEN),
            "the token was written: {line}"
        );
    }
}
