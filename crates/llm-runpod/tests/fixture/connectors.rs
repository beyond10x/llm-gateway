//! A fixture of the `connectors` CLI (beyond10x/connectors v0.36.0) for `tests/transport.rs`.
//!
//! It is reached through a symlink named `connectors` in a per-test directory, and keeps all its
//! state there: `script.json` holds the canned answers, `calls.jsonl` gets one line per
//! invocation with its argv and its input document. It answers `operations describe`,
//! `approvals prepare` and `approvals issue` by default, and `operations invoke` only from the
//! script. An issued proof records the exact input it was issued for, and an invoke of a write
//! whose proof was issued for another input, or that names no proof, is refused the way
//! connectors refuses it, and recorded as `proof_mismatch`. It opens no socket and reads no key.

use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, OpenOptions},
    hash::{Hash, Hasher},
    io::Write,
    path::{Path, PathBuf},
    process::ExitCode,
    thread,
    time::Duration,
};

use serde_json::{Value, json};

const WRITES: [&str; 2] = ["pod.create", "pod.terminate"];

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

fn subject(input: &str) -> String {
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    format!("{:016x}{:016x}", hasher.finish(), input.len())
}

fn fail(code: u8, error: &Value) -> ExitCode {
    println!("{error}");
    ExitCode::from(code)
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let Some(dir) = argv
        .first()
        .and_then(|zero| Path::new(zero).parent())
        .map(Path::to_path_buf)
    else {
        return ExitCode::from(90);
    };
    let args = argv.get(1..).unwrap_or_default().to_vec();
    let input = option(&args, "--input-file")
        .and_then(|path| fs::read_to_string(path).ok())
        .or_else(|| option(&args, "--input-json").map(str::to_owned));
    let operation = option(&args, "--operation").unwrap_or("").to_owned();
    let command = args
        .iter()
        .filter(|arg| !arg.starts_with("--") && *arg != "json")
        .take(2)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let key = format!("{command} {operation}");

    let mut record = json!({"argv": args, "input": input});
    let answer = match command.as_str() {
        "operations describe" => Some(json!({
            "adapter": option(&args, "--adapter"),
            "revision": "revision-7",
            "schema": format!("schema-{operation}"),
            "operation": {"id": operation},
            "stale": false,
        })),
        "approvals prepare" => Some(json!({
            "adapter": option(&args, "--adapter"),
            "preparation": {"subject_sha256": subject(input.as_deref().unwrap_or(""))},
        })),
        "approvals issue" => Some(issue(&args, input.as_deref(), &mut record)),
        "operations invoke" => {
            if WRITES.contains(&operation.as_str()) && !proof_matches(&args, input.as_deref()) {
                record["proof_mismatch"] = json!(true);
            }
            None
        }
        _ => None,
    };
    let calls = dir.join("calls.jsonl");
    let seen = fs::read_to_string(&calls)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["key"] == json!(key))
        .count();
    record["key"] = json!(key);
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&calls) {
        let _ = writeln!(file, "{record}");
    }
    if record.get("proof_mismatch").is_some() {
        return fail(
            2,
            &json!({"error": {"code": "not_granted", "data": {"kind": "approval", "code": "proof_mismatch"}}}),
        );
    }
    if let Some(answer) = scripted(&dir, &key, seen) {
        return play(&answer);
    }
    match answer {
        Some(answer) if answer.get("error").is_none() => {
            println!("{answer}");
            ExitCode::SUCCESS
        }
        Some(error) => fail(2, &error),
        None => ExitCode::from(70),
    }
}

/// The scripted answer for this call, if any: the `seen`-th of the key's list, the last one
/// repeating.
fn scripted(dir: &Path, key: &str, seen: usize) -> Option<Value> {
    let script: Value = serde_json::from_str(&fs::read_to_string(dir.join("script.json")).ok()?)
        .ok()?;
    let answers = script.get(key)?.as_array()?;
    answers.get(seen).or_else(|| answers.last()).cloned()
}

fn play(answer: &Value) -> ExitCode {
    if let Some(ms) = answer.get("sleep_ms").and_then(Value::as_u64) {
        thread::sleep(Duration::from_millis(ms));
    }
    if let Some(raw) = answer.get("raw").and_then(Value::as_str) {
        print!("{raw}");
    } else if let Some(stdout) = answer.get("stdout") {
        println!("{stdout}");
    }
    let code = answer.get("exit").and_then(Value::as_u64).unwrap_or(0);
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

fn issue(args: &[String], input: Option<&str>, record: &mut Value) -> Value {
    let input = input.unwrap_or("");
    let expected = subject(input);
    if option(args, "--approve-subject") != Some(expected.as_str()) {
        record["subject_mismatch"] = json!(true);
        return json!({"error": {"code": "not_granted"}});
    }
    let Some(output) = option(args, "--proof-output").map(PathBuf::from) else {
        return json!({"error": {"code": "invalid_input"}});
    };
    if !output.is_absolute() || output.exists() {
        record["proof_output_refused"] = json!(true);
        return json!({"error": {"code": "invalid_input"}});
    }
    let proof = json!({"reference": "proof", "evidence": {"input": input}});
    if fs::write(&output, proof.to_string()).is_err() {
        return json!({"error": {"code": "internal"}});
    }
    json!({
        "adapter": option(args, "--adapter"),
        "reference": "proof",
        "subject_sha256": expected,
        "disposition": "published",
    })
}

fn proof_matches(args: &[String], input: Option<&str>) -> bool {
    let Some(proof) = option(args, "--approval-file")
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    else {
        return false;
    };
    option(args, "--idempotency-key").is_some_and(|key| !key.is_empty())
        && proof["evidence"]["input"].as_str() == input
}
