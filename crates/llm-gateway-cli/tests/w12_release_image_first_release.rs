//! The first release: the package does not exist yet, so the registry answers `denied` for the
//! version. The existing-version checks of `.github/workflows/release.yml` ask the packages API
//! (a fake `gh` on `PATH`; nothing reaches the network) and treat only its 404 as absence.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_yaml::Value;

fn scripts_with(marker: &str) -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/release.yml");
    let workflow: Value = serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    workflow["jobs"]
        .as_mapping()
        .unwrap()
        .values()
        .flat_map(|job| job["steps"].as_sequence().unwrap().clone())
        .filter_map(|step| step.get("run").and_then(Value::as_str).map(str::to_owned))
        .filter(|run| run.contains(marker))
        .collect()
}

fn fake(dir: &Path, name: &str, body: &str) {
    let path = dir.join("bin").join(name);
    std::fs::write(&path, format!("#!/usr/bin/bash\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Runs the first check with a registry that answers denied and a packages API answering `api`.
fn first_release(case: &str, api: &str) -> (bool, String) {
    let dir: PathBuf =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("w12-first-release-{case}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    fake(
        &dir,
        "docker",
        "echo 'ERROR: denied: requested access to the resource is denied' >&2; exit 1",
    );
    fake(&dir, "gh", api);
    let scripts = scripts_with("imagetools inspect");
    let script = scripts.first().expect("a check script");
    let file = dir.join("step.sh");
    std::fs::write(&file, script).unwrap();
    let out = Command::new("/usr/bin/bash")
        .args(["--noprofile", "--norc", "-e"])
        .arg(&file)
        .current_dir(&dir)
        .env_clear()
        .env("PATH", dir.join("bin"))
        .env("IMAGE", "ghcr.io/beyond10x/llm-gateway")
        .env("GITHUB_REF_NAME", "1.2.3")
        .output()
        .unwrap();
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[test]
fn w12_denied_with_no_package_yet_is_an_absent_version() {
    let (ok, output) = first_release("404", "echo 'gh: Not Found (HTTP 404)' >&2; exit 1");
    assert!(ok, "the first release was refused: {output}");
}

#[test]
fn w12_denied_for_an_existing_package_or_another_api_answer_fails_closed() {
    for (case, api) in [
        ("exists", "echo '{}'; exit 0"),
        ("forbidden", "echo 'gh: Forbidden (HTTP 403)' >&2; exit 1"),
        ("server", "echo 'gh: Server Error (HTTP 500)' >&2; exit 1"),
        ("no-gh", "exit 127"),
    ] {
        let (ok, output) = first_release(case, api);
        assert!(!ok, "{case}: the release went on: {output}");
    }
}

#[test]
fn w12_both_checks_read_the_registry_the_same_way() {
    let scripts = scripts_with("imagetools inspect");
    assert_eq!(scripts.len(), 2);
    let check = |s: &str| s[..s.find("echo \"$reference is not in").unwrap()].to_owned();
    assert_eq!(check(&scripts[0]), check(&scripts[1]));
}
