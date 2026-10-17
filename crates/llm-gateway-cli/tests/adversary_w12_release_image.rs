//! Adversary cases for `.github/workflows/release.yml`: the `run:` scripts of the existing-version
//! checks and of the revision step are executed under bash, as a GitHub-hosted runner runs them
//! (`bash --noprofile --norc -e`), against a fake `docker` on `PATH`. Nothing reaches a registry or
//! the network. `RELEASE_WORKFLOW` names another copy of the file, to run these cases on a mutant.

use std::fmt::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_yaml::Value;

const IMAGE: &str = "ghcr.io/beyond10x/llm-gateway";
const VERSION: &str = "1.2.3";

fn workflow() -> Value {
    let path = std::env::var_os("RELEASE_WORKFLOW").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/release.yml"),
        PathBuf::from,
    );
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_yaml::from_str(&text).expect("release.yml is YAML")
}

fn job<'a>(workflow: &'a Value, name: &str) -> &'a Value {
    workflow
        .get("jobs")
        .and_then(|jobs| jobs.get(name))
        .unwrap_or_else(|| panic!("no job {name}"))
}

fn steps(job: &Value) -> &[Value] {
    job.get("steps")
        .and_then(Value::as_sequence)
        .expect("steps is a list")
}

/// The `run:` script of the one step in `job` whose script contains `marker`.
fn script(workflow: &Value, job_name: &str, marker: &str) -> String {
    let found: Vec<&str> = steps(job(workflow, job_name))
        .iter()
        .filter_map(|step| step.get("run").and_then(Value::as_str))
        .filter(|run| run.contains(marker))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "expected one step of {job_name} running {marker}"
    );
    found[0].to_owned()
}

/// A fresh directory for one case, under the target's scratch.
fn scratch(case: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("adv-w12-release-{case}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bin")).expect("scratch dir");
    dir
}

/// Installs a fake `docker` whose body is `body`; every call's arguments go to `<dir>/calls`.
fn fake_docker(dir: &Path, body: &str) {
    let path = dir.join("bin/docker");
    let calls = dir.join("calls");
    let mut text = String::from("#!/usr/bin/bash\n");
    writeln!(text, "echo \"$*\" >> '{}'", calls.display()).unwrap();
    text.push_str(body);
    std::fs::write(&path, text).expect("fake docker");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

struct Run {
    success: bool,
    output: String,
    calls: String,
}

/// Runs `script` as the runner does, in `cwd`, with `path` as `PATH`.
fn run(dir: &Path, script: &str, cwd: &Path, path: &str) -> Run {
    let file = dir.join("step.sh");
    std::fs::write(&file, script).expect("step script");
    let out = Command::new("/usr/bin/bash")
        .args(["--noprofile", "--norc", "-e"])
        .arg(&file)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", path)
        .env("IMAGE", IMAGE)
        .env("GITHUB_REF_NAME", VERSION)
        .output()
        .expect("bash runs");
    Run {
        success: out.status.success(),
        output: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        calls: std::fs::read_to_string(dir.join("calls")).unwrap_or_default(),
    }
}

fn first_check(case: &str, docker: Option<&str>) -> Run {
    let workflow = workflow();
    let script = script(&workflow, "version-is-new", "imagetools inspect");
    let dir = scratch(case);
    if let Some(body) = docker {
        fake_docker(&dir, body);
    }
    // The check uses only builtins and docker: a PATH of the fake alone.
    let path = dir.join("bin").display().to_string();
    run(&dir, &script, &dir, &path)
}

#[test]
fn adv_w12_first_check_passes_only_on_the_registrys_not_found() {
    let exists = first_check("exists", Some("exit 0\n"));
    assert!(
        !exists.success,
        "an existing version passed: {}",
        exists.output
    );
    assert!(
        exists.output.contains("already exists"),
        "{}",
        exists.output
    );

    let missing = first_check(
        "missing",
        Some("echo \"ERROR: $4: not found\" >&2\nexit 1\n"),
    );
    assert!(
        missing.success,
        "a new version was refused: {}",
        missing.output
    );

    for (case, answer) in [
        (
            "unauthorized",
            "ERROR: unauthorized: authentication required",
        ),
        (
            "denied",
            "ERROR: denied: requested access to the resource is denied",
        ),
        (
            "server",
            "ERROR: unexpected status: 500 Internal Server Error",
        ),
        ("ratelimit", "ERROR: toomanyrequests: retry later"),
    ] {
        let answered = first_check(case, Some(&format!("echo '{answer}' >&2\nexit 1\n")));
        assert!(
            !answered.success,
            "registry answer `{answer}` let the release through: {}",
            answered.output
        );
    }
}

/// The first check reads any output holding `not found` as the registry's answer for the tag,
/// so a shell that cannot find `docker` at all (`docker: command not found`) lets the release on.
#[test]
fn adv_w12_first_check_fails_closed_when_docker_cannot_run() {
    let no_docker = first_check("no-docker", None);
    assert!(
        !no_docker.success,
        "with no docker the check passed: {}",
        no_docker.output
    );
}

fn manifest_step(case: &str, inspect: &str) -> Run {
    let workflow = workflow();
    let script = script(&workflow, "manifest-list", "imagetools create");
    let dir = scratch(case);
    let digests = dir.join("digests");
    std::fs::create_dir_all(&digests).expect("digests dir");
    for digest in ["a".repeat(64), "b".repeat(64)] {
        std::fs::write(digests.join(digest), "").expect("digest file");
    }
    fake_docker(
        &dir,
        &format!(
            "if [ \"$3\" = inspect ]; then\n{inspect}\nfi\n\
             if [ \"$3\" = create ]; then echo created; exit 0; fi\nexit 0\n"
        ),
    );
    let path = format!("{}:/usr/bin:/bin", dir.join("bin").display());
    run(&dir, &script, &digests, &path)
}

#[test]
fn adv_w12_manifest_recheck_refuses_an_existing_version() {
    let exists = manifest_step("manifest-exists", "exit 0");
    assert!(!exists.success, "{}", exists.output);
    assert!(
        !exists.calls.contains("imagetools create"),
        "{}",
        exists.calls
    );
}

/// The manifest job's re-check (`release.yml:115`) reads every failed inspect as "not there",
/// so a registry error at that moment goes on to `imagetools create --tag`, the step that would
/// replace a published version. The first check refuses the same answer.
#[test]
fn adv_w12_manifest_recheck_never_tags_when_the_registry_errors() {
    let errored = manifest_step(
        "manifest-error",
        "echo 'ERROR: unexpected status: 500 Internal Server Error' >&2; exit 1",
    );
    assert!(
        !errored.calls.contains("imagetools create"),
        "a registry error went on to tag the list: calls {:?}, output {}",
        errored.calls,
        errored.output
    );
    assert!(!errored.success, "{}", errored.output);
}

/// A refusal that a later job may ignore refuses nothing: no job or step on the release path may
/// continue on error, and no job that pushes may carry an `if:` (`always()`, `!cancelled()`, …)
/// that runs it after the check failed.
#[test]
fn adv_w12_a_failed_check_stops_every_push() {
    let workflow = workflow();
    let jobs = workflow
        .get("jobs")
        .and_then(Value::as_mapping)
        .expect("jobs");
    for (name, job) in jobs {
        let name = name.as_str().expect("job name");
        assert!(
            job.get("continue-on-error").is_none(),
            "job {name} continues on error"
        );
        assert!(job.get("if").is_none(), "job {name} carries a condition");
        for step in steps(job) {
            assert!(
                step.get("continue-on-error").is_none(),
                "a step of {name} continues on error: {step:?}"
            );
            let checks = step
                .get("run")
                .and_then(Value::as_str)
                .is_some_and(|run| run.contains("imagetools inspect"));
            assert!(
                !(checks && step.get("if").is_some()),
                "the check step of {name} carries a condition"
            );
        }
    }
}

/// `${{ … }}` inside `run:` is pasted into the script before bash reads it; the scripts read the
/// ref through `$GITHUB_REF_NAME` and step outputs through `env:`.
#[test]
fn adv_w12_no_run_script_interpolates_an_expression() {
    let workflow = workflow();
    for (name, job) in workflow
        .get("jobs")
        .and_then(Value::as_mapping)
        .expect("jobs")
    {
        for step in steps(job) {
            if let Some(run) = step.get("run").and_then(Value::as_str) {
                assert!(
                    !run.contains("${{"),
                    "a step of {name:?} interpolates an expression into its script: {run}"
                );
            }
        }
    }
}

/// The revision step, run in a clone holding an annotated tag the way `actions/checkout` fetches
/// it, writes the tag's commit, not the tag object, to `$GITHUB_OUTPUT`.
#[test]
fn adv_w12_revision_step_names_the_tags_commit_for_an_annotated_tag() {
    let workflow = workflow();
    let script = steps(job(&workflow, "build"))
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some("revision"))
        .and_then(|step| step.get("run"))
        .and_then(Value::as_str)
        .expect("revision step")
        .to_owned();
    let dir = scratch("revision");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("repo dir");
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=adv",
                "-c",
                "user.email=adv@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
            ])
            .args(args)
            .current_dir(&repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git(&["init", "-q"]);
    git(&["commit", "-q", "--allow-empty", "-m", "release"]);
    git(&["tag", "-a", VERSION, "-m", "release"]);
    let commit = git(&["rev-parse", "HEAD"]);
    let tag_object = git(&["rev-parse", &format!("refs/tags/{VERSION}")]);
    assert_ne!(commit, tag_object, "the tag is annotated");

    let output = dir.join("github_output");
    let file = dir.join("step.sh");
    std::fs::write(&file, &script).expect("step script");
    let out = Command::new("/usr/bin/bash")
        .args(["--noprofile", "--norc", "-e"])
        .arg(&file)
        .current_dir(&repo)
        .env("GITHUB_REF", format!("refs/tags/{VERSION}"))
        .env("GITHUB_OUTPUT", &output)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("bash runs");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        std::fs::read_to_string(&output).expect("output").trim(),
        format!("sha={commit}")
    );
}
