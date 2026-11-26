//! The recorded Runpod REST API v2 document (`tests/fixture/runpod-openapi-v2.json`) and the
//! check every fake answer of `tests/rest.rs` and every request the client sends there passes:
//! the operation exists at that path and method, and every field the body carries is declared
//! there, of the declared type. The document was written from the cited pages, not downloaded
//! (its `info.description` says so); the published one replaces it, with its digest, and then
//! this check says which field the client reads or writes that Runpod does not declare.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

const DOCUMENT: &str = include_str!("fixture/runpod-openapi-v2.json");
const DIGEST: &str = include_str!("fixture/runpod-openapi-v2.json.sha256");
/// The base path every operation of the document lives under.
const BASE: &str = "/v2";

fn document() -> &'static Value {
    static PARSED: OnceLock<Value> = OnceLock::new();
    PARSED.get_or_init(|| serde_json::from_str(DOCUMENT).expect("the recorded document is JSON"))
}

/// The operation of `method` whose path template matches `path` (base and query removed).
fn operation(method: &str, path: &str) -> &'static Value {
    let path = path
        .strip_prefix(BASE)
        .unwrap_or_else(|| panic!("{path}: not under {BASE}"));
    let path = path.split('?').next().unwrap_or_default();
    let paths = document()["paths"].as_object().expect("paths");
    let (_, item) = paths
        .iter()
        .find(|(template, _)| matches(template, path))
        .unwrap_or_else(|| panic!("the document declares no path {path}"));
    item.get(method.to_ascii_lowercase())
        .unwrap_or_else(|| panic!("the document declares no {method} {path}"))
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

/// `value` against `schema`: its type, its enum, and every member declared.
fn conform(at: &str, value: &Value, schema: &'static Value) {
    let schema = resolve(schema);
    if let Some(declared) = schema.get("type") {
        let allowed: Vec<&str> = match declared {
            Value::String(one) => vec![one.as_str()],
            Value::Array(many) => many.iter().filter_map(Value::as_str).collect(),
            _ => panic!("{at}: a type that is neither a name nor a list"),
        };
        let actual = type_of(value);
        assert!(
            allowed.contains(&actual) || (actual == "integer" && allowed.contains(&"number")),
            "{at}: a {actual}, the document declares {allowed:?}"
        );
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array) {
        assert!(
            options.contains(value),
            "{at}: {value} is not one of {options:?}"
        );
    }
    match value {
        Value::Object(members) => {
            let properties = schema.get("properties").and_then(Value::as_object);
            let additional = schema.get("additionalProperties");
            for required in schema
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                assert!(members.contains_key(required), "{at}: no {required}");
            }
            for (name, member) in members {
                let at = format!("{at}.{name}");
                match (
                    properties.and_then(|declared| declared.get(name)),
                    additional,
                ) {
                    (Some(declared), _) => conform(&at, member, declared),
                    (None, Some(Value::Object(_))) => {
                        conform(&at, member, additional.expect("additional"))
                    }
                    (None, _) => panic!("{at}: the document declares no such field"),
                }
            }
        }
        Value::Array(items) => {
            if let Some(item) = schema.get("items") {
                for (index, element) in items.iter().enumerate() {
                    conform(&format!("{at}[{index}]"), element, item);
                }
            }
        }
        _ => {}
    }
}

/// Asserts that `body`, answered `status` to `method path`, is an answer the document declares.
pub fn check_answer(method: &str, path: &str, status: u16, body: &Value) {
    let operation = operation(method, path);
    let schema = operation
        .pointer(&format!(
            "/responses/{status}/content/application~1json/schema"
        ))
        .unwrap_or_else(|| {
            panic!("{method} {path}: the document declares no JSON {status} answer")
        });
    conform(&format!("{method} {path} {status}"), body, schema);
}

/// Asserts that a request line `<method> <path> HTTP/1.1` and its body are a request the document
/// declares: the operation, each query parameter, and the body's fields.
pub fn check_request(line: &str, body: &[u8]) {
    let mut parts = line.split(' ');
    let (method, target) = (parts.next().expect("method"), parts.next().expect("target"));
    let operation = operation(method, target);
    if let Some((_, query)) = target.split_once('?') {
        for pair in query.split('&') {
            let name = pair.split('=').next().unwrap_or_default();
            let declared = operation["parameters"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|parameter| parameter["in"] == "query" && parameter["name"] == name);
            assert!(
                declared,
                "{line}: the document declares no query parameter {name}"
            );
        }
    }
    match operation.pointer("/requestBody/content/application~1json/schema") {
        Some(schema) => {
            let value: Value = serde_json::from_slice(body).expect("a JSON request body");
            conform(line, &value, schema);
        }
        None => assert!(
            body.is_empty(),
            "{line}: a body the document does not declare"
        ),
    }
}

/// The recorded document is the one its digest names, and says it was written, not downloaded.
#[test]
fn the_recorded_v2_document_matches_its_digest_and_says_how_it_was_made() {
    let digest = Sha256::digest(DOCUMENT.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    let recorded = DIGEST.split_whitespace().next().expect("a digest");
    assert_eq!(
        hex, recorded,
        "tests/fixture/runpod-openapi-v2.json changed without its digest"
    );
    let description = document()["info"]["description"]
        .as_str()
        .expect("description");
    assert!(
        description.starts_with("NOT DOWNLOADED. Written on 2026-11-26"),
        "{description}"
    );
    assert_eq!(document()["servers"][0]["url"], "https://api.runpod.io/v2");
    for (path, method) in [
        ("/v2/pods", "GET"),
        ("/v2/pods", "POST"),
        ("/v2/pods/abc", "DELETE"),
        ("/v2/pods/abc/action", "POST"),
        ("/v2/billing/balance", "GET"),
        ("/v2/billing/pods", "GET"),
    ] {
        let _ = operation(method, path);
    }
}
