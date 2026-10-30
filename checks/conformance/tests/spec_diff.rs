//! The specification diff gate: `b10x-llm-gateway-conformance diff` compares the working tree's
//! `spec/` with `spec/` at the merge base and fails on a breaking or unclassified change that
//! `contracts/diff-acknowledgements.json` does not acknowledge.
//!
//! Each case builds its own Git repository under this target's temporary directory, holding a copy
//! of the repository's `spec/`, and runs the built binary in it with the `ess` on `PATH`.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const REMOVED: &str = "type/llm-gateway.hosting.StopReason/variant-removed/LeaseExpired";
const UNCLASSIFIED: &str = "system/llm-gateway/unclassified-changed";
const ACKNOWLEDGEMENTS: &str = "contracts/diff-acknowledgements.json";

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_tree(&path, &target);
        } else {
            fs::copy(&path, &target).unwrap();
        }
    }
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=spec-diff-test",
            "-c",
            "user.email=spec-diff-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// A repository whose `main` holds this repository's `spec/`, with `origin/main` pointing at it.
fn repository(name: &str) -> PathBuf {
    let repo = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("spec-diff")
        .join(name);
    if repo.exists() {
        fs::remove_dir_all(&repo).unwrap();
    }
    fs::create_dir_all(&repo).unwrap();
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec"),
        &repo.join("spec"),
    );
    git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    git(&repo, &["add", "spec"]);
    git(&repo, &["commit", "--quiet", "-m", "base"]);
    git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo
}

struct Run {
    code: i32,
    output: String,
}

fn diff(repo: &Path, args: &[&str]) -> Run {
    let output = Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway-conformance"))
        .arg("diff")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    Run {
        code: output.status.code().unwrap(),
        output: format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    }
}

fn edit(repo: &Path, file: &str, from: &str, to: &str) {
    let path = repo.join(file);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains(from), "{file} does not hold {from:?}");
    fs::write(&path, text.replacen(from, to, 1)).unwrap();
}

fn remove_lease_expired(repo: &Path) {
    edit(
        repo,
        "spec/domains/hosting.yaml",
        "LifetimeExceeded, LeaseExpired, OperatorRequest",
        "LifetimeExceeded, OperatorRequest",
    );
}

/// An enum attribute's value changed: ESS 0.77.0 classifies this as neither widening nor
/// narrowing, so the delta holds the one `unknown` change `system/llm-gateway/unclassified-changed`.
fn change_unclassified(repo: &Path) {
    edit(
        repo,
        "spec/domains/hosting.yaml",
        "message: \"invalid hosting policy\"",
        "message: \"the hosting policy is invalid\"",
    );
}

fn digest(run: &Run, endpoint: &str) -> String {
    run.output
        .lines()
        .find_map(|line| line.trim().strip_prefix(endpoint))
        .map(|rest| rest.trim().to_owned())
        .unwrap_or_else(|| panic!("no `{endpoint}` digest in:\n{}", run.output))
}

fn acknowledge(repo: &Path, before: &str, after: &str, ids: &[&str]) {
    fs::create_dir_all(repo.join("contracts")).unwrap();
    fs::write(
        repo.join(ACKNOWLEDGEMENTS),
        serde_json::to_string_pretty(&serde_json::json!({
            "format": "ess-diff-acknowledgements/1",
            "before": before,
            "after": after,
            "acknowledged": ids,
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn comparing_the_specification_with_its_merge_base_on_origin_main_exits_0() {
    let repo = repository("self");
    let run = diff(&repo, &[]);
    assert_eq!(run.code, 0, "{}", run.output);
    assert_eq!(
        digest(&run, "before"),
        digest(&run, "after"),
        "{}",
        run.output
    );
}

#[test]
fn a_removed_lease_expired_variant_fails_naming_its_change_and_passes_once_restored() {
    let repo = repository("removed");
    remove_lease_expired(&repo);
    let run = diff(&repo, &[]);
    assert_eq!(run.code, 4, "{}", run.output);
    assert!(
        run.output
            .contains(&format!("spec diff failed on: {REMOVED}")),
        "{}",
        run.output
    );
    git(&repo, &["checkout", "--quiet", "--", "spec"]);
    let run = diff(&repo, &[]);
    assert_eq!(run.code, 0, "{}", run.output);
}

#[test]
fn an_added_domain_is_compatible_in_ess_0_77_and_passes() {
    let repo = repository("added-domain");
    fs::write(
        repo.join("spec/domains/probe.yaml"),
        "domain: llm-gateway.probe\ntypes:\n  - name: llm-gateway.probe.Kind\n    kind: enum\n    variants: [One, Two]\n",
    )
    .unwrap();
    edit(
        &repo,
        "spec/system.yaml",
        "  - llm-gateway.clients\n",
        "  - llm-gateway.clients\n  - llm-gateway.probe\n",
    );
    let run = diff(&repo, &[]);
    assert_eq!(run.code, 0, "{}", run.output);
    assert!(
        run.output.contains("domain/llm-gateway.probe/added"),
        "{}",
        run.output
    );
}

#[test]
fn an_unclassified_change_fails_without_an_acknowledgement_and_passes_with_one() {
    let repo = repository("unclassified");
    change_unclassified(&repo);
    let run = diff(&repo, &[]);
    assert_eq!(run.code, 4, "{}", run.output);
    assert!(
        run.output
            .contains(&format!("spec diff failed on: {UNCLASSIFIED}")),
        "{}",
        run.output
    );
    acknowledge(
        &repo,
        &digest(&run, "before"),
        &digest(&run, "after"),
        &[UNCLASSIFIED],
    );
    let run = diff(&repo, &[]);
    assert_eq!(run.code, 0, "{}", run.output);
    assert!(run.output.contains("1 acknowledged"), "{}", run.output);
}

#[test]
fn an_acknowledgement_bound_to_other_digests_is_refused_by_ess() {
    let repo = repository("mismatch");
    change_unclassified(&repo);
    let run = diff(&repo, &[]);
    assert_eq!(run.code, 4, "{}", run.output);
    let before = digest(&run, "before");
    let after = digest(&run, "after");
    let other = "0".repeat(before.len());
    for (before, after) in [
        (other.as_str(), after.as_str()),
        (before.as_str(), other.as_str()),
    ] {
        acknowledge(&repo, before, after, &[UNCLASSIFIED]);
        let run = diff(&repo, &[]);
        assert_eq!(run.code, 1, "{}", run.output);
        assert!(run.output.contains("refused"), "{}", run.output);
        assert!(
            run.output.contains("conflicting_declaration"),
            "{}",
            run.output
        );
    }
}

#[test]
fn an_acknowledgement_the_base_already_holds_belongs_to_an_earlier_comparison_and_is_not_applied() {
    let repo = repository("inherited");
    acknowledge(&repo, &"1".repeat(64), &"2".repeat(64), &[UNCLASSIFIED]);
    git(&repo, &["add", ACKNOWLEDGEMENTS]);
    git(&repo, &["commit", "--quiet", "-m", "an earlier change"]);
    git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let run = diff(&repo, &[]);
    assert_eq!(run.code, 0, "{}", run.output);
    assert!(run.output.contains("not applied"), "{}", run.output);
    acknowledge(&repo, &"3".repeat(64), &"4".repeat(64), &[UNCLASSIFIED]);
    let run = diff(&repo, &[]);
    assert_eq!(run.code, 1, "{}", run.output);
}

#[test]
fn the_base_is_the_merge_base_of_the_named_revision_with_head() {
    let repo = repository("named-base");
    remove_lease_expired(&repo);
    git(&repo, &["commit", "--quiet", "-am", "remove LeaseExpired"]);
    let run = diff(&repo, &["--base", "HEAD^"]);
    assert_eq!(run.code, 4, "{}", run.output);
    assert!(run.output.contains(REMOVED), "{}", run.output);
    let run = diff(&repo, &["--base", "HEAD"]);
    assert_eq!(run.code, 0, "{}", run.output);
    git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let run = diff(&repo, &[]);
    assert_eq!(run.code, 0, "{}", run.output);
}

#[test]
fn a_base_revision_that_does_not_resolve_is_refused() {
    let repo = repository("no-base");
    let run = diff(&repo, &["--base", "no-such-revision"]);
    assert_eq!(run.code, 1, "{}", run.output);
    assert!(run.output.contains("no-such-revision"), "{}", run.output);
}
