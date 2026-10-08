//! Adversary cases for story:provider-key-files (rows B9 and K7). Red cases assert what the
//! code's own documentation, the matrix or the specification promise; green cases pin a rule no
//! other test in this crate would catch a mutation of.

mod support;

use llm_gateway_cli::{load, vllm_keys};
use std::{fs, os::unix::fs::symlink, path::Path};
use support::{Fixture, VLLM_KEY, document, mutate, with_vllm_key};

fn root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// A second model `name` appended to `text`, naming `key` as its `vllm_api_key_file`.
fn with_model(text: &str, name: &str, key: Option<&Path>) -> String {
    let key = key.map_or(String::new(), |key| {
        format!("vllm_api_key_file = \"{}\"\n", key.display())
    });
    format!(
        "{text}\n[models.{name}]\nprovider = \"runpod\"\nwires = [\"chat\"]\n\
         context_window = 65536\nhf_model = \"example/{name}-model\"\n\
         image = \"vllm/vllm-openai:v0.27.1\"\ngpu_types = [\"NVIDIA L40S\"]\n\
         max_model_len = 1024\n{key}"
    )
}

fn refusal_of(fixture: &Fixture, key: &Path) -> llm_gateway_cli::Refusal {
    let text = with_vllm_key(&document("127.0.0.1:0", &fixture.owner_secret()), key);
    let deployment = load(&fixture.config(&text)).unwrap();
    vllm_keys(&deployment).unwrap_err()
}

/// `vllm_keys`'s documentation: "The message names the model and the file, never the bytes."
/// The trusted-file rules' refusals name only the file.
#[test]
fn adv_b9_every_vllm_key_refusal_names_the_model() {
    let fixture = Fixture::new("adv-w02-names-model");
    let real = fixture.vllm_key();
    let link = fixture.path("key-link");
    symlink(&real, &link).unwrap();
    let cases = [
        ("vllm-api-key:symlink", link),
        ("vllm-api-key:unreadable", fixture.path("absent")),
        ("vllm-api-key:not-regular", fixture.dir.clone()),
    ];
    for (code, path) in cases {
        let refusal = refusal_of(&fixture, &path);
        assert_eq!(refusal.code(), code);
        assert!(
            refusal.message().contains("models.small"),
            "{code} does not name the model: {refusal}"
        );
    }
}

#[test]
fn adv_b9_two_models_sharing_one_key_file_each_hold_its_value() {
    let fixture = Fixture::new("adv-w02-shared");
    let key = fixture.vllm_key();
    let text = with_vllm_key(&document("127.0.0.1:0", &fixture.owner_secret()), &key);
    let text = with_model(&text, "large", Some(&key));
    let keys = vllm_keys(&load(&fixture.config(&text)).unwrap()).unwrap();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys.get("small").unwrap().expose(), VLLM_KEY.as_bytes());
    assert_eq!(keys.get("large").unwrap().expose(), VLLM_KEY.as_bytes());
}

/// The first broken file in alias order is the one refused, as the documentation says.
#[test]
fn adv_b9_the_first_broken_key_in_alias_order_is_refused() {
    let fixture = Fixture::new("adv-w02-order");
    let bad_a = fixture.write("key-a", b"two words\n", 0o600);
    let bad_z = fixture.write("key-z", b"k\n", 0o644);
    let text = document("127.0.0.1:0", &fixture.owner_secret());
    let text = with_model(&text, "zeta", Some(&bad_z));
    let text = with_model(&text, "alpha", Some(&bad_a));
    let refusal = vllm_keys(&load(&fixture.config(&text)).unwrap()).unwrap_err();
    assert_eq!(refusal.code(), "vllm-api-key:not-a-token", "{refusal}");
}

#[test]
fn adv_b9_a_key_file_with_no_permission_at_all_is_unreadable() {
    if rustix::process::geteuid().is_root() {
        eprintln!("SKIPPED: root reads a mode-000 file");
        return;
    }
    let fixture = Fixture::new("adv-w02-mode-000");
    let key = fixture.write("vllm-key", format!("{VLLM_KEY}\n").as_bytes(), 0o000);
    let refusal = refusal_of(&fixture, &key);
    assert_eq!(refusal.code(), "vllm-api-key:unreadable", "{refusal}");
    fs::set_permissions(&key, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
}

/// A bearer value cannot carry a byte that ends or splits a header; trailing ASCII whitespace
/// (including a form feed) is trimmed, anything else outside `!..=~` is refused.
#[test]
fn adv_b9_header_breaking_bytes_are_refused_and_the_refusal_quotes_no_key() {
    let fixture = Fixture::new("adv-w02-header");
    for material in [
        format!(" {VLLM_KEY}\n"),
        format!("\t{VLLM_KEY}"),
        format!("{VLLM_KEY}\x7f"),
        format!("{VLLM_KEY}\x0b"),
        format!("{VLLM_KEY}\rX-Injected: 1"),
        format!("{VLLM_KEY}\u{a0}"),
        format!("{VLLM_KEY}{}", "k".repeat(4097 - VLLM_KEY.len())),
    ] {
        let key = fixture.write("vllm-key", material.as_bytes(), 0o600);
        let refusal = refusal_of(&fixture, &key);
        assert!(
            refusal.code().starts_with("vllm-api-key:"),
            "{material:?}: {refusal}"
        );
        for printed in [refusal.to_string(), format!("{refusal:?}")] {
            assert!(!printed.contains(VLLM_KEY), "{printed}");
        }
    }
    let key = fixture.write(
        "vllm-key",
        format!("{VLLM_KEY}\x0c\r\n\t ").as_bytes(),
        0o600,
    );
    let text = with_vllm_key(&document("127.0.0.1:0", &fixture.owner_secret()), &key);
    let keys = vllm_keys(&load(&fixture.config(&text)).unwrap()).unwrap();
    assert_eq!(keys.get("small").unwrap().expose(), VLLM_KEY.as_bytes());
}

/// K7 under D1 = A: no table of the document admits a Runpod API key file.
#[test]
fn adv_k7_runpod_api_key_file_is_refused_in_every_table() {
    let fixture = Fixture::new("adv-w02-k7");
    let base = document("127.0.0.1:0", &fixture.owner_secret());
    for text in [
        format!("runpod_api_key_file = \"/run/key\"\n{base}"),
        mutate(
            &base,
            "max_model_len = 1024\n",
            "max_model_len = 1024\nrunpod_api_key_file = \"/run/key\"\n",
        ),
    ] {
        let refusal = load(&fixture.config(&text)).expect_err("refused");
        assert_eq!(refusal.code(), "config:schema", "{refusal}");
    }
}

/// The matrix's own total line agrees with its rows.
#[test]
fn adv_b9_the_matrix_totals_agree_with_its_rows() {
    let text = fs::read_to_string(root().join("docs/llmgw-capability-matrix.md")).unwrap();
    let count = |status: &str| {
        text.lines()
            .filter(|line| {
                line.starts_with("| ")
                    && line.split(" | ").nth(4).is_some_and(|cell| cell == status)
            })
            .count()
    };
    let expected = format!(
        "Total: 83 rows. {} covered, {} partial, {} gap, {} not needed.",
        count("covered"),
        count("partial"),
        count("gap"),
        count("not needed")
    );
    assert!(
        text.contains(&expected),
        "the matrix does not say {expected:?}"
    );
}

/// Citations into `serve.rs` from the matrix (C1, D2) and the specification (upstream.yaml)
/// still land on what they cite after the change inserted ~100 lines above them.
#[test]
fn adv_b9_citations_into_serve_rs_still_land() {
    let serve = fs::read_to_string(root().join("crates/llm-gateway-cli/src/serve.rs")).unwrap();
    let lines: Vec<&str> = serve.lines().collect();
    let range = |from: usize, to: usize| lines[from - 1..to].join("\n");
    // D2 cites 132-151 for the stop and 160-170 (also C1) for `start`.
    assert!(
        range(132, 151).contains("fn wait_for_stop"),
        "serve.rs:132-151"
    );
    assert!(range(160, 170).contains("pub fn start"), "serve.rs:160-170");
    // spec/domains/upstream.yaml cites serve.rs:91 and :112 for the per-wire targets.
    assert!(
        lines[90].contains("targets"),
        "serve.rs:91 is {:?}",
        lines[90]
    );
    assert!(
        lines[111].contains("RouteSummary"),
        "serve.rs:112 is {:?}",
        lines[111]
    );
}
