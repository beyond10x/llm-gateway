//! Adversary pass 1 on story:credential-set-command: `b10x-llm-gateway credential set` handles a
//! secret. Every endpoint here is a loopback fixture on 127.0.0.1; no key is real.

mod support;

use std::{
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpListener},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::Duration,
};
use support::{Fixture, bearer_keys, hosted_document, hosted_table};

const KEY: &str = "sk-adversary-0123456789-not-a-real-upstream-key";

fn shipped() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
}

/// A fixture with a 0700 `keys` directory and a document whose hosted bearer model `remote` is at
/// `base_url`, storing its key at `keys/remote`.
fn setup(name: &str, base_url: &str) -> (Fixture, PathBuf, PathBuf) {
    let fixture = Fixture::new(&format!("adv-w13-{name}"));
    let keys = fixture.path("keys");
    fs::create_dir(&keys).unwrap();
    fs::set_permissions(&keys, fs::Permissions::from_mode(0o700)).unwrap();
    let target = keys.join("remote");
    let secret = fixture.owner_secret();
    let config = fixture.config(&hosted_document(
        "127.0.0.1:0",
        &secret,
        &hosted_table("remote", base_url, &bearer_keys(&target)),
    ));
    (fixture, config, target)
}

fn run(args: &[&str], input: &[u8], home: &std::path::Path) -> (Option<i32>, String, String) {
    let mut child = Command::new(shipped())
        .args(args)
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let input = input.to_vec();
    let writer = thread::spawn(move || drop(stdin.write_all(&input)));
    let output = child.wait_with_output().unwrap();
    writer.join().unwrap();
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// The output rule: "No line, event or refusal carries the key or any part of it", and the
/// input rule: the key is "never an argument". An owner who passes it as one anyway (the shape
/// `<tool> secret set NAME VALUE` many CLIs take) gets a command-line error, exit 2, and that
/// error must not repeat the key onto standard error, where a CI log or a shared terminal keeps
/// it. clap's "unexpected argument '<value>' found" quotes it whole.
#[test]
fn adversary_credential_set_a_key_given_as_an_argument_is_not_echoed_back() {
    let (fixture, config, target) = setup("argv-echo", "http://127.0.0.1:9/v1");
    let config = config.to_str().unwrap();
    let stdin_eq = format!("--stdin={KEY}");
    for args in [
        &[
            "credential",
            "set",
            "--config",
            config,
            "--model",
            "remote",
            KEY,
        ][..],
        &[
            "credential",
            "set",
            "--config",
            config,
            "--model",
            "remote",
            &stdin_eq,
        ],
    ] {
        let (code, stdout, stderr) = run(args, b"", &fixture.dir);
        assert_eq!(code, Some(2), "{stderr:?}");
        for start in 0..=KEY.len() - 12 {
            let part = &KEY[start..start + 12];
            assert!(
                !stdout.contains(part) && !stderr.contains(part),
                "the command-line error repeats the key given as an argument: {stderr:?}"
            );
        }
    }
    assert!(!target.exists());
}

/// A loopback endpoint that answers an interim `103 Early Hints` and then the final `401`.
fn interim_then(status: u16) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut connection) = connection else {
                return;
            };
            connection
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut raw = Vec::new();
            let mut chunk = [0_u8; 4096];
            while !raw.windows(4).any(|window| window == b"\r\n\r\n") {
                match connection.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => raw.extend_from_slice(&chunk[..read]),
                }
            }
            drop(write!(
                connection,
                "HTTP/1.1 103 Early Hints\r\nlink: </style.css>; rel=preload\r\n\r\n\
                 HTTP/1.1 {status} Fixture\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            ));
        }
    });
    address
}

/// The probe rule: "401 or 403 is `credential:probe-rejected` ... and writes nothing". An HTTP/1.1
/// server may send any number of 1xx interim answers before the final one (RFC 9110 15.2); the
/// probe reads only the first status line, so a key the endpoint rejects with a final 401 after
/// a `103` is stored as `not-rejected http 103`.
#[test]
fn adversary_credential_set_an_interim_1xx_answer_does_not_hide_a_final_401() {
    let address = interim_then(401);
    let (fixture, config, target) = setup("interim-1xx", &format!("http://{address}/v1"));
    let (code, _stdout, stderr) = run(
        &[
            "credential",
            "set",
            "--config",
            config.to_str().unwrap(),
            "--model",
            "remote",
            "--stdin",
        ],
        format!("{KEY}\n").as_bytes(),
        &fixture.dir,
    );
    assert_eq!(code, Some(1), "{stderr:?}");
    assert!(
        stderr.starts_with("b10x-llm-gateway: refused credential:probe-rejected: "),
        "{stderr:?}"
    );
    assert!(!target.exists(), "a key the endpoint rejected was stored");
}
