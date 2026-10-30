//! A fixture of the `connectors` CLI (beyond10x/connectors 0.60.0) for the transport tests.
//!
//! It is reached through a symlink named `connectors` in a per-test directory, and keeps all its
//! state there: `script.json` holds the canned answers, `calls.jsonl` gets one line per
//! invocation with its argv and its input document. It accepts only the command lines connectors
//! 0.60.0 keeps (`CHANGELOG.md`, `## 0.60.0`):
//!
//! * `invoke <connector>/<instance> <operation>` with `--connection`, `--schema`, `--revision`,
//!   one of `--input-json` or `--input-file`, and for a write `--approval-file` and
//!   `--idempotency-key` (`contracts/cli/v1alpha1/semantics.md`, "Invoking an operation";
//!   `ess/domains/cli.yaml` `InvokeInput`). Any other option, `--adapter`, `--connector`,
//!   `--instance`, `--endpoint` and `--operation` included, is a parse refusal, as is a line
//!   with other than two positional arguments. Its answer names `connector` and `instance` and
//!   carries no `adapter` (`InvokeResult`).
//! * `operations describe`, `approvals prepare`, `approvals issue`, `connections describe` and
//!   `connections revalidate`, selecting the instance with `--connector <c> --instance <i>`, or
//!   with the deprecated `--adapter <alias>`, which the record marks `deprecated_selector`.
//!
//! The removed command lines, the `invoke` subcommand of `operations`, `serve` and the top-level
//! `describe`, are the fixed `cli_parse` envelope on stderr, stdout empty, exit 2 (connectors
//! 0.60.0 `apps/connectors/tests/removed_forms.rs`), and so is an `invoke` line this fixture
//! refuses as it parses it. The record marks a removed line `removed`, and keys every refused
//! line `refused <operation>`, so it never counts as an invoke.
//!
//! It answers `operations describe` for the operations of its catalog ([`CATALOG`]), refusing
//! any other, `approvals prepare` and `approvals issue` by default, and `invoke` only from the
//! script. An issued proof records the exact input it was issued for, and an invoke of a write
//! whose proof was issued for another input is refused the way connectors refuses it, and
//! recorded as `proof_mismatch`; a write with no proof is `approval_required`, and a read given
//! a proof or an idempotency key `invalid_input`. A `pod.create` body carrying a key outside
//! connectors' `body_keys` is refused `invalid_input` before any request, and recorded as
//! `body_keys_refused`. Every answer goes out in connectors' published framing (`succeed`,
//! `fail`): a script may write the bare `result` or `error`, and the fixture frames it; `raw`
//! is printed as given. It opens no socket and reads no key.

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
/// The operations this fixture's Runpod catalog selects. `operations describe` of any other is
/// refused `not_found`, as connectors refuses an operation its catalog does not hold.
const CATALOG: [&str; 4] = ["pod.create", "pods.list", "pod.terminate", "billing.pods"];
const CONNECTION_REVISION: &str = "conn-rev-3";
/// The one connector this fixture configures (connectors 0.60.0
/// `adapters/catalog/providers/runpod/connector.json`, `connector_id`).
const CONNECTOR: &str = "runpod";
/// connectors 0.60.0's parse refusal under `--output json`, byte for byte
/// (`apps/connectors/tests/removed_forms.rs`, `JSON_PARSE`).
const PARSE_REFUSAL: &str = "{\"error\":{\"code\":\"cli_parse\",\"data\":{}},\"ok\":false}";
/// The global options, each with a value, that may lead a command line.
const GLOBALS: [&str; 3] = ["--output", "--config", "--state"];
/// The options of `invoke` this fixture models, each with a value (connectors 0.60.0
/// `ess/domains/cli.yaml` `InvokeInput`).
const INVOKE_OPTIONS: [&str; 7] = [
    "--connection",
    "--schema",
    "--revision",
    "--input-json",
    "--input-file",
    "--approval-file",
    "--idempotency-key",
];
/// connectors `adapters/catalog/providers/runpod/operations.json`, `pod.create` `body_keys`:
/// the only keys a `pod.create` `body` may carry.
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

/// The value of `--<name> <value>` or `--<name>=<value>`.
fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == name {
            return rest.next().map(String::as_str);
        }
        if let Some(value) = arg.strip_prefix(name).and_then(|v| v.strip_prefix('=')) {
            return Some(value);
        }
    }
    None
}

fn subject(input: &str) -> String {
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    format!("{:016x}{:016x}", hasher.finish(), input.len())
}

/// connectors' failure framing (`contracts/cli/v1alpha1/semantics.md`, "--output json"):
/// stdout stays empty and stderr gets one `{"ok":false,"error":{"code":"failure","data":…}}`
/// envelope, whose `data` is the `Failure` (`code`, `stage`, `next_action`, `mutation`). A
/// script writes a failure as `{"error":{"code":…,"data":…}}`; this is where it is framed.
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

/// The fixed parse refusal: stdout empty, the `cli_parse` envelope on stderr, exit 2.
fn parse_refusal() -> ExitCode {
    eprintln!("{PARSE_REFUSAL}");
    ExitCode::from(2)
}

/// connectors' success framing: one `{"ok":true,"result":…}` envelope on stdout. A script
/// writes a success as the bare `result`; an answer that already carries `ok` is printed as is.
/// An `invoke` result names the target's `connector` and `instance` and carries no `adapter`,
/// as connectors 0.60.0 answers it (`apps/connectors/src/local/invoke.rs`, `dispatch`).
fn succeed(result: &Value, target: Option<&Target>) {
    if result.get("ok").is_some() {
        println!("{result}");
        return;
    }
    let mut result = result.clone();
    if let (Some(target), Some(fields)) = (target, result.as_object_mut()) {
        fields.remove("adapter");
        fields.insert("connector".to_owned(), json!(target.connector));
        fields.insert("instance".to_owned(), json!(target.instance));
    }
    println!("{}", json!({"ok": true, "result": result}));
}

/// The instance an `invoke` names.
struct Target {
    connector: String,
    instance: String,
}

/// connectors-core `valid_id`: 1..=128 bytes of `[A-Za-z0-9._-]`.
fn valid_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

/// The positional arguments of an `invoke` line, `None` when it is a parse refusal: an option
/// this fixture does not model, an option without its value, or the input given twice.
fn invoke_positionals(args: &[String]) -> Option<Vec<String>> {
    let mut positional = Vec::new();
    let mut rest = args.iter();
    let mut trailing = false;
    while let Some(arg) = rest.next() {
        if trailing || !arg.starts_with('-') {
            positional.push(arg.clone());
            continue;
        }
        if arg == "--" {
            trailing = true;
            continue;
        }
        let name = arg.split_once('=').map_or(arg.as_str(), |(name, _)| name);
        if !INVOKE_OPTIONS.contains(&name) {
            return None;
        }
        if !arg.contains('=') {
            rest.next()?;
        }
    }
    let sources = ["--input-json", "--input-file"]
        .iter()
        .filter(|name| option(args, name).is_some())
        .count();
    (sources <= 1).then_some(positional)
}

/// The instance a selecting command names: `--connector <c> --instance <i>`, or the
/// deprecated `--adapter <alias>` alone (`semantics.md`, "Selecting an instance"). `Err` is
/// the refusal connectors answers.
fn selection(args: &[String], record: &mut Value) -> Result<String, Value> {
    match (
        option(args, "--adapter"),
        option(args, "--connector"),
        option(args, "--instance"),
    ) {
        (Some(alias), None, None) => {
            record["deprecated_selector"] = json!(true);
            Ok(alias.to_owned())
        }
        (None, Some(CONNECTOR), Some(instance)) => Ok(instance.to_owned()),
        (None, Some(_), Some(_)) => Err(json!({"error": {"code": "not_found", "data": {
            "kind": "operational", "stage": "admission", "next_action": "check_configuration",
        }}})),
        _ => Err(json!({"error": {"code": "invalid_input", "data": {
            "kind": "usage", "stage": "arguments", "next_action": "none",
        }}})),
    }
}

/// `operations describe`: the operation's schema, or `not_found` for one the catalog does not
/// offer.
fn describe(instance: &str, operation: &str) -> Value {
    if !CATALOG.contains(&operation) {
        return json!({"error": {"code": "not_found"}});
    }
    json!({
        "adapter": instance,
        "connector": CONNECTOR,
        "instance": instance,
        "revision": "revision-7",
        "schema": format!("schema-{operation}"),
        "operation": {"id": operation},
        "stale": false,
    })
}

/// The answer the fixture gives a selecting command itself, before any script.
fn selected_answer(
    command: &str,
    args: &[String],
    instance: &str,
    operation: &str,
    input: Option<&str>,
    record: &mut Value,
) -> Option<Value> {
    let named = |mut answer: Value| {
        if let Some(fields) = answer.as_object_mut() {
            fields.insert("adapter".to_owned(), json!(instance));
            fields.insert("connector".to_owned(), json!(CONNECTOR));
            fields.insert("instance".to_owned(), json!(instance));
        }
        answer
    };
    match command {
        "operations describe" => Some(describe(instance, operation)),
        "approvals prepare" => Some(named(json!({
            "preparation": {"subject_sha256": subject(input.unwrap_or(""))},
        }))),
        "approvals issue" => Some(issue(args, input, record)).map(|answer| {
            if answer.get("error").is_some() {
                answer
            } else {
                named(answer)
            }
        }),
        // `connections describe` and `connections revalidate` (connectors ess/domains/cli.yaml
        // `ConnectionDescribeResult`, `ConnectionRevalidateInput`): the connection's revision is
        // `conn-rev-3`, and a revalidate naming another one is refused.
        "connections describe" => Some(json!({"connection": {
            "summary": {
                "adapter": instance,
                "connector": CONNECTOR,
                "instance": instance,
                "connection": option(args, "--connection"),
                "revision": CONNECTION_REVISION,
                "state": "pending",
            },
            "stale": false,
        }})),
        "connections revalidate" => {
            if option(args, "--expected-revision") == Some(CONNECTION_REVISION) {
                Some(json!({"connection": {"summary": {"revision": CONNECTION_REVISION}}}))
            } else {
                Some(json!({"error": {"code": "revision_conflict"}}))
            }
        }
        _ => None,
    }
}

/// What a command line is, after its leading global options.
enum Line {
    /// `invoke <target> <operation>`.
    Invoke(Target, String),
    /// A command line connectors 0.60.0 refuses as it parses it.
    Refused,
    /// `invoke` whose target does not name the configured connector: `not_found`.
    Unknown,
    /// Any other command, by its two command words.
    Command(String),
}

fn line(rest: &[String], record: &mut Value) -> Line {
    let words: Vec<&str> = rest
        .iter()
        .take_while(|arg| !arg.starts_with('-'))
        .take(2)
        .map(String::as_str)
        .collect();
    match words.as_slice() {
        ["invoke", ..] => {
            let Some(positional) = invoke_positionals(&rest[1..]) else {
                return Line::Refused;
            };
            let [target, operation] = positional.as_slice() else {
                return Line::Refused;
            };
            record["target"] = json!(target);
            record["operation"] = json!(operation);
            match target.split_once('/') {
                Some((CONNECTOR, instance)) if valid_id(instance) && valid_id(operation) => {
                    Line::Invoke(
                        Target {
                            connector: CONNECTOR.to_owned(),
                            instance: instance.to_owned(),
                        },
                        operation.clone(),
                    )
                }
                _ => Line::Unknown,
            }
        }
        ["operations", "invoke"] | ["serve" | "describe", ..] => {
            record["removed"] = json!(true);
            Line::Refused
        }
        _ => Line::Command(words.join(" ")),
    }
}

/// The call's key, `<command> <operation>`, and the answer the fixture gives it itself when no
/// script does. A refused line is never counted as an invoke: its key is `refused <operation>`.
fn keyed(
    parsed: &Line,
    rest: &[String],
    input: Option<&str>,
    record: &mut Value,
) -> (String, Option<Value>) {
    match parsed {
        Line::Invoke(_, operation) => {
            invoke_checks(rest, operation, input, record);
            (format!("invoke {operation}"), None)
        }
        Line::Refused | Line::Unknown => {
            let operation = record["operation"]
                .as_str()
                .or_else(|| option(rest, "--operation"))
                .unwrap_or("")
                .to_owned();
            let command = if matches!(parsed, Line::Refused) {
                "refused"
            } else {
                "invoke"
            };
            (format!("{command} {operation}"), None)
        }
        Line::Command(command) => {
            let operation = option(rest, "--operation").unwrap_or("");
            let answer = match selection(rest, record) {
                Ok(instance) => {
                    record["instance"] = json!(instance);
                    selected_answer(command, rest, &instance, operation, input, record)
                }
                Err(refusal) => Some(refusal),
            };
            (format!("{command} {operation}"), answer)
        }
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
    let mut skip = 0;
    while args
        .get(skip)
        .is_some_and(|arg| GLOBALS.contains(&arg.as_str()))
    {
        skip += 2;
    }
    let rest = args.get(skip..).unwrap_or_default();
    let input = option(rest, "--input-file")
        .and_then(|path| fs::read_to_string(path).ok())
        .or_else(|| option(rest, "--input-json").map(str::to_owned));

    let mut record = json!({"argv": args, "input": input});
    let parsed = line(rest, &mut record);
    let (key, answer) = keyed(&parsed, rest, input.as_deref(), &mut record);
    let target = match &parsed {
        Line::Invoke(target, _) => Some(target),
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
    match parsed {
        Line::Refused => return parse_refusal(),
        Line::Unknown => {
            return fail(
                1,
                &json!({"error": {"code": "not_found", "data": {
                    "kind": "operational", "stage": "admission",
                    "next_action": "check_configuration",
                }}}),
            );
        }
        Line::Invoke(..) | Line::Command(_) => {}
    }
    if let Some(refused) = refusal(&record) {
        return refused;
    }
    if let Some(answer) = scripted(&dir, &key, seen) {
        return play(&answer, target);
    }
    match answer {
        Some(answer) if answer.get("error").is_none() => {
            succeed(&answer, None);
            ExitCode::SUCCESS
        }
        Some(error) => fail(2, &error),
        None => ExitCode::from(70),
    }
}

/// What connectors 0.60.0 checks of an `invoke` before it dispatches, recorded for
/// [`refusal`].
fn invoke_checks(args: &[String], operation: &str, input: Option<&str>, record: &mut Value) {
    if WRITES.contains(&operation) {
        if option(args, "--approval-file").is_none() {
            record["approval_required"] = json!(true);
        } else if !proof_matches(args, input) {
            record["proof_mismatch"] = json!(true);
        }
    } else if option(args, "--approval-file").is_some()
        || option(args, "--idempotency-key").is_some()
    {
        record["read_with_approval"] = json!(true);
    }
    if operation == "pod.create" {
        let refused = unadmitted_body_keys(input);
        if !refused.is_empty() {
            record["body_keys_refused"] = json!(refused);
        }
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

fn play(answer: &Value, target: Option<&Target>) -> ExitCode {
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
        succeed(stdout, target);
    }
    // A failure leaves stdout empty and writes its `{"ok":false,"error":…}` envelope to stderr.
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

/// The keys of a `pod.create` input's `body` that connectors does not admit, in order. An input
/// that is not an object with an object `body` is refused as a whole.
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
    // connectors 0.60.0 `invoke.rs` `prepare_with`: a write without `--approval-file` is
    // `approval_required` at `admission`, exit 1, naming the steps that issue the proof.
    if record.get("approval_required").is_some() {
        return Some(fail(
            1,
            &json!({"error": {"code": "failure", "data": {
                "kind": "operational", "code": "approval_required", "stage": "admission",
                "next_action": "none",
                "approval_steps": ["approvals prepare", "approvals clock-check", "approvals issue"],
            }}}),
        ));
    }
    // A read given `--approval-file` or `--idempotency-key` is `invalid_input` naming it.
    if record.get("read_with_approval").is_some() {
        return Some(fail(
            2,
            &json!({"error": {"code": "failure", "data": {
                "kind": "usage", "code": "invalid_input", "stage": "arguments",
                "next_action": "none",
            }}}),
        ));
    }
    if record.get("proof_mismatch").is_some() {
        return Some(fail(
            2,
            &json!({"error": {"code": "not_granted", "data": {"kind": "approval", "code": "proof_mismatch"}}}),
        ));
    }
    // connectors docs/catalog-runpod.md: "A body carrying any other key … is refused as
    // `invalid_input` before any request."
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
