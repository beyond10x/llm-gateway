//! The test step is one task and two CI steps with the same commands: an untimed build of the
//! tests, then the timed run under `.config/nextest.toml`, where a test over 60 s is killed and
//! fails and the run ends at 120 s. Decision D89: the 120 s bound counts the run's wall clock;
//! the build, and a wait for a build slot before it, are not the test step.

use super::{read, workflow};
use serde_yaml::Value;

const BUILD_STEP: &str = "cargo nextest run --workspace --locked --no-run";
const TEST_STEP: &str = "cargo run --quiet --locked -p b10x-llm-gateway-xtask -- test";

fn runs(steps: &Value) -> Vec<(String, String)> {
    steps
        .as_sequence()
        .expect("steps")
        .iter()
        .filter_map(|step| {
            Some((
                step["name"].as_str()?.to_owned(),
                step["run"].as_str()?.to_owned(),
            ))
        })
        .collect()
}

fn commands(task: &Value) -> Vec<String> {
    task["cmds"]
        .as_sequence()
        .expect("cmds")
        .iter()
        .map(|command| match command {
            Value::String(line) => line.clone(),
            other => format!("task: {}", other["task"].as_str().expect("a task")),
        })
        .collect()
}

fn runs_tests(command: &str) -> bool {
    command.contains("cargo test")
        || command.contains("cargo nextest")
        || command.contains(TEST_STEP)
}

#[test]
fn the_test_step_is_the_same_build_and_timed_run_in_the_local_gate_and_in_ci() {
    let workflow = workflow();
    let steps = runs(&workflow["jobs"]["serving-crates"]["steps"]);
    let tests: Vec<(&str, &str)> = steps
        .iter()
        .filter(|(_, run)| runs_tests(run))
        .map(|(name, run)| (name.as_str(), run.trim()))
        .collect();
    assert_eq!(
        tests,
        [("Build the tests", BUILD_STEP), ("Rust tests", TEST_STEP)],
        "CI builds the tests in a step of its own, then runs the timed step"
    );

    let taskfile: Value = serde_yaml::from_str(&read("Taskfile.yml")).expect("Taskfile.yml");
    assert_eq!(
        commands(&taskfile["tasks"]["test"]),
        [BUILD_STEP, TEST_STEP]
    );
    let rust = commands(&taskfile["tasks"]["rust"]);
    assert_eq!(
        rust.first().map(String::as_str),
        Some("task: test"),
        "{rust:?}"
    );
    for (name, task) in taskfile["tasks"].as_mapping().expect("tasks") {
        if name.as_str() == Some("test") {
            continue;
        }
        for command in commands(task) {
            assert!(
                !runs_tests(&command) || command == "task: test",
                "task {name:?} runs tests outside the test step: {command}"
            );
        }
    }
}

#[test]
fn a_test_over_60_s_is_killed_and_fails_and_the_run_ends_at_120_s() {
    let config: toml::Table = toml::from_str(&read(".config/nextest.toml")).expect("nextest.toml");
    let profile = &config["profile"]["default"];
    let slow = &profile["slow-timeout"];
    assert_eq!(slow["period"].as_str(), Some("60s"));
    assert_eq!(slow["terminate-after"].as_integer(), Some(1));
    assert_eq!(profile["global-timeout"].as_str(), Some("120s"));
    assert_eq!(profile["junit"]["path"].as_str(), Some("junit.xml"));
}

/// The nextest the configuration requires is the one CI pins, so an older one on a developer's
/// `PATH` is refused before it runs a test.
#[test]
fn the_configuration_requires_the_nextest_ci_pins() {
    let config: toml::Table = toml::from_str(&read(".config/nextest.toml")).expect("nextest.toml");
    let workflow = workflow();
    let pinned = workflow["jobs"]["serving-crates"]["steps"]
        .as_sequence()
        .expect("steps")
        .iter()
        .find(|step| step["name"].as_str() == Some("Install cargo-nextest"))
        .and_then(|step| step["env"]["NEXTEST_VERSION"].as_str())
        .expect("CI pins a version");
    assert_eq!(config["nextest-version"].as_str(), Some(pinned));
}

#[test]
fn ci_installs_cargo_nextest_pinned_by_version_and_digest_before_the_test_step() {
    let workflow = workflow();
    let steps = workflow["jobs"]["serving-crates"]["steps"]
        .as_sequence()
        .expect("steps");
    let position = |name: &str| {
        steps
            .iter()
            .position(|step| step["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("no step {name:?}"))
    };
    assert!(position("Install cargo-nextest") < position("Rust tests"));
    let install = &steps[position("Install cargo-nextest")];
    let version = install["env"]["NEXTEST_VERSION"]
        .as_str()
        .expect("a version");
    assert_eq!(version.split('.').count(), 3, "{version}");
    assert!(
        version.split('.').all(|part| part.parse::<u32>().is_ok()),
        "{version}"
    );
    let digest = install["env"]["NEXTEST_SHA256"].as_str().expect("a digest");
    assert!(
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "{digest}"
    );
    let run = install["run"].as_str().expect("a script");
    assert!(run.contains("sha256sum --check --strict"), "{run}");
    assert!(
        run.contains("--version | grep -F \"cargo-nextest $NEXTEST_VERSION \""),
        "{run}"
    );
}

#[test]
fn every_crate_with_integration_tests_builds_one_test_binary_from_tests_main_rs() {
    let root = super::repository();
    let manifest: toml::Table = toml::from_str(&read("Cargo.toml")).expect("Cargo.toml");
    let members = manifest["workspace"]["members"]
        .as_array()
        .expect("members");
    for member in members {
        let member = member.as_str().expect("a member path");
        let tests = root.join(member).join("tests");
        let Ok(entries) = std::fs::read_dir(&tests) else {
            continue;
        };
        let package: toml::Table = toml::from_str(&read(&format!("{member}/Cargo.toml")))
            .unwrap_or_else(|error| panic!("{member}/Cargo.toml: {error}"));
        assert_eq!(
            package["package"]
                .get("autotests")
                .and_then(toml::Value::as_bool),
            Some(false),
            "{member}: cargo builds every tests/*.rs as a test binary of its own unless \
             `autotests = false`"
        );
        let targets = package
            .get("test")
            .and_then(toml::Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(targets.len(), 1, "{member}: one [[test]] target");
        assert_eq!(
            targets[0]["path"].as_str(),
            Some("tests/main.rs"),
            "{member}"
        );
        let main = std::fs::read_to_string(tests.join("main.rs"))
            .unwrap_or_else(|error| panic!("{member}/tests/main.rs: {error}"));
        for entry in entries {
            let path = entry.expect("an entry").path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
                continue;
            }
            let name = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .expect("a name");
            if name == "main" {
                continue;
            }
            assert!(
                main.lines().any(|line| line == format!("mod {name};")),
                "{member}/tests/{name}.rs is no module of tests/main.rs: cargo would build it as \
                 a test binary of its own"
            );
        }
    }
}
