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
pub const VLLM_KEY: &str = "vllm-key-small-5f3c0a9e1b7d4c2a8e6f0b1d3c5a7e9f";

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
