//! The gate's test step, held to the workspace rule "Tests stay fast". Repository files are read
//! at run time through `CARGO_MANIFEST_DIR`.

mod budget;

use std::path::{Path, PathBuf};

fn repository() -> PathBuf {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    Path::new(&manifest)
        .join("../..")
        .canonicalize()
        .expect("the repository root exists")
}

fn read(relative: &str) -> String {
    std::fs::read_to_string(repository().join(relative))
        .unwrap_or_else(|error| panic!("reading {relative}: {error}"))
}

fn workflow() -> serde_yaml::Value {
    serde_yaml::from_str(&read(".github/workflows/gate.yml")).expect("gate.yml is YAML")
}
