//! A fixture of the `connectors` CLI (beyond10x/connectors v0.37.0) for `tests/transport.rs`.
//!
//! It is reached through a symlink named `connectors` in a per-test directory, and keeps all its
//! state there: `script.json` holds the canned answers, `calls.jsonl` gets one line per
//! invocation with its argv and its input document. It answers `operations describe`,
//! `approvals prepare` and `approvals issue` by default, and `operations invoke` only from the
//! script. An issued proof records the exact input it was issued for, and an invoke of a write
//! whose proof was issued for another input, or that names no proof, is refused the way
//! connectors refuses it, and recorded as `proof_mismatch`. A `pod.create` body carrying a key
//! outside connectors v0.37.0's `body_keys` is refused `invalid_input` before any request, and
//! recorded as `body_keys_refused`. Every answer goes out in connectors'
//! published framing (`succeed`, `fail`): a script may write the bare `result` or `error`, and
//! the fixture frames it; `raw` is printed as given. It opens no socket and reads no key.

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
const CONNECTION_REVISION: &str = "conn-rev-3";
/// connectors v0.37.0 `adapters/catalog/providers/runpod/operations.json`, `pod.create`
/// `body_keys`: the only keys a `pod.create` `body` may carry.
const POD_CREATE_BODY_KEYS: [&str; 15] = [
    "name",
    "imageName",
    "gpuTypeIds",
    "gpuCount",
    "containerDiskInGb",
    "volumeInGb",
    "volumeMountPath",
    "ports",
    "env",
    "cloudType",
    "dataCenterIds",
    "interruptible",
    "dockerStartCmd",
    "dockerEntrypoint",
    "networkVolumeId",
];

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

/// connectors' failure framing (v0.37.0 `contracts/cli/v1alpha1/semantics.md:178-181`): stdout
/// stays empty and stderr gets one `{"ok":false,"error":{"code":"failure","data":…}}` envelope,
/// whose `data` is the `Failure` (`code`, `stage`, `next_action`, `mutation`). A script
/// writes a failure as `{"error":{"code":…,"data":…}}`; this is where it is framed.
fn fail(code: u8, error: &Value) -> ExitCode {
    let inner = error.get("error").unwrap_or(error);
    let mut data = inner.get("data").cloned().unwrap_or_else(|| json!({}));
    if data.get("code").is_none()
        && let Some(object) = data.as_object_mut()
    {
        object.insert(
            "code".to_owned(),
            inner.get("code").cloned().unwrap_or(Value::Null),
        );
    }
    eprintln!(
        "{}",
        json!({"ok": false, "error": {"code": "failure", "data": data}})
    );
    ExitCode::from(code)
}

/// connectors' success framing: one `{"ok":true,"result":…}` envelope on stdout. A script
/// writes a success as the bare `result`; an answer that already carries `ok` is printed as is.
fn succeed(result: &Value) {
    if result.get("ok").is_some() {
        println!("{result}");
    } else {
        println!("{}", json!({"ok": true, "result": result}));
    }
}

fn main() -> ExitCode {
    let command_line: Vec<String> = std::env::args().collect();
    // A child started by `hold_stdout_ms`: it only keeps the inherited stdout open, the way a
    // helper or an auto-started owner process the CLI launches would, and then exits.
    if command_line.get(1).map(String::as_str) == Some("__hold_stdout") {
        let ms = command_line
            .get(2)
            .and_then(|ms| ms.parse().ok())
            .unwrap_or(0);
        thread::sleep(Duration::from_millis(ms));
        return ExitCode::SUCCESS;
    }
    let Some(dir) = command_line
        .first()
        .and_then(|zero| Path::new(zero).parent())
        .map(Path::to_path_buf)
    else {
        return ExitCode::from(90);
    };
    let args = command_line.get(1..).unwrap_or_default().to_vec();
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
        // `connections describe` and `connections revalidate` (connectors ess/domains/cli.yaml
        // `ConnectionDescribeResult`, `ConnectionRevalidateInput`): the connection's revision is
        // `conn-rev-3`, and a revalidate naming another one is refused.
        "connections describe" => Some(json!({"connection": {
            "summary": {
                "adapter": option(&args, "--adapter"),
                "connection": option(&args, "--connection"),
                "revision": CONNECTION_REVISION,
                "state": "pending",
            },
            "stale": false,
        }})),
        "connections revalidate" => {
            if option(&args, "--expected-revision") == Some(CONNECTION_REVISION) {
                Some(json!({"connection": {"summary": {"revision": CONNECTION_REVISION}}}))
            } else {
                Some(json!({"error": {"code": "revision_conflict"}}))
            }
        }
        "operations invoke" => {
            if WRITES.contains(&operation.as_str()) && !proof_matches(&args, input.as_deref()) {
                record["proof_mismatch"] = json!(true);
            }
            if operation == "pod.create" {
                let refused = unadmitted_body_keys(input.as_deref());
                if !refused.is_empty() {
                    record["body_keys_refused"] = json!(refused);
                }
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
    if let Some(refused) = refusal(&record) {
        return refused;
    }
    if let Some(answer) = scripted(&dir, &key, seen) {
        return play(&answer);
    }
    match answer {
        Some(answer) if answer.get("error").is_none() => {
            succeed(&answer);
            ExitCode::SUCCESS
        }
        Some(error) => fail(2, &error),
        None => ExitCode::from(70),
    }
}

/// The scripted answer for this call, if any: the `seen`-th of the key's list, the last one
/// repeating.
fn scripted(dir: &Path, key: &str, seen: usize) -> Option<Value> {
    let script: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("script.json")).ok()?).ok()?;
    let answers = script.get(key)?.as_array()?;
    answers.get(seen).or_else(|| answers.last()).cloned()
}

fn play(answer: &Value) -> ExitCode {
    if let Some(ms) = answer.get("hold_stdout_ms").and_then(Value::as_u64)
        && let Ok(exe) = std::env::current_exe()
    {
        let _ = std::process::Command::new(exe)
            .args(["__hold_stdout", &ms.to_string()])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    if let Some(ms) = answer.get("sleep_ms").and_then(Value::as_u64) {
        thread::sleep(Duration::from_millis(ms));
    }
    let code = answer.get("exit").and_then(Value::as_u64).unwrap_or(0);
    let code = u8::try_from(code).unwrap_or(1);
    if let Some(raw) = answer.get("raw").and_then(Value::as_str) {
        print!("{raw}");
    } else if let Some(stdout) = answer.get("stdout") {
        if stdout.get("error").is_some() && stdout.get("ok").is_none() {
            return fail(if code == 0 { 1 } else { code }, stdout);
        }
        succeed(stdout);
    }
    // connectors v0.37.0 `contracts/cli/v1alpha1/semantics.md`: a failure leaves stdout empty
    // and writes its `{"ok":false,"error":…}` envelope to stderr.
    if let Some(stderr) = answer.get("stderr") {
        eprintln!("{stderr}");
    }
    ExitCode::from(code)
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

/// The keys of a `pod.create` input's `body` that connectors v0.37.0 does not admit, in order.
/// An input that is not an object with an object `body` is refused as a whole.
fn unadmitted_body_keys(input: Option<&str>) -> Vec<String> {
    let body = input
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .and_then(|document| document.get("body").cloned());
    let Some(Value::Object(body)) = body else {
        return vec!["<body>".to_owned()];
    };
    body.keys()
        .filter(|key| !POD_CREATE_BODY_KEYS.contains(&key.as_str()))
        .cloned()
        .collect()
}

/// The refusals connectors makes before any request, for what was recorded about this call.
fn refusal(record: &Value) -> Option<ExitCode> {
    if record.get("proof_mismatch").is_some() {
        return Some(fail(
            2,
            &json!({"error": {"code": "not_granted", "data": {"kind": "approval", "code": "proof_mismatch"}}}),
        ));
    }
    // connectors v0.37.0 docs/catalog-runpod.md:88-89: "A body carrying any other key … is
    // refused as `invalid_input` before any request."
    if record.get("body_keys_refused").is_some() {
        return Some(fail(
            2,
            &json!({"error": {"code": "failure", "data": {
                "kind": "usage", "code": "invalid_input", "stage": "arguments",
                "next_action": "retry_explicitly",
                "mutation": {"classification": "not_attempted", "replayed": false},
            }}}),
        ));
    }
    None
}
