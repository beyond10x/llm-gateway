//! story:gateway-translation, correction round 1: no key of a translated request is dropped.
//!
//! llm 0.6.0's chat and responses decoders passed over keys at levels below the top they did not
//! read, and the gateway closed those levels itself; llm 0.7.0's decoders refuse every unknown key
//! at every level by its path. For each client wire, one request carrying every nested level the
//! decoder reads is prepared, then the same request with one unknown key added at each level:
//! every one must be `translation-unsupported`, whichever side holds the subset.

use llm_gateway::{
    Label, RefusalCode, RelayModel, ToolCalling, TranslationRequest, Translator, Wire,
};
use llm_gateway_cli::LlmTranslator;
use serde_json::{Value, json};

fn model(upstream: Wire) -> RelayModel {
    RelayModel::new(
        Label::new("code").unwrap(),
        Label::new("served").unwrap(),
        vec![upstream],
    )
    .unwrap()
    .with_tool_calling(ToolCalling::Parsed)
}

fn prepare(ingress: Wire, upstream: Wire, body: &Value) -> Option<RefusalCode> {
    let model = model(upstream);
    LlmTranslator::new()
        .prepare(&TranslationRequest {
            ingress,
            upstream,
            model: &model,
            body: body.to_string().as_bytes(),
        })
        .err()
}

/// `base` with `"unknown_key": 1` added to the object at `pointer`.
fn with_unknown_key(base: &Value, pointer: &str) -> Value {
    with(base, pointer, "unknown_key", json!(1))
}

/// `base` with `key` set to `value` in the object at `pointer`.
fn with(base: &Value, pointer: &str, key: &str, value: Value) -> Value {
    let mut body = base.clone();
    body.pointer_mut(pointer)
        .and_then(Value::as_object_mut)
        .unwrap_or_else(|| panic!("no object at {pointer}"))
        .insert(key.to_owned(), value);
    body
}

/// Prepares `base`, then asserts each probe is `translation-unsupported`, naming every one that
/// is not.
fn assert_closed(ingress: Wire, upstream: Wire, base: &Value, probes: &[(&str, Value)]) {
    assert_eq!(
        prepare(ingress, upstream, base),
        None,
        "the base request is refused"
    );
    let dropped: Vec<String> = probes
        .iter()
        .filter_map(|(level, body)| {
            let refused = prepare(ingress, upstream, body);
            (refused != Some(RefusalCode::TranslationUnsupported))
                .then(|| format!("{level}: {refused:?}"))
        })
        .collect();
    assert!(
        dropped.is_empty(),
        "keys prepared instead of refused: {dropped:#?}"
    );
}

fn chat_base() -> Value {
    json!({
        "model": "code",
        "messages": [
            {"role": "system", "content": "Be brief."},
            {"role": "user", "content": [{"type": "text", "text": "Weather in Kyoto?"}]},
            {"role": "assistant", "content": "Checking.", "tool_calls": [
                {"id": "call_1", "type": "function", "function": {"name": "lookup", "arguments": "{}"}}
            ]},
            {"role": "tool", "tool_call_id": "call_1", "content": "Sunny"}
        ],
        "tools": [{"type": "function", "function": {"name": "lookup", "description": "Find a city", "parameters": {"type": "object"}}}],
        "tool_choice": {"type": "function", "function": {"name": "lookup"}},
        "stream": true,
        "stream_options": {"include_usage": true}
    })
}

/// Chat (`llm_chat::decode_ingress_request`, llm 0.7.0 `crates/llm-chat/src/ingress.rs` and
/// `docs/chat.md`): every level it reads, closed.
#[test]
fn translation_guard_chat_refuses_an_unknown_key_at_every_level() {
    let base = chat_base();
    let probes = [
        ("top level", with_unknown_key(&base, "")),
        ("stream_options", with_unknown_key(&base, "/stream_options")),
        ("a system message", with_unknown_key(&base, "/messages/0")),
        ("a user message", with_unknown_key(&base, "/messages/1")),
        (
            "messages[].name",
            with(&base, "/messages/1", "name", json!("alice")),
        ),
        (
            "a content part",
            with_unknown_key(&base, "/messages/1/content/0"),
        ),
        (
            "messages[].content[].cache_control",
            with(
                &base,
                "/messages/1/content/0",
                "cache_control",
                json!({"type": "ephemeral"}),
            ),
        ),
        (
            "an assistant message",
            with_unknown_key(&base, "/messages/2"),
        ),
        (
            "messages[].reasoning_content",
            with(
                &base,
                "/messages/2",
                "reasoning_content",
                json!("the user asks"),
            ),
        ),
        (
            "messages[].refusal",
            with(&base, "/messages/2", "refusal", Value::Null),
        ),
        (
            "a tool call",
            with_unknown_key(&base, "/messages/2/tool_calls/0"),
        ),
        (
            "a tool call of another type",
            with(&base, "/messages/2/tool_calls/0", "type", json!("custom")),
        ),
        (
            "a tool call's function",
            with_unknown_key(&base, "/messages/2/tool_calls/0/function"),
        ),
        ("a tool message", with_unknown_key(&base, "/messages/3")),
        (
            "tool_calls on a user message",
            with(&base, "/messages/1", "tool_calls", json!([])),
        ),
        (
            "tool_call_id on an assistant message",
            with(&base, "/messages/2", "tool_call_id", json!("call_1")),
        ),
        ("a tool", with_unknown_key(&base, "/tools/0")),
        (
            "a tool's function",
            with_unknown_key(&base, "/tools/0/function"),
        ),
        (
            "tools[].function.strict",
            with(&base, "/tools/0/function", "strict", json!(true)),
        ),
        ("tool_choice", with_unknown_key(&base, "/tool_choice")),
        (
            "a tool_choice of another type",
            with(&base, "/tool_choice", "type", json!("allowed_tools")),
        ),
        (
            "tool_choice.function",
            with_unknown_key(&base, "/tool_choice/function"),
        ),
    ];
    assert_closed(Wire::Chat, Wire::Messages, &base, &probes);
}

fn responses_base() -> Value {
    let message = |role: &str, part: &str, text: &str| json!({"type": "message", "role": role, "content": [{"type": part, "text": text}]});
    json!({
        "model": "code",
        "input": [
            message("developer", "input_text", "Be brief."),
            message("user", "input_text", "Weather in Kyoto?"),
            {"type": "function_call", "call_id": "call_1", "name": "lookup", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "call_1", "output": "{\"ok\":true,\"output\":\"Sunny\"}"},
            message("assistant", "output_text", "Sunny."),
            message("user", "input_text", "Thanks")
        ],
        "tools": [{"type": "function", "name": "lookup", "description": "Find a city", "parameters": {"type": "object"}, "strict": false}],
        "tool_choice": {"type": "function", "name": "lookup"},
        "reasoning": {"effort": "low"},
        "stream": true,
        "store": false,
        "include": ["reasoning.encrypted_content"]
    })
}

/// Responses (`llm_responses::ingest_request`, llm 0.7.0 `crates/llm-responses/src/request.rs`):
/// every level it reads, closed. An item's `id` is not a probe: llm 0.7.0 reads it as the
/// unforwarded name `input-item-ids` (Codex sends one on every item), and the request carrying
/// it is prepared. Nor is an assistant `output_text` part's empty `annotations`, which a
/// replayed answer carries and which loses nothing; a non-empty one is refused.
#[test]
fn translation_guard_responses_refuses_an_unknown_key_at_every_level() {
    let base = responses_base();
    let probes = [
        ("top level", with_unknown_key(&base, "")),
        ("reasoning", with_unknown_key(&base, "/reasoning")),
        ("a developer message", with_unknown_key(&base, "/input/0")),
        (
            "a developer message's part",
            with_unknown_key(&base, "/input/0/content/0"),
        ),
        ("a user message", with_unknown_key(&base, "/input/1")),
        (
            "a user message's part",
            with_unknown_key(&base, "/input/1/content/0"),
        ),
        ("a function call", with_unknown_key(&base, "/input/2")),
        (
            "input[].status",
            with(&base, "/input/2", "status", json!("completed")),
        ),
        (
            "a function call output",
            with_unknown_key(&base, "/input/3"),
        ),
        ("an assistant message", with_unknown_key(&base, "/input/4")),
        (
            "input[].content[].annotations, not empty",
            with(
                &base,
                "/input/4/content/0",
                "annotations",
                json!([{"type": "url_citation"}]),
            ),
        ),
        ("a tool", with_unknown_key(&base, "/tools/0")),
        ("tool_choice", with_unknown_key(&base, "/tool_choice")),
    ];
    assert_closed(Wire::Responses, Wire::Chat, &base, &probes);
    assert_eq!(
        prepare(
            Wire::Responses,
            Wire::Chat,
            &with(&base, "/input/1", "id", json!("msg_1"))
        ),
        None,
        "input[].id"
    );
    assert_eq!(
        prepare(
            Wire::Responses,
            Wire::Chat,
            &with(&base, "/input/4/content/0", "annotations", json!([]))
        ),
        None,
        "an assistant part's empty annotations"
    );
}

fn messages_base() -> Value {
    json!({
        "model": "code",
        "max_tokens": 64,
        "system": [{"type": "text", "text": "Be brief."}],
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": "Weather in Kyoto?"}]},
            {"role": "assistant", "content": [
                {"type": "text", "text": "Checking."},
                {"type": "tool_use", "id": "toolu_1", "name": "lookup", "input": {}}
            ]},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "Sunny", "is_error": false}]}
        ],
        "tools": [{"name": "lookup", "description": "Find a city", "input_schema": {"type": "object"}}],
        "tool_choice": {"type": "tool", "name": "lookup"},
        "output_config": {"effort": "low"},
        "temperature": 0.5,
        "stream": true
    })
}

/// Messages (`llm_messages::decode_request`, llm 0.7.0 `crates/llm-messages/src/codec.rs`): the
/// decoder closes every level it reads itself; the probes pin that.
#[test]
fn translation_guard_messages_refuses_an_unknown_key_at_every_level() {
    let base = messages_base();
    let probes = [
        ("top level", with_unknown_key(&base, "")),
        ("a system block", with_unknown_key(&base, "/system/0")),
        ("a message", with_unknown_key(&base, "/messages/0")),
        (
            "a text block",
            with_unknown_key(&base, "/messages/0/content/0"),
        ),
        (
            "a tool_use block",
            with_unknown_key(&base, "/messages/1/content/1"),
        ),
        (
            "a tool_result block",
            with_unknown_key(&base, "/messages/2/content/0"),
        ),
        ("a tool", with_unknown_key(&base, "/tools/0")),
        ("tool_choice", with_unknown_key(&base, "/tool_choice")),
        ("output_config", with_unknown_key(&base, "/output_config")),
    ];
    assert_closed(Wire::Messages, Wire::Chat, &base, &probes);
}
