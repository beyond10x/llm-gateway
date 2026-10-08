//! Fixtures shared by the row-named tests: a private directory per test, a valid deployment
//! document and an owner secret. Everything is written under `CARGO_TARGET_TMPDIR`.

#![allow(dead_code)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

/// 39 bytes: above the verifier's 32-byte minimum. Written with a trailing newline, which the
/// reader trims.
pub const OWNER_SECRET: &str = "owner-secret-0123456789abcdefghijklmnop";

/// A model's vLLM key (row B9). Written with a trailing newline, which the reader trims.
pub const VLLM_KEY: &str = "small-model-test-token-not-a-secret";

/// The deployment document's size bound, llmgw `src/config.rs:16`.
pub const CONFIG_LIMIT: usize = 256 * 1024;

/// The owner secret's size bound, llmgw `src/config.rs:17`.
pub const SECRET_LIMIT: usize = 4 * 1024;

pub struct Fixture {
    pub dir: PathBuf,
}

impl Fixture {
    pub fn new(name: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("gateway-cli-{name}-{}", std::process::id()));
        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        Self { dir }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Writes `bytes` and then sets exactly `mode`, so the umask plays no part.
    pub fn write(&self, name: &str, bytes: &[u8], mode: u32) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    pub fn owner_secret(&self) -> PathBuf {
        self.write(
            "owner-secret",
            format!("{OWNER_SECRET}\n").as_bytes(),
            0o600,
        )
    }

    /// [`VLLM_KEY`] and its newline at `vllm-key`, mode 0600.
    pub fn vllm_key(&self) -> PathBuf {
        self.write("vllm-key", format!("{VLLM_KEY}\n").as_bytes(), 0o600)
    }

    /// A valid document whose model `small` names `key` as its `vllm_api_key_file`.
    pub fn config_with_vllm_key(&self, listen: &str, key: &Path) -> PathBuf {
        let secret = self.owner_secret();
        self.config(&with_vllm_key(&document(listen, &secret), key))
    }

    pub fn config(&self, text: &str) -> PathBuf {
        self.write("gateway.toml", text.as_bytes(), 0o600)
    }

    /// A valid document listening on `listen`, with this fixture's owner secret.
    pub fn valid_config(&self, listen: &str) -> PathBuf {
        let secret = self.owner_secret();
        self.config(&document(listen, &secret))
    }
}

/// The smallest valid deployment document: one provider, one model, every defaulted key
/// omitted. `max_model_len` sits at its lower bound and `context_window` well above its own, so
/// a mutation of one of them breaks exactly one rule.
pub fn document(listen: &str, owner_secret_file: &Path) -> String {
    format!(
        r#"listen = "{listen}"
owner_secret_file = "{}"

[providers.runpod]
kind = "runpod-vllm"

[models.small]
provider = "runpod"
wires = ["chat", "responses"]
context_window = 65536
hf_model = "example/small-model"
image = "vllm/vllm-openai:v0.27.1"
gpu_types = ["NVIDIA L40S"]
max_model_len = 1024
"#,
        owner_secret_file.display()
    )
}

/// `text` (a [`document`]) with `vllm_api_key_file = "<key>"` in its `[models.small]` table.
pub fn with_vllm_key(text: &str, key: &Path) -> String {
    mutate(
        text,
        "max_model_len = 1024\n",
        &format!(
            "max_model_len = 1024\nvllm_api_key_file = \"{}\"\n",
            key.display()
        ),
    )
}

/// `text` with `find` replaced once. Panics when `find` is absent, so a case whose mutation
/// silently stopped applying fails instead of passing on the unmutated document.
pub fn mutate(text: &str, find: &str, replace: &str) -> String {
    assert!(
        text.contains(find),
        "the mutation target {find:?} is absent from the document, so this case tests nothing"
    );
    text.replacen(find, replace, 1)
}

/// `text` padded with one comment line to exactly `size` bytes.
pub fn padded(text: &str, size: usize) -> String {
    let room = size - text.len();
    assert!(room >= 2, "the document is already {} bytes", text.len());
    let mut out = String::with_capacity(size);
    out.push_str(text);
    out.push('#');
    out.push_str(&"x".repeat(room - 2));
    out.push('\n');
    assert_eq!(out.len(), size);
    out
}

/// `text` (a [`document`]) with a `[providers.runpod.connectors]` table holding `lines`.
pub fn with_connectors(text: &str, lines: &str) -> String {
    mutate(
        text,
        "\n[models.small]\n",
        &format!("\n[providers.runpod.connectors]\n{lines}\n[models.small]\n"),
    )
}

/// A complete `connectors` table naming `executable` and `work_directory`, adapter `gpu`,
/// connection `conn-1`, with `timeout_seconds` left to its default.
pub fn connectors_table(executable: &Path, work_directory: &Path) -> String {
    format!(
        "executable = \"{}\"\nadapter = \"gpu\"\nconnection = \"conn-1\"\nwork_directory = \"{}\"\n",
        executable.display(),
        work_directory.display()
    )
}

/// A fixture of the `connectors` CLI (`crates/llm-runpod/tests/fixture/connectors.rs`) in its own
/// directory: the `connectors` link to the fixture program, its `script.json` and the
/// `calls.jsonl` it appends one line to per invocation, and a private `work` directory.
pub struct Connectors {
    pub dir: PathBuf,
}

impl Connectors {
    pub fn new(fixture: &Fixture) -> Self {
        let dir = fixture.path("connectors-fixture");
        fs::create_dir_all(dir.join("work")).unwrap();
        fs::set_permissions(dir.join("work"), fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(
            env!("CARGO_BIN_EXE_gateway-connectors-fixture"),
            dir.join("connectors"),
        )
        .unwrap();
        Self { dir }
    }

    pub fn executable(&self) -> PathBuf {
        self.dir.join("connectors")
    }

    pub fn work(&self) -> PathBuf {
        self.dir.join("work")
    }

    /// The document's `connectors` table for this fixture.
    pub fn table(&self) -> String {
        connectors_table(&self.executable(), &self.work())
    }

    pub fn script(&self, script: &serde_json::Value) {
        fs::write(self.dir.join("script.json"), script.to_string()).unwrap();
    }

    pub fn calls(&self) -> Vec<serde_json::Value> {
        fs::read_to_string(self.dir.join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// The calls whose key, `<command> <operation>`, is `key`.
    pub fn calls_of(&self, key: &str) -> Vec<serde_json::Value> {
        self.calls()
            .into_iter()
            .filter(|call| call["key"] == serde_json::json!(key))
            .collect()
    }
}

/// A successful `operations invoke pods.list` answer listing `pods`.
pub fn listed(pods: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({"stdout": {
        "adapter": "gpu", "operation": "pods.list", "revision": "revision-7",
        "result": {"status": 200, "body": pods},
    }})
}

/// An applied `operations invoke pod.create` answer with the created `pod`.
pub fn created(pod: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({"stdout": {
        "adapter": "gpu", "operation": "pod.create", "revision": "revision-7",
        "result": {"status": 200, "body": pod},
        "mutation": {"classification": "applied", "replayed": false},
    }})
}

/// A running pod of the model `small` as Runpod's `pods.list` answers it. It carries no owner
/// tag, so the startup sweep leaves it alone, and the pool adopts it by its id once its create
/// answers with it.
pub fn running_small_pod() -> serde_json::Value {
    serde_json::json!({
        "id": "podsmall1", "name": "b10x-llm-small", "desiredStatus": "RUNNING",
        "env": {}, "gpu": {"id": "NVIDIA L40S", "count": 1},
        "lastStartedAt": "2026-10-08T10:00:00.000Z",
    })
}

/// [`document`] with the model `small` relayed through `connectors`: its vLLM key file, two GPU
/// types tried in order, a 30-second startup deadline and the request hold `hold_seconds`.
pub fn connected_document(
    fixture: &Fixture,
    listen: &str,
    connectors: &Connectors,
    hold_seconds: u64,
) -> String {
    let secret = fixture.owner_secret();
    let key = fixture.vllm_key();
    let text = with_vllm_key(&document(listen, &secret), &key);
    let text = mutate(
        &text,
        "gpu_types = [\"NVIDIA L40S\"]",
        "gpu_types = [\"NVIDIA L40S\", \"NVIDIA A40\"]",
    );
    let text = mutate(
        &text,
        "max_model_len = 1024\n",
        &format!(
            "max_model_len = 1024\nstart_wait_seconds = 30\nrequest_hold_seconds = {hold_seconds}\n"
        ),
    );
    with_connectors(&text, &connectors.table())
}
