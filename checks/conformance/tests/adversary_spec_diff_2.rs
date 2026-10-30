//! Adversary cases, second pass, for the specification diff gate
//! (`b10x-llm-gateway-conformance diff`): the `spawn` helper's refusal text, the `BaseCopy` guard
//! after a killed run, and the refusal for a base that shares no history with `HEAD`.
//!
//! Each case builds its own Git repository under this target's temporary directory, holding a copy
//! of the repository's `spec/`, and runs the built binary in it.

use std::{
    env,
    ffi::OsString,
    fs,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

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

fn repository(name: &str) -> PathBuf {
    let repo = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("adversary-spec-diff-2")
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
    git(&repo, &["add", "-A", "spec"]);
    git(&repo, &["commit", "--quiet", "-m", "base"]);
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
fn path_without_ess() -> Vec<PathBuf> {
    let path = env::var_os("PATH").unwrap();
    env::split_paths(&path)
        .filter(|dir| !dir.join("ess").exists())
        .collect()
}

/// An `ess` that is on `PATH` and cannot be executed (a download whose mode lost `+x`) is refused
/// with the OS's "Permission denied". The refusal must not send the operator looking for `ess` on
/// `PATH`, where it already is: the `spawn` helper appends "is it on PATH?" to every start error.
#[test]
fn an_ess_on_path_that_cannot_be_executed_is_not_reported_as_missing_from_path() {
    let repo = repository("ess-not-executable");
    let fake = repo.join("fake-bin");
    fs::create_dir_all(&fake).unwrap();
    fs::write(fake.join("ess"), b"not a program\n").unwrap();
    let mut dirs = vec![fake];
    dirs.extend(path_without_ess());
    let path = env::join_paths(dirs).unwrap();
    let run = diff_with_path(&repo, &[], Some(&path));
    assert_eq!(run.code, 1, "{}", run.output);
    assert!(
        run.output.contains("os error 13"),
        "expected the permission refusal:\n{}",
        run.output
    );
    assert!(
        !run.output.contains("is it on PATH"),
        "an ess found on PATH is reported as possibly missing from PATH:\n{}",
        run.output
    );
}

/// Linux `O_NONBLOCK`: opening a FIFO for writing with it fails with `ENXIO` until a reader holds
/// the FIFO open, so a successful open proves `ess` is reading it.
const O_NONBLOCK: i32 = 0o4000;

/// A run killed while `ess` compares (Ctrl-C in `task check`, a cancelled job) never reaches the
/// `BaseCopy` drop, so `target/spec-diff/base-<pid>` stays. The next run has another pid and removes
/// only its own `base-<pid>`: the dead run's copy is never removed by any later run.
#[test]
fn a_base_copy_left_by_a_killed_run_is_removed_by_a_later_run() {
    let repo = repository("killed-run");
    let fifo = repo.join("spec/zz-hold.yaml");
    let made = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(made.success(), "mkfifo failed");
    let mut gate = Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway-conformance"))
        .arg("diff")
        .current_dir(&repo)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let writer = loop {
        match fs::OpenOptions::new()
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open(&fifo)
        {
            Ok(writer) => break writer,
            Err(error) => {
                assert!(
                    Instant::now() < deadline,
                    "ess never opened the held spec file: {error}"
                );
                thread::sleep(Duration::from_millis(50));
            }
        }
    };
    let killed = format!("base-{}", gate.id());
    gate.kill().unwrap();
    gate.wait().unwrap();
    drop(writer);
    assert_eq!(
        leftovers(&repo),
        vec![repo.join("target/spec-diff").join(&killed)],
        "precondition: the killed run left its base copy"
    );
    fs::remove_file(&fifo).unwrap();
    let run = diff_with_path(&repo, &[], None);
    assert_eq!(run.code, 0, "{}", run.output);
    assert_eq!(
        leftovers(&repo),
        Vec::<PathBuf>::new(),
        "the killed run's base copy outlived a later complete run:\n{}",
        run.output
    );
}

/// When `--base` shares no history with `HEAD` (an unrelated branch, or a shallow clone whose
/// fetched `origin/main` lost its parents), `git merge-base` exits 1 and prints nothing. The
/// refusal must still say why; today it ends at "refused: " with no reason.
#[test]
fn a_base_with_no_common_history_is_refused_with_a_reason() {
    let repo = repository("unrelated-base");
    git(&repo, &["checkout", "--quiet", "--orphan", "unrelated"]);
    git(&repo, &["commit", "--quiet", "-m", "unrelated history"]);
    git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(&repo, &["checkout", "--quiet", "main"]);
    let run = diff_with_path(&repo, &[], None);
    assert_eq!(run.code, 1, "{}", run.output);
    let reason = run.output.split_once("refused:").map_or_else(
        || panic!("no refusal in:\n{}", run.output),
        |(_, rest)| rest.trim().trim_matches('"').trim().to_owned(),
    );
    assert!(
        !reason.is_empty(),
        "the refusal gives no reason:\n{}",
        run.output
    );
}
