//! Adversarial cases for the Run on Kubernetes guide: its manifests' init script, its deployment
//! document and its stop, each driven from the page's own text. Only loopback sockets and files
//! under `CARGO_TARGET_TMPDIR` are used; no cluster is needed.

use rustix::process::{Pid, Signal, getegid, geteuid, kill_process};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::{SocketAddr, TcpStream},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::Duration,
};

const GUIDE: &str = include_str!("../../../website/docs/guides/run-on-kubernetes.md");
const LISTENING: &str = "b10x-llm-gateway: listening on ";
/// Long enough for the gateway's accept poll and for a signal to be acted on.
const SETTLE: Duration = Duration::from_millis(300);

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("adversary-k8s-{name}-{}", std::process::id()));
    drop(fs::remove_dir_all(&dir));
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

/// The lines of a YAML block scalar that follows the line `opener`, with the block's indentation
/// removed, up to the first line indented less than the block's first line.
fn block_after(opener: &str) -> String {
    let mut lines = GUIDE
        .lines()
        .skip_while(|line| line.trim() != opener)
        .skip(1);
    let first = lines
        .next()
        .unwrap_or_else(|| panic!("the guide has no {opener:?} block"));
    let indent = first.len() - first.trim_start().len();
    let mut text = format!("{}\n", &first[indent..]);
    for line in lines {
        if line.trim().is_empty() {
            text.push('\n');
            continue;
        }
        if line.len() - line.trim_start().len() < indent || line.starts_with("---") {
            break;
        }
        text.push_str(line.get(indent..).unwrap_or(""));
        text.push('\n');
    }
    text
}

/// The init container's script with its two volume paths moved under `dir` and its target owner
/// made the user running the test: that user, owning a `0400` file without `CAP_DAC_OVERRIDE`,
/// stands where the init container's root stands with only `CAP_CHOWN`.
fn init_script(input: &Path, output: &Path) -> String {
    block_after("- |")
        .lines()
        .take_while(|line| !line.starts_with("securityContext:"))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("\"/in/", &format!("\"{}/", input.display()))
        .replace("\"/run/llm-gateway/", &format!("\"{}/", output.display()))
        .replace(
            "65532:65532",
            &format!("{}:{}", geteuid().as_raw(), getegid().as_raw()),
        )
}

/// Runs the script as the init container would: as root, `CAP_CHOWN` is the only capability.
fn run_init(script: &str) -> Output {
    let mut command = if geteuid().is_root() {
        let mut command = Command::new("setpriv");
        command.args(["--bounding-set", "-all,+chown", "--", "sh", "-c", script]);
        command
    } else {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    };
    command.stdin(Stdio::null()).output().unwrap()
}

/// The Secret volume's file and the `emptyDir`, as the pod has them under an `fsGroup`: the file
/// `0440`, the directory world-writable with the set-group-id bit.
fn volumes(dir: &Path) -> (PathBuf, PathBuf) {
    let input = dir.join("in");
    let output = dir.join("run");
    fs::create_dir_all(&input).unwrap();
    fs::create_dir_all(&output).unwrap();
    fs::set_permissions(&output, fs::Permissions::from_mode(0o2777)).unwrap();
    let secret = input.join("owner-secret");
    fs::write(&secret, b"owner-secret-0123456789abcdefghijklmnop\n").unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o440)).unwrap();
    (input, output)
}

#[test]
fn the_guides_init_script_runs_again_over_its_own_copy() {
    // Kubernetes runs every init container again when the pod sandbox is recreated, and an
    // `emptyDir` lives as long as the pod, so the second run finds the `0400` copy it made.
    let dir = scratch("init-rerun");
    let (input, output) = volumes(&dir);
    let script = init_script(&input, &output);
    let first = run_init(&script);
    assert!(first.status.success(), "first run: {first:?}");
    let copy = fs::metadata(output.join("owner-secret")).unwrap();
    assert_eq!(copy.mode() & 0o7777, 0o400, "the copy is 0400");
    let second = run_init(&script);
    assert!(
        second.status.success(),
        "the guide's init script fails when it runs again over the emptyDir it filled: {}",
        String::from_utf8_lossy(&second.stderr)
    );
}

#[test]
fn the_guides_document_starts_on_the_copy_and_prints_the_pasted_line() {
    let dir = scratch("document");
    let (input, output) = volumes(&dir);
    let first = run_init(&init_script(&input, &output));
    assert!(first.status.success(), "{first:?}");
    let document = block_after("gateway.toml: |")
        .replace("\"0.0.0.0:8080\"", "\"127.0.0.1:0\"")
        .replace("\"/run/llm-gateway/", &format!("\"{}/", output.display()));
    let config = dir.join("gateway.toml");
    fs::write(&config, document).unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o644)).unwrap();
    let mut child = Command::new(binary())
        .args(["--config", config.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    drop(child.kill());
    drop(child.wait());
    let address = line
        .trim_end()
        .strip_prefix(LISTENING)
        .unwrap_or_else(|| panic!("the page's document did not start: {line:?}"));
    let printed = line.trim_end().replace(address, "0.0.0.0:8080");
    assert!(
        GUIDE.lines().any(|candidate| candidate == printed),
        "the guide must paste {printed:?} on a line of its own"
    );
}

#[test]
fn the_guides_pasted_command_names_the_document_its_deployment_mounts() {
    let command = GUIDE
        .lines()
        .find(|line| line.starts_with("b10x-llm-gateway --config "))
        .expect("the guide pastes the command it ran");
    let path = command.trim_start_matches("b10x-llm-gateway --config ");
    assert!(
        GUIDE.contains(&format!("\"--config\", \"{path}\"")),
        "the guide pastes `{command}`, but its Deployment passes another document path"
    );
}

struct Gateway {
    child: Child,
    address: SocketAddr,
}

fn serve(dir: &Path) -> Gateway {
    let secret = dir.join("owner-secret");
    fs::write(&secret, b"owner-secret-0123456789abcdefghijklmnop\n").unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o400)).unwrap();
    let config = dir.join("gateway.toml");
    fs::write(
        &config,
        block_after("gateway.toml: |")
            .replace("\"0.0.0.0:8080\"", "\"127.0.0.1:0\"")
            .replace(
                "\"/run/llm-gateway/owner-secret\"",
                &format!("\"{}\"", secret.display()),
            ),
    )
    .unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    let mut child = Command::new(binary())
        .args(["--config", config.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let address = line
        .trim_end()
        .strip_prefix(LISTENING)
        .unwrap_or_else(|| panic!("did not start: {line:?}"))
        .parse()
        .unwrap();
    Gateway { child, address }
}

#[test]
fn a_connection_made_after_sigterm_is_refused_while_an_accepted_one_drains() {
    // The guide: "A connection made after the signal is not accepted". A client the Service
    // still routes during the stop should be refused at once, not held until the drain ends.
    let dir = scratch("after-sigterm");
    let mut gateway = serve(&dir);
    let mut held = TcpStream::connect(gateway.address).unwrap();
    held.write_all(b"GET /ready HTTP/1.1\r\nHost: gateway\r\n")
        .unwrap();
    thread::sleep(SETTLE);
    kill_process(Pid::from_child(&gateway.child), Signal::TERM).unwrap();
    thread::sleep(SETTLE);
    let late = TcpStream::connect_timeout(&gateway.address, Duration::from_secs(2));
    held.write_all(b"\r\n").unwrap();
    drop(gateway.child.wait());
    assert!(
        late.is_err(),
        "a connection made after SIGTERM, while an accepted one drains, completed its handshake \
         with the gateway's still-open listener"
    );
}
