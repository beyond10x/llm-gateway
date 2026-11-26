//! The non-streamed answer of a streamed one (story:runpod-rest-host).
//!
//! The gateway sends every call to a Runpod pod streamed, because Runpod's proxy closes a
//! connection on which the pod stays silent past its bound (100 s by Runpod's documentation,
//! Cloudflare's Proxy Read Timeout of 125 s by Cloudflare's), and a long non-streamed answer is
//! silent until its end. A client that did not ask for a stream gets the one body this module
//! makes of the events, as the same server would have answered it unstreamed:
//!
//! * chat (`OpenAI` chat completions): a `chat.completion` whose choices merge every chunk's
//!   delta: `content` and `reasoning_content` (or `reasoning`) concatenated, tool calls joined
//!   by their `index`, the last `finish_reason`, the `logprobs` content joined, and the usage
//!   chunk's `usage`. The stream must end with `[DONE]`.
//! * responses (`OpenAI` responses): the `response` of the `response.completed`,
//!   `response.incomplete` or `response.failed` event.
//! * messages (Anthropic messages): the `message` of `message_start` with every content block
//!   built from its start and deltas (`text`, `thinking`, `signature`, and a tool's
//!   `partial_json` read as its `input`), `message_delta`'s stop reason and usage merged in.
//!   The stream must end with `message_stop`.
//!
//! A stream longer than [`MAX_ASSEMBLED_BYTES`], an event that does not read as JSON, or an
//! `error` event makes no answer, and the gateway answers `upstream-failed`.

use llm_gateway::{Assembly, EventReader, StreamAssembler, Wire};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// The most bytes of one event stream read to assemble one answer.
pub const MAX_ASSEMBLED_BYTES: usize = 16 * 1024 * 1024;

/// The assembler the composition hands the relay: one [`Assembly`] per wire.
#[derive(Debug, Clone, Copy, Default)]
pub struct EventAssembler;

impl StreamAssembler for EventAssembler {
    fn begin(&self, wire: Wire) -> Box<dyn Assembly> {
        Box::new(Assembling {
            reader: EventReader::default(),
            read: 0,
            failed: false,
            answer: match wire {
                Wire::Chat => Answer::Chat(Chat::default()),
                Wire::Responses => Answer::Responses(None),
                Wire::Messages => Answer::Messages(Message::default()),
            },
        })
    }
}

/// One answer being assembled.
struct Assembling {
    reader: EventReader,
    /// Bytes of the stream read so far.
    read: usize,
    failed: bool,
    answer: Answer,
}

enum Answer {
    Chat(Chat),
    /// The final `response`, once read.
    Responses(Option<Value>),
    Messages(Message),
}

impl Assembly for Assembling {
    fn feed(&mut self, bytes: &[u8]) -> bool {
        self.read = self.read.saturating_add(bytes.len());
        if self.failed || self.read > MAX_ASSEMBLED_BYTES {
            self.failed = true;
            return false;
        }
        self.reader.feed(bytes);
        for event in self.reader.take() {
            if !self.answer.event(&event) {
                self.failed = true;
                return false;
            }
        }
        true
    }

    fn finish(self: Box<Self>) -> Option<Vec<u8>> {
        if self.failed {
            return None;
        }
        let answer = match self.answer {
            Answer::Chat(chat) => chat.finish()?,
            Answer::Responses(response) => response?,
            Answer::Messages(message) => message.finish()?,
        };
        serde_json::to_vec(&answer).ok()
    }
}

impl Answer {
    /// Reads one event's `data`; `false` for one that makes no answer.
    fn event(&mut self, data: &[u8]) -> bool {
        if let Self::Chat(chat) = self
            && data == b"[DONE]"
        {
            chat.done = true;
            return true;
        }
        let Ok(event) = serde_json::from_slice::<Value>(data) else {
            return false;
        };
        if event.get("type").and_then(Value::as_str) == Some("error") {
            return false;
        }
        match self {
            Self::Chat(chat) => chat.chunk(event),
            Self::Responses(response) => {
                if matches!(
                    event.get("type").and_then(Value::as_str),
                    Some("response.completed" | "response.incomplete" | "response.failed")
                ) {
                    *response = event.get("response").cloned();
                }
                true
            }
            Self::Messages(message) => message.event(&event),
        }
    }
}

/// A chat completion's chunks, merged.
#[derive(Default)]
struct Chat {
    /// The first chunk's top-level members but `object`, `choices` and `usage`.
    top: Option<Map<String, Value>>,
    choices: BTreeMap<u64, Choice>,
    usage: Option<Value>,
    done: bool,
}

#[derive(Default)]
struct Choice {
    role: Option<Value>,
    content: Option<String>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    refusal: Option<String>,
    tool_calls: BTreeMap<u64, ToolCall>,
    finish_reason: Option<Value>,
    logprobs: Option<Vec<Value>>,
}

#[derive(Default)]
struct ToolCall {
    id: Option<Value>,
    kind: Option<Value>,
    name: String,
    arguments: String,
}

/// Appends `value`, when it is a string, to `text`.
fn append(text: &mut Option<String>, value: Option<&Value>) {
    if let Some(piece) = value.and_then(Value::as_str) {
        text.get_or_insert_with(String::new).push_str(piece);
    }
}

impl Chat {
    fn chunk(&mut self, mut chunk: Value) -> bool {
        let Some(object) = chunk.as_object_mut() else {
            return false;
        };
        if object.get("error").is_some_and(|error| !error.is_null()) {
            return false;
        }
        if let Some(usage) = object.remove("usage").filter(|usage| !usage.is_null()) {
            self.usage = Some(usage);
        }
        let choices = object.remove("choices");
        if self.top.is_none() {
            object.remove("object");
            self.top = Some(std::mem::take(object));
        }
        for choice in choices
            .as_ref()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let index = choice.get("index").and_then(Value::as_u64).unwrap_or(0);
            let merged = self.choices.entry(index).or_default();
            if !merged.merge(choice) {
                return false;
            }
        }
        true
    }

    fn finish(self) -> Option<Value> {
        if !self.done {
            return None;
        }
        let mut answer = self.top.unwrap_or_default();
        answer.insert("object".to_owned(), json!("chat.completion"));
        let choices: Vec<Value> = self
            .choices
            .into_iter()
            .map(|(index, choice)| choice.finish(index))
            .collect();
        answer.insert("choices".to_owned(), Value::Array(choices));
        if let Some(usage) = self.usage {
            answer.insert("usage".to_owned(), usage);
        }
        Some(Value::Object(answer))
    }
}

impl Choice {
    fn merge(&mut self, choice: &Value) -> bool {
        if let Some(reason) = choice
            .get("finish_reason")
            .filter(|reason| !reason.is_null())
        {
            self.finish_reason = Some(reason.clone());
        }
        if let Some(content) = choice
            .pointer("/logprobs/content")
            .and_then(Value::as_array)
        {
            self.logprobs
                .get_or_insert_with(Vec::new)
                .extend(content.iter().cloned());
        }
        let Some(delta) = choice.get("delta") else {
            return true;
        };
        if let Some(role) = delta.get("role").filter(|role| !role.is_null()) {
            self.role = Some(role.clone());
        }
        append(&mut self.content, delta.get("content"));
        append(&mut self.reasoning_content, delta.get("reasoning_content"));
        append(&mut self.reasoning, delta.get("reasoning"));
        append(&mut self.refusal, delta.get("refusal"));
        for call in delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(index) = call.get("index").and_then(Value::as_u64) else {
                return false;
            };
            let merged = self.tool_calls.entry(index).or_default();
            if let Some(id) = call.get("id").filter(|id| !id.is_null()) {
                merged.id = Some(id.clone());
            }
            if let Some(kind) = call.get("type").filter(|kind| !kind.is_null()) {
                merged.kind = Some(kind.clone());
            }
            if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                merged.name.push_str(name);
            }
            if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str) {
                merged.arguments.push_str(arguments);
            }
        }
        true
    }

    fn finish(self, index: u64) -> Value {
        let mut message = Map::new();
        message.insert(
            "role".to_owned(),
            self.role.unwrap_or_else(|| json!("assistant")),
        );
        message.insert(
            "content".to_owned(),
            self.content.map_or(Value::Null, Value::String),
        );
        for (key, text) in [
            ("reasoning_content", self.reasoning_content),
            ("reasoning", self.reasoning),
            ("refusal", self.refusal),
        ] {
            if let Some(text) = text {
                message.insert(key.to_owned(), Value::String(text));
            }
        }
        if !self.tool_calls.is_empty() {
            let calls: Vec<Value> = self
                .tool_calls
                .into_values()
                .map(|call| {
                    json!({
                        "id": call.id.unwrap_or(Value::Null),
                        "type": call.kind.unwrap_or_else(|| json!("function")),
                        "function": {"name": call.name, "arguments": call.arguments},
                    })
                })
                .collect();
            message.insert("tool_calls".to_owned(), Value::Array(calls));
        }
        json!({
            "index": index,
            "message": message,
            "logprobs": self.logprobs.map_or(Value::Null, |content| json!({"content": content})),
            "finish_reason": self.finish_reason.unwrap_or(Value::Null),
        })
    }
}

/// A messages answer's events, merged.
#[derive(Default)]
struct Message {
    /// The `message` of `message_start`, with `message_delta` merged in.
    start: Option<Map<String, Value>>,
    blocks: BTreeMap<u64, Value>,
    /// A tool block's `partial_json` pieces, by block index.
    inputs: BTreeMap<u64, String>,
    stopped: bool,
}

impl Message {
    fn event(&mut self, event: &Value) -> bool {
        let index = event.get("index").and_then(Value::as_u64);
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                let Some(message) = event.get("message").and_then(Value::as_object) else {
                    return false;
                };
                self.start = Some(message.clone());
            }
            Some("content_block_start") => {
                let (Some(index), Some(block)) = (index, event.get("content_block")) else {
                    return false;
                };
                self.blocks.insert(index, block.clone());
            }
            Some("content_block_delta") => {
                let (Some(index), Some(delta)) = (index, event.get("delta")) else {
                    return false;
                };
                let Some(block) = self.blocks.get_mut(&index).and_then(Value::as_object_mut) else {
                    return false;
                };
                let text = |key: &str| delta.get(key).and_then(Value::as_str).unwrap_or_default();
                let mut extend = |key: &str, piece: &str| {
                    let joined = format!(
                        "{}{piece}",
                        block.get(key).and_then(Value::as_str).unwrap_or_default()
                    );
                    block.insert(key.to_owned(), Value::String(joined));
                };
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => extend("text", text("text")),
                    Some("thinking_delta") => extend("thinking", text("thinking")),
                    Some("signature_delta") => extend("signature", text("signature")),
                    Some("citations_delta") => {
                        if let Some(citation) = delta.get("citation") {
                            let citations = block
                                .entry("citations")
                                .or_insert_with(|| Value::Array(Vec::new()));
                            if let Some(list) = citations.as_array_mut() {
                                list.push(citation.clone());
                            }
                        }
                    }
                    Some("input_json_delta") => self
                        .inputs
                        .entry(index)
                        .or_default()
                        .push_str(text("partial_json")),
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                let Some(index) = index else {
                    return false;
                };
                if let Some(json) = self.inputs.remove(&index) {
                    let input = if json.trim().is_empty() {
                        json!({})
                    } else {
                        match serde_json::from_str::<Value>(&json) {
                            Ok(input) => input,
                            Err(_) => return false,
                        }
                    };
                    if let Some(block) = self.blocks.get_mut(&index).and_then(Value::as_object_mut)
                    {
                        block.insert("input".to_owned(), input);
                    }
                }
            }
            Some("message_delta") => {
                let Some(message) = self.start.as_mut() else {
                    return false;
                };
                if let Some(delta) = event.get("delta").and_then(Value::as_object) {
                    for (key, value) in delta {
                        message.insert(key.clone(), value.clone());
                    }
                }
                if let Some(usage) = event.get("usage").and_then(Value::as_object) {
                    let merged = message
                        .entry("usage")
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Some(merged) = merged.as_object_mut() {
                        for (key, value) in usage {
                            merged.insert(key.clone(), value.clone());
                        }
                    }
                }
            }
            Some("message_stop") => self.stopped = true,
            _ => {}
        }
        true
    }

    fn finish(self) -> Option<Value> {
        if !self.stopped || !self.inputs.is_empty() {
            return None;
        }
        let mut message = self.start?;
        message.insert(
            "content".to_owned(),
            Value::Array(self.blocks.into_values().collect()),
        );
        Some(Value::Object(message))
    }
}
