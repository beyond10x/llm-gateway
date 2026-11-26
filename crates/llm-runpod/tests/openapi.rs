//! Runpod's published REST API v2 document (`tests/fixture/runpod-openapi-v2.json`, downloaded
//! from `https://docs.runpod.io/api-reference-v2/openapi.json` on 2026-11-26, its SHA-256 beside
//! it) and the check every fake answer of `tests/rest.rs` and every request the client sends there
//! passes: the operation exists at that path and method, every query parameter is declared there
//! (and one of its values, when the document lists them), and every field a body carries is
//! declared, of the declared type, with every required field present.
//!
//! The document's server is `https://api.runpod.io` and its paths start with `/v2/`; the client's
//! base `https://api.runpod.io/v2` followed by its own paths makes the same URLs. A schema is
//! read through `$ref`, `allOf` (whose parts' fields are one object's), `anyOf` and `oneOf` (at
//! least one alternative). A field no part declares is refused even where the document leaves
//! extra fields open: a fake answer holds only what Runpod says it sends.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt::Write as _, sync::OnceLock};

const DOCUMENT: &str = include_str!("fixture/runpod-openapi-v2.json");
const DIGEST: &str = include_str!("fixture/runpod-openapi-v2.json.sha256");
/// The prefix every operation the client calls lives under.
const PREFIX: &str = "/v2/";

fn document() -> &'static Value {
    static PARSED: OnceLock<Value> = OnceLock::new();
    PARSED.get_or_init(|| serde_json::from_str(DOCUMENT).expect("the published document is JSON"))
}

/// The path item whose template matches `path` (query removed) and its operation of `method`.
fn operation(method: &str, path: &str) -> (&'static Value, &'static Value) {
    let path = path.split('?').next().unwrap_or_default();
    assert!(path.starts_with(PREFIX), "{path}: not under {PREFIX}");
    let paths = document()["paths"].as_object().expect("paths");
    let (_, item) = paths
        .iter()
        .find(|(template, _)| matches(template, path))
        .unwrap_or_else(|| panic!("the document declares no path {path}"));
    let operation = item
        .get(method.to_ascii_lowercase())
        .unwrap_or_else(|| panic!("the document declares no {method} {path}"));
    (item, operation)
}

fn matches(template: &str, path: &str) -> bool {
    let (template, path): (Vec<&str>, Vec<&str>) =
        (template.split('/').collect(), path.split('/').collect());
    template.len() == path.len()
        && template
            .iter()
            .zip(&path)
            .all(|(want, got)| (want.starts_with('{') && !got.is_empty()) || want == got)
}

fn resolve(schema: &'static Value) -> &'static Value {
    match schema.get("$ref").and_then(Value::as_str) {
        Some(reference) => {
            let pointer = reference.strip_prefix('#').expect("a local reference");
            resolve(
                document()
                    .pointer(pointer)
                    .unwrap_or_else(|| panic!("{reference}")),
            )
        }
        None => schema,
    }
}

fn type_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) if number.is_i64() || number.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// What a schema and its `allOf` parts declare together about an object or an array.
#[derive(Default)]
struct Declared {
    properties: BTreeMap<&'static str, Vec<&'static Value>>,
    required: Vec<&'static str>,
    additional: Option<&'static Value>,
    items: Vec<&'static Value>,
    /// Whether any part says what an object's members are.
    members: bool,
}

fn declared(schema: &'static Value, into: &mut Declared) {
    let schema = resolve(schema);
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        into.members = true;
        for (name, property) in properties {
            into.properties.entry(name).or_default().push(property);
        }
    }
    if let Some(additional) = schema.get("additionalProperties") {
        into.members = true;
        if additional.is_object() {
            into.additional = Some(additional);
        }
    }
    into.required.extend(
        schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str),
    );
    if let Some(items) = schema.get("items") {
        into.items.push(items);
    }
    for part in schema
        .get("allOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        declared(part, into);
    }
}

/// `value`'s type, enum and alternatives against `schema` and each of its `allOf` parts.
fn shallow(at: &str, value: &Value, schema: &'static Value) -> Result<(), String> {
    let schema = resolve(schema);
    if let Some(declared) = schema.get("type") {
        let allowed: Vec<&str> = match declared {
            Value::String(one) => vec![one.as_str()],
            Value::Array(many) => many.iter().filter_map(Value::as_str).collect(),
            _ => return Err(format!("{at}: a type that is neither a name nor a list")),
        };
        let actual = type_of(value);
        if !(allowed.contains(&actual) || (actual == "integer" && allowed.contains(&"number"))) {
            return Err(format!(
                "{at}: a {actual}, the document declares {allowed:?}"
            ));
        }
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array)
        && !options.contains(value)
    {
        return Err(format!("{at}: {value} is not one of {options:?}"));
    }
    for part in schema
        .get("allOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        shallow(at, value, part)?;
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(alternatives) = schema.get(key).and_then(Value::as_array) {
            let failures: Vec<String> = alternatives
                .iter()
                .filter_map(|alternative| conform(at, value, alternative).err())
                .collect();
            if failures.len() == alternatives.len() {
                return Err(format!("{at}: no alternative of {key} holds: {failures:?}"));
            }
        }
    }
    Ok(())
}

/// `value` against `schema`: its type, its enum, its alternatives, every required member present
/// and every member declared.
fn conform(at: &str, value: &Value, schema: &'static Value) -> Result<(), String> {
    shallow(at, value, schema)?;
    let mut parts = Declared::default();
    declared(schema, &mut parts);
    match value {
        Value::Object(members) if parts.members => {
            for required in &parts.required {
                if !members.contains_key(*required) {
                    return Err(format!("{at}: no {required}"));
                }
            }
            for (name, member) in members {
                let at = format!("{at}.{name}");
                match (parts.properties.get(name.as_str()), parts.additional) {
                    (Some(declared), _) => {
                        for schema in declared {
                            conform(&at, member, schema)?;
                        }
                    }
                    (None, Some(additional)) => conform(&at, member, additional)?,
                    (None, None) => {
                        return Err(format!("{at}: the document declares no such field"));
                    }
                }
            }
        }
        Value::Array(items) => {
            for (index, element) in items.iter().enumerate() {
                for item in &parts.items {
                    conform(&format!("{at}[{index}]"), element, item)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// The JSON schema of `response`, under either JSON media type the document uses.
fn json_schema(response: &'static Value) -> Option<&'static Value> {
    let content = resolve(response).get("content")?;
    ["application/json", "application/problem+json"]
        .into_iter()
        .find_map(|media| content.get(media)?.get("schema"))
}

/// Asserts that `body`, answered `status` to `method path`, is an answer the document declares:
/// under that status, or under `default` when the operation names no such status.
pub fn check_answer(method: &str, path: &str, status: u16, body: &Value) {
    let (_, operation) = operation(method, path);
    let responses = &operation["responses"];
    let response = responses
        .get(status.to_string())
        .or_else(|| responses.get("default"))
        .unwrap_or_else(|| panic!("{method} {path}: the document declares no {status} answer"));
    let schema = json_schema(response).unwrap_or_else(|| {
        panic!("{method} {path}: the document declares no JSON {status} answer")
    });
    if let Err(failure) = conform(&format!("{method} {path} {status}"), body, schema) {
        panic!("{failure}");
    }
}

/// Every parameter of an operation: its path item's and its own, each resolved.
fn parameters(item: &'static Value, operation: &'static Value) -> Vec<&'static Value> {
    [item, operation]
        .into_iter()
        .filter_map(|holder| holder.get("parameters")?.as_array())
        .flatten()
        .map(resolve)
        .collect()
}

/// Asserts that a request line `<method> <path> HTTP/1.1` and its body are a request the document
/// declares: the operation, each query parameter (a string one also by its value), and the
/// body's fields.
pub fn check_request(line: &str, body: &[u8]) {
    let mut parts = line.split(' ');
    let (method, target) = (parts.next().expect("method"), parts.next().expect("target"));
    let (item, operation) = operation(method, target);
    let declared = parameters(item, operation);
    if let Some((_, query)) = target.split_once('?') {
        for pair in query.split('&') {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            let parameter = declared
                .iter()
                .find(|parameter| parameter["in"] == "query" && parameter["name"] == name)
                .unwrap_or_else(|| {
                    panic!("{line}: the document declares no query parameter {name}")
                });
            if let Some(schema) = parameter.get("schema")
                && resolve(schema)["type"] == "string"
                && let Err(failure) = conform(
                    &format!("{line} {name}"),
                    &Value::String(value.to_owned()),
                    schema,
                )
            {
                panic!("{failure}");
            }
        }
    }
    match operation.pointer("/requestBody/content/application~1json/schema") {
        Some(schema) => {
            let value: Value = serde_json::from_slice(body).expect("a JSON request body");
            if let Err(failure) = conform(line, &value, schema) {
                panic!("{failure}");
            }
        }
        None => assert!(
            body.is_empty(),
            "{line}: a body the document does not declare"
        ),
    }
}

/// The recorded document is the one its digest names: Runpod's published v2 document, served
/// from `https://api.runpod.io` with every path under `/v2/`, declaring each call the client
/// makes.
#[test]
fn the_recorded_v2_document_is_the_published_one_and_matches_its_digest() {
    let digest = Sha256::digest(DOCUMENT.as_bytes());
    let hex = digest.iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    });
    let recorded = DIGEST.split_whitespace().next().expect("a digest");
    assert_eq!(
        hex, recorded,
        "tests/fixture/runpod-openapi-v2.json changed without its digest"
    );
    assert_eq!(document()["info"]["title"], "Runpod REST API");
    assert_eq!(document()["info"]["version"], "2.0.0");
    assert_eq!(document()["servers"][0]["url"], "https://api.runpod.io");
    for (path, method) in [
        ("/v2/pods", "GET"),
        ("/v2/pods", "POST"),
        ("/v2/pods/abc", "GET"),
        ("/v2/pods/abc", "DELETE"),
        ("/v2/pods/abc/logs", "GET"),
        ("/v2/pods/abc/action", "POST"),
        ("/v2/billing/balance", "GET"),
        ("/v2/billing/pods", "GET"),
    ] {
        let _ = operation(method, path);
    }
}

/// The client's default base is the document's server followed by `/v2`, so the paths it
/// appends (`/pods`, `/billing/pods`, …) are the document's.
#[test]
fn the_default_base_is_the_documents_server_and_its_v2_prefix() {
    let server = document()["servers"][0]["url"].as_str().expect("server");
    assert_eq!(
        llm_runpod::RestBinding::runpod().base_url,
        format!("{server}{}", PREFIX.trim_end_matches('/'))
    );
}

/// The check refuses what it exists to refuse: an undeclared field, a missing required one, a
/// value outside an enum, and an undeclared query parameter or value.
#[test]
fn the_check_refuses_an_undeclared_or_missing_field_and_a_value_off_its_enum() {
    let balance = |body: Value| {
        std::panic::catch_unwind(|| check_answer("GET", "/v2/billing/balance", 200, &body))
    };
    assert!(balance(serde_json::json!({"balanceAmount": 1.5, "billingMode": "PREPAID"})).is_ok());
    assert!(balance(serde_json::json!({"balanceAmount": 1.5})).is_err());
    assert!(
        balance(serde_json::json!({"balanceAmount": 1.5, "billingMode": "PREPAID", "x": 1}))
            .is_err()
    );
    assert!(balance(serde_json::json!({"balanceAmount": 1.5, "billingMode": "CREDIT"})).is_err());
    let request = |line: &'static str| std::panic::catch_unwind(|| check_request(line, b""));
    assert!(request("GET /v2/billing/pods?bucketSize=day HTTP/1.1").is_ok());
    assert!(request("GET /v2/billing/pods?bucketSize=fortnight HTTP/1.1").is_err());
    assert!(request("GET /v2/billing/pods?since=yesterday HTTP/1.1").is_err());
    assert!(request("GET /v2/pods?cursor=c2 HTTP/1.1").is_ok());
}

/// A container's start is read from the pod's log stream, `GET /v2/pods/{id}/logs`: the
/// document declares the request the client sends (`source=system`, `tail=100` inside its
/// bound), answers it with `text/event-stream`, and names the `data` payload's fields the client
/// reads (`source`, `line`, `ts`). A source the document does not list is refused.
#[test]
fn the_pod_log_stream_declares_the_request_and_the_fields_a_container_start_is_read_from() {
    check_request(
        "GET /v2/pods/abc123/logs?source=system&tail=100 HTTP/1.1",
        b"",
    );
    assert!(
        std::panic::catch_unwind(|| check_request(
            "GET /v2/pods/abc123/logs?source=host HTTP/1.1",
            b""
        ))
        .is_err()
    );
    let (item, operation) = operation("GET", "/v2/pods/abc123/logs");
    let tail = parameters(item, operation)
        .into_iter()
        .find(|parameter| parameter["name"] == "tail")
        .expect("a tail parameter");
    assert!(tail["schema"]["maximum"].as_u64() >= Some(100));
    let stream = &operation["responses"]["200"]["content"]["text/event-stream"];
    let example = stream["examples"]["logLine"]["value"]
        .as_str()
        .expect("an example event");
    let data: Value = serde_json::from_str(
        example
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .expect("a data line"),
    )
    .expect("a JSON payload");
    let mut fields: Vec<&str> = data
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    assert_eq!(fields, ["line", "source", "ts"]);
}
