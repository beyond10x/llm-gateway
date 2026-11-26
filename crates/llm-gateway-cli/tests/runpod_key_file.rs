//! story:runpod-key-file: a provider's Runpod API key read from the file `api_key_file` names,
//! under the trusted-file rules a model's `vllm_api_key_file` follows. The composition the
//! shipped binary runs (`start_connected`) is driven against the loopback fake Runpod of
//! `runpod_host`; the owner rule runs the binary itself inside a user namespace. Every key is a
//! test literal in a fixture file: no keychain is read, no pod is started and nothing leaves the
//! loopback interface.

use crate::{
    runpod_host::{Fake, Pods, both_at},
    support::{Fixture, SECRET_LIMIT, VLLM_KEY},
};
use llm_gateway_cli::{Refusal, parse, start_connected};
use std::{
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const FILE_KEY: &str = "rpa_runpod-key-from-a-test-file";

/// The file `api_key_file` names: [`FILE_KEY`] and the newline an editor adds, mode 0600.
fn key_file(fixture: &Fixture) -> PathBuf {
    fixture.write("runpod-key", format!("{FILE_KEY}\n").as_bytes(), 0o600)
}

/// A gateway with no ingress credential, one Runpod provider whose key is the file `key` and the
/// provider lines `lines`, and one model `small` whose vLLM key is the fixture's file.
fn document(fixture: &Fixture, key: &Path, lines: &str) -> String {
    format!(
        "listen = \"127.0.0.1:0\"\ningress_auth = \"none\"\n\n[providers.runpod]\n\
         kind = \"runpod-vllm\"\napi_key_file = \"{}\"\n{lines}\n\
         [models.small]\nprovider = \"runpod\"\nwires = [\"chat\"]\n\
         context_window = 65536\nhf_model = \"example/small-model\"\n\
         image = \"vllm/vllm-openai:v0.27.1\"\ngpu_types = [\"NVIDIA L40S\"]\n\
         max_model_len = 1024\nvllm_api_key_file = \"{}\"\n",
        key.display(),
        fixture.vllm_key().display(),
    )
}

/// Starts the composition for `key` against a fresh fake and returns its refusal; asserts the
/// fake received no request.
fn refused_start(fixture: &Fixture, key: &Path) -> Refusal {
    let fake = Fake::start();
    let pods = Pods::new(&fake, vec![".pods.fake.test"]);
    let deployment = parse(&document(fixture, key, &both_at(&fake))).unwrap();
    let Err(refusal) = start_connected(&deployment, |transport| transport, pods) else {
        panic!("the start was not refused");
    };
    assert!(
        fake.seen().is_empty(),
        "a refused start sent {:?}",
        fake.seen()
            .into_iter()
            .map(|seen| seen.line)
            .collect::<Vec<_>>()
    );
    refusal
}

/// Asserts `refusal` is `code`, its message starts `providers.runpod.api_key_file:` and quotes
/// neither the fixture's directory nor `material`.
fn assert_rule(fixture: &Fixture, refusal: &Refusal, code: &str, material: &str) {
    assert_eq!(refusal.code(), code, "{refusal}");
    assert!(
        refusal
            .message()
            .starts_with("providers.runpod.api_key_file: "),
        "{refusal}"
    );
    let text = refusal.to_string();
    assert!(
        !text.contains(&fixture.dir.display().to_string()),
        "the refusal quotes the path: {text}"
    );
    if !material.trim().is_empty() {
        assert!(
            !text.contains(material.trim()),
            "the refusal quotes the key: {text}"
        );
    }
}

/// Acceptance 1: `api_key_file` is read from the document as a path; it is refused beside
/// `api_key` and beside `connectors`, naming `providers.<name>.api_key_file` and quoting nothing
/// written, and counts as `api_key` for "at most one provider declares `connectors` or
/// `api_key`".
#[test]
fn runpod_key_file_is_refused_beside_api_key_and_connectors_and_counts_as_api_key() {
    let fixture = Fixture::new("runpod-key-file-document");
    let key = key_file(&fixture);
    let text = document(&fixture, &key, "");
    let deployment = parse(&text).unwrap();
    let provider = &deployment.providers["runpod"];
    assert_eq!(provider.api_key_file.as_deref(), Some(key.as_path()));
    assert_eq!(provider.api_key, None);
    let keychain = "api_key = \"keychain:secret-service/secret-account\"\n";
    let connectors = "\n[providers.runpod.connectors]\nexecutable = \"/usr/bin/connectors\"\n\
                      adapter = \"runpod\"\nconnection = \"main\"\nwork_directory = \"/var/empty\"\n";
    let beside_connectors =
        text.replace("\n[models.small]", &format!("{connectors}\n[models.small]"));
    for (name, refused) in [
        ("beside api_key", document(&fixture, &key, keychain)),
        ("beside connectors", beside_connectors),
    ] {
        let refusal = parse(&refused).unwrap_err();
        assert_eq!(refusal.code(), "config:value", "{name}: {refusal}");
        assert!(
            refusal
                .message()
                .starts_with("providers.runpod.api_key_file "),
            "{name}: {refusal}"
        );
        let shown = refusal.to_string();
        assert!(!shown.contains("secret"), "{name}: {shown}");
        assert!(
            !shown.contains(&fixture.dir.display().to_string()),
            "{name}: {shown}"
        );
    }
    for (name, other) in [
        ("api_key", keychain.to_owned()),
        (
            "api_key_file",
            format!("api_key_file = \"{}\"\n", key.display()),
        ),
        (
            "connectors",
            connectors.replace("runpod.connectors", "other.connectors"),
        ),
    ] {
        let second = format!("\n[providers.other]\nkind = \"runpod-vllm\"\n{other}");
        let refused = text.replace("\n[models.small]", &format!("{second}\n[models.small]"));
        let refusal = parse(&refused).unwrap_err();
        assert_eq!(refusal.code(), "config:value", "{name}: {refusal}");
        assert!(
            refusal.message().starts_with("providers "),
            "{name}: {refusal}"
        );
    }
    // `management_url` stands beside a key file as beside a keychain key.
    parse(&document(
        &fixture,
        &key,
        "management_url = \"http://127.0.0.1:9/v2\"\n",
    ))
    .unwrap();
}

/// Acceptance 3: a key read from the file (its trailing newline trimmed) reaches the fake
/// Runpod REST API as `Authorization: Bearer <key>` on the start's pod listing.
#[test]
fn runpod_key_file_a_key_read_from_the_file_reaches_the_rest_api_as_a_bearer() {
    let fixture = Fixture::new("runpod-key-file-bearer");
    let fake = Fake::start();
    let pods = Pods::new(&fake, vec![".pods.fake.test"]);
    let deployment = parse(&document(&fixture, &key_file(&fixture), &both_at(&fake))).unwrap();
    let relaying = start_connected(&deployment, |transport| transport, pods).unwrap();
    relaying.shutdown();
    let seen = fake.seen();
    let management: Vec<_> = seen
        .iter()
        .filter(|seen| seen.line.contains(" /v2/"))
        .collect();
    assert!(
        management
            .iter()
            .any(|seen| seen.line.starts_with("GET /v2/pods")),
        "{seen:?}",
        seen = seen.iter().map(|seen| &seen.line).collect::<Vec<_>>()
    );
    for call in management {
        assert!(
            call.head.contains(&format!(
                "\r\nauthorization: bearer {}\r\n",
                FILE_KEY.to_ascii_lowercase()
            )) || call.head.ends_with(&format!(
                "\r\nauthorization: bearer {}",
                FILE_KEY.to_ascii_lowercase()
            )),
            "{}: {}",
            call.line,
            call.head
        );
    }
}

/// Acceptance 3: a missing key file refuses the start as `endpoint-credential:unreadable` and
/// no request reaches the fake.
#[test]
fn runpod_key_file_a_missing_file_refuses_the_start_with_no_request_sent() {
    let fixture = Fixture::new("runpod-key-file-missing");
    let refusal = refused_start(&fixture, &fixture.path("absent-key"));
    assert_rule(&fixture, &refusal, "endpoint-credential:unreadable", "");
}

/// Acceptance 2 and 3, the open rule: a file the process may not open.
#[test]
fn runpod_key_file_an_unopenable_file_is_unreadable() {
    let fixture = Fixture::new("runpod-key-file-unopenable");
    let key = fixture.write("runpod-key", format!("{FILE_KEY}\n").as_bytes(), 0o000);
    let refusal = refused_start(&fixture, &key);
    assert_rule(
        &fixture,
        &refusal,
        "endpoint-credential:unreadable",
        FILE_KEY,
    );
}

/// Acceptance 2 and 3, the symlink rule: the final component is not followed.
#[test]
fn runpod_key_file_a_symbolic_link_is_refused() {
    let fixture = Fixture::new("runpod-key-file-symlink");
    let link = fixture.path("runpod-key-link");
    symlink(key_file(&fixture), &link).unwrap();
    let refusal = refused_start(&fixture, &link);
    assert_rule(&fixture, &refusal, "endpoint-credential:symlink", FILE_KEY);
}

/// Acceptance 2 and 3, the regular-file rule.
#[test]
fn runpod_key_file_a_directory_is_not_regular() {
    let fixture = Fixture::new("runpod-key-file-directory");
    let refusal = refused_start(&fixture, &fixture.dir);
    assert_rule(&fixture, &refusal, "endpoint-credential:not-regular", "");
}

/// Acceptance 2 and 3, the mode rule: any group or world bit (mode & 0o077) refuses the file.
#[test]
fn runpod_key_file_a_group_or_world_bit_is_unsafe() {
    let fixture = Fixture::new("runpod-key-file-mode");
    for mode in [0o640, 0o604, 0o620, 0o602, 0o610, 0o601] {
        let key = fixture.write("runpod-key", format!("{FILE_KEY}\n").as_bytes(), mode);
        let refusal = refused_start(&fixture, &key);
        assert_rule(
            &fixture,
            &refusal,
            "endpoint-credential:unsafe-mode",
            FILE_KEY,
        );
    }
}

/// Acceptance 2 and 3, the size bound: over the 4098-byte file bound, and a 4097-byte key inside
/// it once trimmed. A 4096-byte key and its CRLF is admitted.
#[test]
fn runpod_key_file_a_key_over_the_bound_is_too_large() {
    let fixture = Fixture::new("runpod-key-file-size");
    for material in ["k".repeat(SECRET_LIMIT + 3), "k".repeat(SECRET_LIMIT + 1)] {
        let key = fixture.write("runpod-key", material.as_bytes(), 0o600);
        let refusal = refused_start(&fixture, &key);
        assert_rule(
            &fixture,
            &refusal,
            "endpoint-credential:too-large",
            &material,
        );
    }
    let fake = Fake::start();
    let pods = Pods::new(&fake, vec![".pods.fake.test"]);
    let key = fixture.write(
        "runpod-key",
        format!("{}\r\n", "k".repeat(SECRET_LIMIT)).as_bytes(),
        0o600,
    );
    let deployment = parse(&document(&fixture, &key, &both_at(&fake))).unwrap();
    start_connected(&deployment, |transport| transport, pods)
        .unwrap()
        .shutdown();
}

/// Acceptance 2 and 3, the text rule: the file is UTF-8.
#[test]
fn runpod_key_file_bytes_that_are_not_utf8_are_refused() {
    let fixture = Fixture::new("runpod-key-file-utf8");
    let key = fixture.write("runpod-key", b"rpa_key-\xff\xfe\n", 0o600);
    let refusal = refused_start(&fixture, &key);
    assert_rule(&fixture, &refusal, "endpoint-credential:not-utf8", "");
}

/// Acceptance 2 and 3, the token rule: one non-empty printable ASCII token.
#[test]
fn runpod_key_file_a_value_that_is_not_one_token_is_refused() {
    let fixture = Fixture::new("runpod-key-file-token");
    for material in [
        "\n".to_owned(),
        format!("{FILE_KEY} {FILE_KEY}\n"),
        format!("{FILE_KEY}\0"),
        format!("{FILE_KEY}\r\nInjected: header\n"),
    ] {
        let key = fixture.write("runpod-key", material.as_bytes(), 0o600);
        let refusal = refused_start(&fixture, &key);
        assert_rule(
            &fixture,
            &refusal,
            "endpoint-credential:not-a-token",
            &material,
        );
    }
}

/// Acceptance 2 and 3, the owner rule: inside a user namespace that maps this user to uid 4242,
/// a root-owned file appears owned by the overflow uid, neither the effective user nor root. The
/// shipped binary refuses its start naming the key; its management URL is a closed loopback
/// port, which a refused start never reaches.
#[test]
fn runpod_key_file_a_file_owned_by_neither_this_user_nor_root_is_refused() {
    let namespaces = Command::new("unshare")
        .args(["--user", "--map-user=4242", "true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !namespaces {
        eprintln!("SKIPPED: this host refuses unprivileged user namespaces");
        return;
    }
    let fixture = Fixture::new("runpod-key-file-owner");
    let config = fixture.config(&document(
        &fixture,
        Path::new("/etc/passwd"),
        "management_url = \"http://127.0.0.1:9/v2\"\n",
    ));
    let output = Command::new("unshare")
        .args(["--user", "--map-user=4242"])
        .arg(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
        .args(["--config", config.to_str().unwrap()])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "b10x-llm-gateway: refused endpoint-credential:untrusted-owner: \
             providers.runpod.api_key_file: "
        ),
        "{stderr:?}"
    );
    assert!(!stderr.contains("/etc/passwd"), "{stderr:?}");
    assert_eq!(output.status.code(), Some(1), "{stderr:?}");
    // Control: the vLLM key file of the same document, which this user owns, passed the rule.
    assert!(!stderr.contains("vllm-api-key"), "{stderr:?}");
    assert!(!stderr.contains(VLLM_KEY), "{stderr:?}");
}
