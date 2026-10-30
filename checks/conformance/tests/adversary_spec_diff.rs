//! Adversary cases for the specification diff gate (`b10x-llm-gateway-conformance diff`).
//!
//! Each case builds its own Git repository under this target's temporary directory, holding a copy
//! of the repository's `spec/`, and runs the built binary in it.

use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

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
            "user.name=spec-diff-adversary",
            "-c",
            "user.email=spec-diff-adversary@example.invalid",
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

fn commit(repo: &Path, message: &str) {
    git(repo, &["add", "-A", "spec"]);
    if repo.join(ACKNOWLEDGEMENTS).exists() {
        git(repo, &["add", ACKNOWLEDGEMENTS]);
    }
    git(repo, &["commit", "--quiet", "-m", message]);
}

fn repository(name: &str) -> PathBuf {
    let repo = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("adversary-spec-diff")
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
    commit(&repo, "base");
    git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo
}

struct Run {
    code: i32,
    output: String,
}

fn diff_with_path(repo: &Path, args: &[&str], path: Option<&OsString>) -> Run {
    let mut command = Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway-conformance"));
    command.arg("diff").args(args).current_dir(repo);
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let output = command.output().unwrap();
    Run {
        code: output.status.code().unwrap(),
        output: format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    }
}

fn diff(repo: &Path, args: &[&str]) -> Run {
    diff_with_path(repo, args, None)
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
        .map_or_else(
            || panic!("no `{endpoint}` digest in:\n{}", run.output),
            |rest| rest.trim().to_owned(),
        )
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

fn leftovers(repo: &Path) -> Vec<PathBuf> {
    let dir = repo.join("target/spec-diff");
    if !dir.exists() {
        return Vec::new();
    }
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}

/// `PATH` with every directory that holds an `ess` removed, so `git` resolves and `ess` does not.
fn path_without_ess() -> OsString {
    let path = env::var_os("PATH").unwrap();
    let kept: Vec<PathBuf> = env::split_paths(&path)
        .filter(|dir| !dir.join("ess").exists())
        .collect();
    let joined = env::join_paths(kept).unwrap();
    let probe = Command::new("git")
        .arg("--version")
        .env("PATH", &joined)
        .output()
        .unwrap();
    assert!(probe.status.success(), "git must stay on PATH");
    joined
}

/// CI runs the gate on a committed tree: the acknowledgement this change ships is in `HEAD`, not
/// only in the working tree. It is applied against the merge base of `origin/main` (pull request)
/// and against the previous head (`--base <before>`, push to main). A gate that compared the file
/// with `HEAD` instead of the merge base would never apply a committed acknowledgement.
#[test]
fn an_acknowledgement_committed_with_its_change_is_applied_on_the_pull_request_and_the_push() {
    let repo = repository("committed-ack");
    change_unclassified(&repo);
    let run = diff(&repo, &[]);
    assert_eq!(run.code, 4, "{}", run.output);
    acknowledge(
        &repo,
        &digest(&run, "before"),
        &digest(&run, "after"),
        &[UNCLASSIFIED],
    );
    commit(&repo, "an unclassified change and its acknowledgement");
    let pull_request = diff(&repo, &[]);
    assert_eq!(pull_request.code, 0, "{}", pull_request.output);
    assert!(
        pull_request.output.contains("1 acknowledged"),
        "{}",
        pull_request.output
    );
    let before = git(&repo, &["rev-parse", "HEAD^"]);
    let push = diff(&repo, &["--base", before.trim()]);
    assert_eq!(push.code, 0, "{}", push.output);
    assert!(push.output.contains("1 acknowledged"), "{}", push.output);
}

/// The base is the merge base of `--base` and `HEAD`, not `--base` itself: when `origin/main` has
/// moved on since this branch forked, what `main` added later is not this change's removal.
#[test]
fn the_compared_base_is_the_fork_point_when_origin_main_has_moved_on() {
    let repo = repository("moved-main");
    remove_lease_expired(&repo);
    commit(&repo, "fork point without LeaseExpired");
    git(&repo, &["branch", "feature"]);
    git(&repo, &["checkout", "--quiet", "HEAD~1", "--", "spec"]);
    commit(&repo, "main restores LeaseExpired after the fork");
    git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(&repo, &["checkout", "--quiet", "feature"]);
    let run = diff(&repo, &[]);
    assert_eq!(run.code, 0, "{}", run.output);
    assert_eq!(
        digest(&run, "before"),
        digest(&run, "after"),
        "{}",
        run.output
    );
}

/// The base copy under `target/spec-diff/` is removed after a passing and after a failing run.
#[test]
fn the_materialised_base_is_removed_after_a_passing_and_a_failing_run() {
    let repo = repository("cleanup");
    let run = diff(&repo, &[]);
    assert_eq!(run.code, 0, "{}", run.output);
    assert_eq!(leftovers(&repo), Vec::<PathBuf>::new());
    remove_lease_expired(&repo);
    let run = diff(&repo, &[]);
    assert_eq!(run.code, 4, "{}", run.output);
    assert_eq!(leftovers(&repo), Vec::<PathBuf>::new());
}

/// Without `ess` on `PATH` the gate cannot run; the refusal says which program is missing.
#[test]
fn without_ess_on_path_the_refusal_names_ess() {
    let repo = repository("no-ess-message");
    let run = diff_with_path(&repo, &[], Some(&path_without_ess()));
    assert_eq!(run.code, 1, "{}", run.output);
    assert!(
        run.output
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| word == "ess"),
        "the refusal does not name the missing program:\n{}",
        run.output
    );
}

/// A run that stops before `ess` answers leaves no base copy behind under `target/spec-diff/`.
#[test]
fn without_ess_on_path_no_base_copy_is_left_behind() {
    let repo = repository("no-ess-leftover");
    let run = diff_with_path(&repo, &[], Some(&path_without_ess()));
    assert_ne!(run.code, 0, "{}", run.output);
    assert_eq!(
        leftovers(&repo),
        Vec::<PathBuf>::new(),
        "a stopped run left its base copy:\n{}",
        run.output
    );
}
