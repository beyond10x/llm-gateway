//! Route inspection cannot resolve a secret or provision a resource, because the gateway crate
//! is not linked against anything that can — directly or transitively. This is the structural
//! half of the acceptance; the behavioural half is in `gateway.rs`.
//!
//! Two independent mechanisms, each with a **positive control** that proves it is not vacuous:
//!
//! 1. The crate's declared dependency list, read from `cargo metadata --no-deps` by matching the
//!    dependency array to its closing bracket. The control reads a workspace package that really
//!    declares several dependencies and asserts the forbidden ones among them are found.
//! 2. The transitive closure, read from `cargo tree`, which cargo computes rather than this
//!    file. The control reads the conformance runner, which links the hosting crates, and
//!    asserts the closure really does surface forbidden crates there and crates it never declared.
//!
//! An earlier version of this file split the metadata at the first `]`, which closes the first
//! dependency's own `features` array; it examined one dependency of eighteen, and it examined no
//! transitive edge at all. Until this crate moved out of beyond10x/llm, the two controls read
//! llm's conformance runner and `llm-routing`, neither of which is in this workspace.

use std::{path::Path, process::Command};

/// Crates that resolve credentials, speak to an upstream, or create billed resources.
const FORBIDDEN: &[&str] = &[
    "b10x-llm-credentials",
    "b10x-llm-http",
    "b10x-llm-modal",
    "b10x-llm-provision",
    "b10x-llm-runpod",
    "axum",
    "hyper",
    "keyring-core",
    "reqwest",
    "tokio",
];

const MARKER: &str = "\"dependencies\":[";

fn workspace_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cargo(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO"))
        .args(args)
        .current_dir(workspace_root())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "cargo {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn metadata() -> String {
    cargo(&[
        "metadata",
        "--offline",
        "--locked",
        "--no-deps",
        "--format-version",
        "1",
    ])
}

/// Every dependency `package` declares, read to the array's matching bracket rather than to the
/// first `]` encountered — a dependency entry contains its own `features` and `target` arrays.
fn declared_dependencies(metadata: &str, package: &str) -> Vec<String> {
    let start = metadata
        .find(&format!("\"name\":\"{package}\""))
        .unwrap_or_else(|| panic!("{package} is absent from cargo metadata"));
    let rest = &metadata[start..];
    let open = rest
        .find(MARKER)
        .unwrap_or_else(|| panic!("{package} declares no dependency list"))
        + MARKER.len();
    let bytes = rest.as_bytes();
    let mut depth = 1_usize;
    let mut index = open;
    while index < bytes.len() && depth > 0 {
        match bytes[index] {
            b'[' => depth += 1,
            b']' => depth -= 1,
            _ => {}
        }
        index += 1;
    }
    assert_eq!(depth, 0, "{package}'s dependency array is unterminated");
    rest[open..index - 1]
        .split("{\"name\":\"")
        .skip(1)
        .filter_map(|entry| entry.split_once('"').map(|(name, _)| name.to_string()))
        .collect()
}

/// Every package cargo itself resolves into `package`'s build, including the package itself.
fn transitive_closure(package: &str) -> Vec<String> {
    cargo(&[
        "tree",
        "-p",
        package,
        "--locked",
        "--offline",
        "--all-features",
        "--edges",
        "normal,build",
        "--prefix",
        "none",
        "--format",
        "{p}",
    ])
    .lines()
    .filter_map(|line| line.split_whitespace().next())
    .filter(|name| !name.is_empty())
    .map(str::to_string)
    .collect()
}

#[test]
fn the_declared_dependency_scan_reads_the_whole_array() {
    let metadata = metadata();
    // Positive control: a real workspace package that declares many dependencies, with two
    // forbidden names that are not the first entry. A scan that stops at the first `]` finds
    // neither.
    let control = declared_dependencies(&metadata, "b10x-llm-gateway-conformance");
    assert!(
        control.len() > 1,
        "the control package no longer declares several dependencies: {control:?}"
    );
    for forbidden in ["b10x-llm-provision", "b10x-llm-runpod"] {
        assert!(
            control.first().is_some_and(|first| first != forbidden),
            "{forbidden} is the control package's first dependency, so finding it would not \
             prove the scan reads past the first entry: {control:?}"
        );
        assert!(
            control.iter().any(|name| name == forbidden),
            "the control package no longer declares {forbidden}, so this case would no longer \
             prove the scan reads past the first entry: {control:?}"
        );
    }

    let declared = declared_dependencies(&metadata, "b10x-llm-gateway");
    for forbidden in FORBIDDEN {
        assert!(
            !declared.iter().any(|name| name == forbidden),
            "llm-gateway declares {forbidden}, so inspection can no longer be shown to resolve \
             no secret and provision nothing: {declared:?}"
        );
    }
    assert!(
        declared.is_empty(),
        "llm-gateway is expected to declare nothing at all: {declared:?}"
    );
}

#[test]
fn the_transitive_closure_contains_nothing_that_can_resolve_a_secret_or_provision() {
    // Positive control: the conformance runner links the hosting crates, so its closure really
    // does surface forbidden crates, and it reaches crates it never declares itself. If either
    // stops holding, the control has stopped proving that the closure walk finds offenders and
    // sees past the first edge.
    let control = transitive_closure("b10x-llm-gateway-conformance");
    let control_offenders: Vec<&String> = control
        .iter()
        .filter(|name| FORBIDDEN.contains(&name.as_str()))
        .collect();
    assert!(
        control_offenders
            .iter()
            .any(|name| *name == "b10x-llm-provision")
            && control_offenders
                .iter()
                .any(|name| *name == "b10x-llm-runpod"),
        "the conformance runner's closure no longer reaches the crates this control needs: \
         {control:?}"
    );
    let declared = declared_dependencies(&metadata(), "b10x-llm-gateway-conformance");
    assert!(
        control
            .iter()
            .any(|name| name != "b10x-llm-gateway-conformance" && !declared.contains(name)),
        "the conformance runner's closure holds only what it declares, so this control no \
         longer proves the walk sees past the first edge: {control:?}"
    );

    let closure = transitive_closure("b10x-llm-gateway");
    assert!(
        closure.iter().any(|name| name == "b10x-llm-gateway"),
        "the closure walk did not even find the gateway: {closure:?}"
    );
    let offenders: Vec<&String> = closure
        .iter()
        .filter(|name| FORBIDDEN.contains(&name.as_str()))
        .collect();
    assert!(
        offenders.is_empty(),
        "llm-gateway's build reaches {offenders:?}. Taking the llm-routing dependency costs \
         exactly this guarantee, which is a decision, not a refactor."
    );
    assert_eq!(
        closure,
        vec!["b10x-llm-gateway".to_string()],
        "llm-gateway is expected to link nothing at all"
    );
}

#[test]
fn the_manifest_names_nothing_forbidden() {
    let manifest = include_str!("../Cargo.toml");
    for forbidden in FORBIDDEN {
        assert!(
            !manifest.contains(forbidden),
            "the gateway manifest names {forbidden}"
        );
    }
}
