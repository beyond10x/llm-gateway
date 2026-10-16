//! The release workflow, `.github/workflows/release.yml`: a pushed `<major>.<minor>.<patch>` tag,
//! and nothing else, builds the root `Dockerfile` natively on an amd64 and an arm64 runner,
//! pushes one image per architecture and one manifest list `ghcr.io/beyond10x/llm-gateway:<tag>`,
//! labels the image with the tag's commit, and refuses a version the registry already holds.
//!
//! The workflow is parsed as YAML and its `uses:` lines are read as text (a pin's tag comment is
//! not part of the YAML value); nothing here runs the workflow or reaches a registry.

use std::path::Path;

use serde_yaml::{Mapping, Value};

const IMAGE: &str = "ghcr.io/beyond10x/llm-gateway";
const TAG_PATTERN: &str = "[0-9]+.[0-9]+.[0-9]+";
/// Each platform with the GitHub-hosted runner that builds it natively.
const PLATFORMS: [(&str, &str); 2] = [
    ("linux/amd64", "ubuntu-24.04"),
    ("linux/arm64", "ubuntu-24.04-arm"),
];

fn text() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/release.yml");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

fn workflow() -> Value {
    serde_yaml::from_str(&text()).expect("release.yml is YAML")
}

fn mapping<'a>(value: &'a Value, what: &str) -> &'a Mapping {
    value
        .as_mapping()
        .unwrap_or_else(|| panic!("{what} is not a mapping: {value:?}"))
}

fn get<'a>(value: &'a Value, key: &str) -> &'a Value {
    value
        .get(key)
        .unwrap_or_else(|| panic!("no `{key}` in {value:?}"))
}

fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    get(value, key)
        .as_str()
        .unwrap_or_else(|| panic!("`{key}` is not a string in {value:?}"))
}

fn keys(value: &Mapping) -> Vec<&str> {
    value.keys().filter_map(Value::as_str).collect()
}

fn jobs(workflow: &Value) -> Vec<(&str, &Value)> {
    mapping(get(workflow, "jobs"), "jobs")
        .iter()
        .map(|(name, job)| (name.as_str().expect("a job name is a string"), job))
        .collect()
}

fn steps(job: &Value) -> &[Value] {
    get(job, "steps").as_sequence().expect("steps is a list")
}

/// Every step of every job, with its job's name.
fn all_steps(workflow: &Value) -> Vec<(&str, &Value)> {
    jobs(workflow)
        .into_iter()
        .flat_map(|(name, job)| steps(job).iter().map(move |step| (name, step)))
        .collect()
}

/// The action a step uses, without its ref: `docker/build-push-action`.
fn action(step: &Value) -> Option<&str> {
    let uses = step.get("uses")?.as_str()?;
    Some(uses.split_once('@').map_or(uses, |(name, _)| name))
}

/// The one job with a step that uses `name`, and that step.
fn job_using<'a>(workflow: &'a Value, name: &str) -> (&'a str, &'a Value, &'a Value) {
    let found: Vec<_> = jobs(workflow)
        .into_iter()
        .flat_map(|(job_name, job)| {
            steps(job)
                .iter()
                .filter(|step| action(step) == Some(name))
                .map(move |step| (job_name, job, step))
        })
        .collect();
    assert_eq!(found.len(), 1, "expected one step using {name}");
    found[0]
}

fn needs(job: &Value) -> Vec<&str> {
    match job.get("needs") {
        None => Vec::new(),
        Some(Value::String(one)) => vec![one.as_str()],
        Some(Value::Sequence(many)) => many.iter().filter_map(Value::as_str).collect(),
        Some(other) => panic!("needs is neither a name nor a list: {other:?}"),
    }
}

/// The `run:` script of every step of a job, joined.
fn scripts(job: &Value) -> String {
    steps(job)
        .iter()
        .filter_map(|step| step.get("run").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The job whose script checks the registry for the version before anything is pushed.
fn refusing_jobs(workflow: &Value) -> Vec<&str> {
    jobs(workflow)
        .into_iter()
        .filter(|(_, job)| {
            let script = scripts(job);
            script.contains("imagetools inspect") || script.contains("manifest inspect")
        })
        .map(|(name, _)| name)
        .collect()
}

#[test]
fn release_runs_on_a_pushed_version_tag_and_nothing_else() {
    let workflow = workflow();
    let on = mapping(get(&workflow, "on"), "on");
    assert_eq!(keys(on), ["push"], "the workflow has another trigger");
    let push = mapping(get(get(&workflow, "on"), "push"), "on.push");
    assert_eq!(
        keys(push),
        ["tags"],
        "a push trigger with branches or paths runs on more than a tag"
    );
    let tags: Vec<&str> = get(get(get(&workflow, "on"), "push"), "tags")
        .as_sequence()
        .expect("on.push.tags is a list")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(tags, [TAG_PATTERN]);
}

#[test]
fn release_permissions_are_contents_read_and_packages_write_only() {
    let workflow = workflow();
    let permissions = mapping(get(&workflow, "permissions"), "permissions");
    let mut granted: Vec<(&str, &str)> = permissions
        .iter()
        .map(|(scope, level)| {
            (
                scope.as_str().expect("a scope is a string"),
                level.as_str().expect("a level is a string"),
            )
        })
        .collect();
    granted.sort_unstable();
    assert_eq!(granted, [("contents", "read"), ("packages", "write")]);
    for (name, job) in jobs(&workflow) {
        assert!(
            job.get("permissions").is_none(),
            "job {name} sets its own permissions"
        );
    }
}

#[test]
fn release_every_action_is_pinned_by_commit_sha_with_its_tag_in_a_comment() {
    let text = text();
    let mut seen = 0;
    for line in text.lines() {
        let Some(rest) = line
            .trim_start()
            .trim_start_matches("- ")
            .strip_prefix("uses:")
        else {
            continue;
        };
        seen += 1;
        let (reference, comment) = rest.split_once('#').unwrap_or((rest, ""));
        let (_, sha) = reference
            .trim()
            .split_once('@')
            .unwrap_or_else(|| panic!("`{}` names no ref", line.trim()));
        assert!(
            sha.len() == 40
                && sha
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
            "`{}` is not pinned by a full commit SHA",
            line.trim()
        );
        assert!(
            comment.trim().starts_with('v'),
            "`{}` does not name its tag in a trailing comment",
            line.trim()
        );
    }
    let workflow = workflow();
    let parsed = all_steps(&workflow)
        .into_iter()
        .filter(|(_, step)| step.get("uses").is_some())
        .count();
    assert!(parsed > 0, "the workflow uses no action");
    assert_eq!(seen, parsed, "a `uses:` line was not read as text");
}

#[test]
fn release_builds_both_platforms_natively_from_the_root_dockerfile() {
    let workflow = workflow();
    let (name, job, build) = job_using(&workflow, "docker/build-push-action");
    let include = get(get(get(job, "strategy"), "matrix"), "include")
        .as_sequence()
        .expect("strategy.matrix.include is a list");
    let mut pairs: Vec<(&str, &str)> = include
        .iter()
        .map(|entry| (string(entry, "platform"), string(entry, "runner")))
        .collect();
    pairs.sort_unstable();
    assert_eq!(pairs, PLATFORMS, "job {name} builds other platforms");
    assert_eq!(string(job, "runs-on"), "${{ matrix.runner }}");
    let with = get(build, "with");
    assert_eq!(string(with, "platforms"), "${{ matrix.platform }}");
    assert_eq!(string(with, "context"), ".");
    assert_eq!(string(with, "file"), "Dockerfile");
    for (job_name, step) in all_steps(&workflow) {
        assert_ne!(
            action(step),
            Some("docker/setup-qemu-action"),
            "job {job_name} sets up emulation"
        );
    }
}

#[test]
fn release_pushes_one_image_per_platform_and_one_manifest_list_under_the_image_name() {
    let workflow = workflow();
    assert_eq!(string(get(&workflow, "env"), "IMAGE"), IMAGE);
    for line in text().lines().filter(|line| line.contains("ghcr.io/")) {
        assert!(
            line.contains(&format!("{IMAGE}\"")) || line.trim_end().ends_with(IMAGE),
            "`{}` names another image",
            line.trim()
        );
    }
    let (build_job, _, build) = job_using(&workflow, "docker/build-push-action");
    let outputs = string(get(build, "with"), "outputs");
    assert!(
        outputs.contains("name=${{ env.IMAGE }}")
            && outputs.contains("push-by-digest=true")
            && outputs.contains("push=true"),
        "the build pushes {outputs}, not this platform's image by digest"
    );
    let manifests: Vec<_> = jobs(&workflow)
        .into_iter()
        .filter(|(_, job)| scripts(job).contains("imagetools create"))
        .collect();
    assert_eq!(
        manifests.len(),
        1,
        "expected one job creating the manifest list"
    );
    let (manifest_name, manifest) = manifests[0];
    assert!(
        needs(manifest).contains(&build_job),
        "job {manifest_name} does not wait for {build_job}"
    );
    assert!(
        scripts(manifest).contains("--tag \"$IMAGE:$GITHUB_REF_NAME\""),
        "job {manifest_name} does not tag the list $IMAGE:$GITHUB_REF_NAME"
    );
}

#[test]
fn release_labels_the_image_with_the_tags_commit() {
    let workflow = workflow();
    let (_, job, build) = job_using(&workflow, "docker/build-push-action");
    let arguments = string(get(build, "with"), "build-args");
    assert!(
        arguments
            .lines()
            .any(|line| line.trim() == "SOURCE_SHA=${{ steps.revision.outputs.sha }}"),
        "build-args {arguments:?} does not pass SOURCE_SHA from the revision step"
    );
    let revision = steps(job)
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some("revision"))
        .expect("the build job has a step with id revision");
    let script = string(revision, "run");
    assert!(
        script.contains("git rev-parse")
            && script.contains("$GITHUB_REF^{commit}")
            && script.contains("sha=")
            && script.contains("$GITHUB_OUTPUT"),
        "the revision step does not resolve the tag to its commit: {script}"
    );
}

#[test]
fn release_refuses_an_existing_version_before_any_push() {
    let workflow = workflow();
    let refusing = refusing_jobs(&workflow);
    assert!(
        !refusing.is_empty(),
        "no job checks the registry for the version"
    );
    for (name, job) in jobs(&workflow) {
        let pushes = steps(job)
            .iter()
            .any(|step| action(step) == Some("docker/build-push-action"))
            || scripts(job).contains("imagetools create");
        if pushes {
            assert!(
                needs(job).iter().any(|need| refusing.contains(need))
                    || needs(job)
                        .iter()
                        .any(|need| needs(get(get(&workflow, "jobs"), need))
                            .iter()
                            .any(|inner| refusing.contains(inner))),
                "job {name} pushes without waiting for the existing-version check"
            );
        }
    }
    for name in &refusing {
        let script = scripts(get(get(&workflow, "jobs"), name));
        assert!(
            script.contains("already exists") && script.contains("exit 1"),
            "job {name} does not fail saying the version already exists"
        );
    }
}
