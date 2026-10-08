//! Row D1: the root `Dockerfile` builds `b10x-llm-gateway` into a distroless, non-root image that
//! exposes 8080, runs the binary as its entrypoint and labels the source revision. llmgw's
//! `Dockerfile:13-18` at 048ebd8 is the model.
//!
//! The file is read as text and split into instructions; no container tool runs here. A build is
//! recorded in `docs/verification/2026-10-08-container-image.md`.

use std::path::Path;

const BINARY: &str = "b10x-llm-gateway";

/// One instruction: its keyword in upper case and its arguments, continuations joined.
struct Instruction {
    keyword: String,
    arguments: String,
}

fn dockerfile() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Dockerfile");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

/// Splits the file into instructions, skipping comments and blank lines and joining lines that
/// end in a backslash.
fn instructions(text: &str) -> Vec<Instruction> {
    let mut joined = Vec::new();
    let mut pending = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if pending.is_empty() && (trimmed.is_empty() || trimmed.starts_with('#')) {
            continue;
        }
        if let Some(head) = trimmed.strip_suffix('\\') {
            pending.push_str(head);
            pending.push(' ');
        } else {
            pending.push_str(trimmed);
            joined.push(std::mem::take(&mut pending));
        }
    }
    assert!(
        pending.is_empty(),
        "the Dockerfile ends inside a continuation"
    );
    joined
        .into_iter()
        .map(|line| {
            let (keyword, arguments) = line.split_once(char::is_whitespace).unwrap_or((&line, ""));
            Instruction {
                keyword: keyword.to_ascii_uppercase(),
                arguments: arguments.trim().to_string(),
            }
        })
        .collect()
}

/// The final stage: its `FROM` and every instruction after it.
fn final_stage(all: &[Instruction]) -> &[Instruction] {
    let start = all
        .iter()
        .rposition(|instruction| instruction.keyword == "FROM")
        .expect("the Dockerfile has a FROM");
    &all[start..]
}

/// The image a `FROM` names: its first argument that is not a `--flag`.
fn from_image(from: &Instruction) -> &str {
    from.arguments
        .split_whitespace()
        .find(|word| !word.starts_with("--"))
        .expect("FROM names an image")
}

/// The exec-form `ENTRYPOINT` of the final stage.
fn entrypoint(stage: &[Instruction]) -> Vec<String> {
    let entrypoint = stage
        .iter()
        .rfind(|instruction| instruction.keyword == "ENTRYPOINT")
        .expect("the final stage has an ENTRYPOINT");
    serde_json::from_str(&entrypoint.arguments).expect("ENTRYPOINT is in exec form")
}

#[test]
fn d1_the_final_stage_is_distroless() {
    let all = instructions(&dockerfile());
    let image = from_image(&final_stage(&all)[0]);
    assert!(
        image.starts_with("gcr.io/distroless/"),
        "final base image {image} is not distroless"
    );
}

#[test]
fn d1_the_final_stage_runs_as_a_non_root_user() {
    let all = instructions(&dockerfile());
    let user = final_stage(&all)
        .iter()
        .rfind(|instruction| instruction.keyword == "USER")
        .expect("the final stage sets USER");
    let name = user.arguments.split(':').next().unwrap_or_default();
    assert!(
        !name.is_empty() && name != "root" && name != "0",
        "USER {} is root",
        user.arguments
    );
}

#[test]
fn d1_the_final_stage_exposes_8080() {
    let all = instructions(&dockerfile());
    let exposed = final_stage(&all)
        .iter()
        .filter(|instruction| instruction.keyword == "EXPOSE")
        .flat_map(|instruction| instruction.arguments.split_whitespace())
        .any(|port| port == "8080" || port == "8080/tcp");
    assert!(exposed, "the final stage does not EXPOSE 8080");
}

#[test]
fn d1_the_entrypoint_is_the_gateway_binary_built_from_this_workspace() {
    let all = instructions(&dockerfile());
    let stage = final_stage(&all);
    let entrypoint = entrypoint(stage);
    let program = entrypoint.first().expect("ENTRYPOINT names a program");
    assert_eq!(
        Path::new(program)
            .file_name()
            .and_then(|name| name.to_str()),
        Some(BINARY),
        "ENTRYPOINT {entrypoint:?}"
    );
    // The program is copied in from a build stage, which builds the workspace's binary package.
    let copied = stage.iter().any(|instruction| {
        instruction.keyword == "COPY"
            && instruction.arguments.starts_with("--from=")
            && instruction.arguments.split_whitespace().last() == Some(program.as_str())
    });
    assert!(copied, "nothing copies {program} in from a build stage");
    let built = all.iter().any(|instruction| {
        instruction.keyword == "RUN"
            && instruction.arguments.contains("cargo build")
            && instruction.arguments.contains("--locked")
            && instruction.arguments.contains("-p b10x-llm-gateway-cli")
    });
    assert!(
        built,
        "no build stage runs cargo build -p b10x-llm-gateway-cli"
    );
}

#[test]
fn d1_a_label_carries_the_source_revision_from_a_build_argument() {
    let all = instructions(&dockerfile());
    let stage = final_stage(&all);
    let label = stage
        .iter()
        .filter(|instruction| instruction.keyword == "LABEL")
        .find_map(|instruction| {
            instruction
                .arguments
                .strip_prefix("org.opencontainers.image.revision=")
        })
        .expect("the final stage labels org.opencontainers.image.revision");
    let argument = label
        .trim_matches('"')
        .trim_start_matches('$')
        .trim_start_matches('{')
        .trim_end_matches('}');
    assert!(
        label.trim_matches('"').starts_with('$') && !argument.is_empty(),
        "the revision label {label} is not a build argument"
    );
    let declared = stage.iter().any(|instruction| {
        instruction.keyword == "ARG" && instruction.arguments.split('=').next() == Some(argument)
    });
    assert!(
        declared,
        "the final stage does not declare ARG {argument}, so the label would be empty"
    );
}
