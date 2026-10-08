//! The crate reference, from `cargo metadata --no-deps`.

use std::{fmt::Write as _, path::Path, process::Command};

use serde_json::Value;

use crate::{Result, cell, front_matter};

/// Where the crate reference lands, relative to the repository root.
pub const PAGE: &str = "website/docs/reference/crates.md";

/// `cargo metadata --no-deps` for the workspace at `root`.
pub(crate) fn metadata(root: &Path) -> Result<Value> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--offline",
        ])
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .output()
        .map_err(|error| format!("running cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("parsing cargo metadata: {error}"))
}

/// One workspace package as the page lists it.
struct Package {
    name: String,
    library: Option<String>,
    binaries: Vec<String>,
    path: String,
    description: Option<String>,
    features: Vec<String>,
}

fn text(value: &Value, key: &str) -> Result<String> {
    value[key]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("cargo metadata: {key} is not a string"))
}

fn package(value: &Value, root: &str) -> Result<Package> {
    let name = text(value, "name")?;
    let manifest = text(value, "manifest_path")?;
    let path = manifest
        .strip_prefix(root)
        .and_then(|rest| rest.strip_suffix("Cargo.toml"))
        .map(|rest| rest.trim_matches('/').to_owned())
        .ok_or_else(|| format!("{name} lies outside the workspace root"))?;
    let mut library = None;
    let mut binaries = Vec::new();
    for target in value["targets"].as_array().into_iter().flatten() {
        let kinds: Vec<_> = target["kind"].as_array().into_iter().flatten().collect();
        let target_name = text(target, "name")?;
        if kinds.iter().any(|kind| *kind == "lib") {
            library = Some(target_name);
        } else if kinds.iter().any(|kind| *kind == "bin") {
            binaries.push(target_name);
        }
    }
    binaries.sort();
    let features = value["features"]
        .as_object()
        .into_iter()
        .flat_map(|map| map.keys())
        .filter(|feature| *feature != "default")
        .cloned()
        .collect();
    Ok(Package {
        name,
        library,
        binaries,
        path,
        description: value["description"].as_str().map(str::to_owned),
        features,
    })
}

fn code_list(items: &[String]) -> String {
    if items.is_empty() {
        "—".to_owned()
    } else {
        items
            .iter()
            .map(|item| format!("`{item}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The crate reference page.
pub(crate) fn page(metadata: &Value) -> Result<String> {
    let root = text(metadata, "workspace_root")?;
    let mut packages = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata lists no packages")?
        .iter()
        .map(|value| package(value, &root))
        .collect::<Result<Vec<_>>>()?;
    packages.sort_by(|a, b| a.path.cmp(&b.path));
    let version = metadata["packages"][0]["version"]
        .as_str()
        .ok_or("cargo metadata: no version")?;

    let mut page = front_matter(
        "Crates",
        4,
        &format!(
            "Every package in the llm-gateway workspace at {version}: what it is, its library and its binaries."
        ),
    );
    let _ = write!(
        page,
        "# Crates\n\nThe workspace holds {} packages at version `{version}`. None is published to \
         a registry: build from a release tag. Package names start with `b10x-`; library names do \
         not, so `b10x-llm-gateway` is `use llm_gateway`. This page is generated from \
         `cargo metadata` by `llm-gateway-docs`.\n\n",
        packages.len()
    );
    page.push_str(
        "| Package | Library | Binaries | Directory | What it is |\n| --- | --- | --- | --- | --- |\n",
    );
    for package in &packages {
        let library = package
            .library
            .as_ref()
            .map_or_else(|| "—".to_owned(), |library| format!("`{library}`"));
        let _ = writeln!(
            page,
            "| `{}` | {library} | {} | `{}` | {} |",
            package.name,
            code_list(&package.binaries),
            package.path,
            package.description.as_deref().map_or("—".to_owned(), cell)
        );
    }
    let featured: Vec<_> = packages
        .iter()
        .filter(|package| !package.features.is_empty())
        .collect();
    if featured.is_empty() {
        page.push_str("\nNo package declares an optional feature.\n");
    } else {
        page.push_str("\n## Optional features\n\n");
        for package in featured {
            let _ = writeln!(
                page,
                "- `{}`: {}",
                package.name,
                code_list(&package.features)
            );
        }
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "workspace_root": "/w",
            "packages": [
                {
                    "name": "b10x-x", "version": "1.2.3", "manifest_path": "/w/crates/x/Cargo.toml",
                    "description": "The x crate.",
                    "targets": [
                        {"kind": ["lib"], "name": "x"},
                        {"kind": ["bin"], "name": "x-tool"},
                        {"kind": ["test"], "name": "t"}
                    ],
                    "features": {}
                },
                {
                    "name": "tool", "version": "1.2.3", "manifest_path": "/w/checks/tool/Cargo.toml",
                    "targets": [{"kind": ["bin"], "name": "tool"}],
                    "features": {}
                }
            ]
        })
    }

    #[test]
    fn page_lists_packages_libraries_and_binaries() {
        let page = page(&sample()).unwrap();
        assert!(page.contains(crate::HEADER));
        assert!(page.contains("custom_edit_url: null"));
        assert!(page.contains("| `b10x-x` | `x` | `x-tool` | `crates/x` | The x crate. |"));
        assert!(page.contains("| `tool` | — | `tool` | `checks/tool` | — |"));
        assert!(page.contains("No package declares an optional feature."));
        assert!(page.find("checks/tool").unwrap() < page.find("crates/x").unwrap());
        assert!(!page.contains("/w/"), "absolute paths stay out of the page");
    }

    #[test]
    fn a_package_outside_the_root_is_refused() {
        let mut metadata = sample();
        metadata["packages"][0]["manifest_path"] = json!("/elsewhere/Cargo.toml");
        assert!(page(&metadata).is_err());
    }
}
