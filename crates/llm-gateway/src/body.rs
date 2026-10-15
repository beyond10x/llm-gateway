//! Reading a relayed request body as JSON without deserialising it.
//!
//! The relay changes at most three string values of a body (rows W2 and W4), may add one
//! `output_config.effort` member (a model's declared effort), and must pass every other byte
//! through unchanged. So the body is scanned, not parsed into a tree: the scan validates the
//! whole document against RFC 8259, records where the values it may rewrite lie, whether the body
//! names an effort and where one would be added, and whether a top-level `tools` array is
//! non-empty (the tool-calling refusal). The rewrite then replaces exactly those byte ranges and
//! inserts at most that one member.
//!
//! The scan keeps its nesting on an explicit stack rather than the call stack, so a body that is
//! nothing but `[[[[…` costs memory bounded by the body bound and cannot overflow a thread's
//! stack.

use crate::{error::RefusalCode, json::Writer, relay::ReasoningEffort};
use std::ops::Range;

/// The client efforts the served chat template rejects: `high` (llmgw `src/lib.rs:37`) and
/// `max`, which Claude Code also sends; the Qwen3.8 template accepts only `xhigh`, `medium` and
/// `low` (`docs/gateway.md` "The relay", step 6).
const CLIENT_EFFORTS: [&str; 2] = ["high", "max"];
/// The template's nearest accepted depth, written as a JSON string (llmgw `src/lib.rs:39`).
const SERVED_EFFORT: &str = "\"xhigh\"";

/// What the relay needs to know about a body that is one JSON object.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Analysis {
    /// The top-level `model` value, unescaped.
    pub(crate) model: String,
    /// Where the `model` value's string token lies, quotes included.
    model_span: Range<usize>,
    /// Every `output_config.effort` and `chat_template_kwargs.reasoning_effort` string token
    /// whose unescaped value is exactly `high` or `max`.
    efforts: Vec<Range<usize>>,
    /// Whether a top-level `tools` member is a non-empty array: the body offers the model a
    /// tool. With `tools` named twice at the top level, either one counts.
    pub(crate) offers_tools: bool,
    /// Where a declared effort would be written, or `None` when the body names an effort
    /// already or its top-level `output_config` is not an object.
    effort_slot: Option<EffortSlot>,
}

/// Where a declared effort is written into a body that names none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffortSlot {
    /// Into the last top-level `output_config` object, as its last member: at the byte offset
    /// of its closing brace, or just after its last member's value, with a `,` before.
    Object { at: usize, after_member: bool },
    /// No top-level `output_config`: a new last member of the top-level object, after its last
    /// member's value.
    Top { at: usize },
}

impl Analysis {
    /// The body with `model` set to `upstream_model`; when `map_effort`, every recorded effort
    /// set to `xhigh`; and when `declared` is given and the body names no effort, its
    /// `output_config.effort` set to `declared` by inserting that one member. Nothing else
    /// changes.
    pub(crate) fn rewrite(
        &self,
        body: &[u8],
        upstream_model: &str,
        map_effort: bool,
        declared: Option<ReasoningEffort>,
    ) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.string(upstream_model);
        let model = writer.finish();
        let mut replacements: Vec<(Range<usize>, &[u8])> =
            vec![(self.model_span.clone(), model.as_bytes())];
        if map_effort {
            for span in &self.efforts {
                replacements.push((span.clone(), SERVED_EFFORT.as_bytes()));
            }
        }
        let inserted = declared
            .zip(self.effort_slot)
            .map(|(effort, slot)| match slot {
                EffortSlot::Object { at, after_member } => {
                    let comma = if after_member { "," } else { "" };
                    (at, format!("{comma}\"effort\":\"{}\"", effort.as_str()))
                }
                EffortSlot::Top { at } => (
                    at,
                    format!(",\"output_config\":{{\"effort\":\"{}\"}}", effort.as_str()),
                ),
            });
        if let Some((at, member)) = &inserted {
            replacements.push((*at..*at, member.as_bytes()));
        }
        replacements.sort_by_key(|(span, _)| span.start);
        let mut out = Vec::with_capacity(body.len() + model.len());
        let mut from = 0;
        for (span, value) in replacements {
            out.extend_from_slice(&body[from..span.start]);
            out.extend_from_slice(value);
            from = span.end;
        }
        out.extend_from_slice(&body[from..]);
        out
    }
}

/// Which object a member belongs to, as far as the relay cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Top,
    OutputConfig,
    TemplateKwargs,
    Other,
}

/// Where the value about to be scanned sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Root,
    Model,
    OutputConfig,
    TemplateKwargs,
    Effort,
    Tools,
    Other,
}

impl Slot {
    fn member(role: Role, key: &str) -> Self {
        match (role, key) {
            (Role::Top, "model") => Self::Model,
            (Role::Top, "tools") => Self::Tools,
            (Role::Top, "output_config") => Self::OutputConfig,
            (Role::Top, "chat_template_kwargs") => Self::TemplateKwargs,
            (Role::OutputConfig, "effort") | (Role::TemplateKwargs, "reasoning_effort") => {
                Self::Effort
            }
            _ => Self::Other,
        }
    }

    fn role(self) -> Role {
        match self {
            Self::Root => Role::Top,
            Self::OutputConfig => Role::OutputConfig,
            Self::TemplateKwargs => Role::TemplateKwargs,
            Self::Model | Self::Effort | Self::Tools | Self::Other => Role::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frame {
    Object(Role),
    Array,
}

/// What the scan expects next.
#[derive(Debug, Clone, Copy)]
enum Next {
    Value(Slot),
    FirstMember,
    Member,
    FirstElement,
    AfterValue,
}

/// What the scan has seen of the top-level `model` member.
#[derive(Default)]
struct Found {
    models: usize,
    model: Option<(Range<usize>, String)>,
    efforts: Vec<Range<usize>>,
    /// The array just opened is a top-level `tools`; its first element decides `offers_tools`.
    tools_opened: bool,
    offers_tools: bool,
    /// A top-level `output_config.effort` or `chat_template_kwargs.reasoning_effort` member was
    /// seen, whatever its value.
    names_effort: bool,
    /// A top-level `output_config` value was not an object.
    output_config_not_object: bool,
    /// Where the last top-level `output_config` object's closing brace, or its last member's
    /// value, ends.
    output_config: Option<EffortSlot>,
    /// Where the top-level object's last member's value ends.
    top_end: Option<usize>,
}

impl Found {
    fn value(&mut self, slot: Slot, span: Range<usize>, text: Option<String>) {
        if slot == Slot::Effort {
            self.names_effort = true;
        }
        match slot {
            Slot::Model => {
                self.models += 1;
                self.model = text.map(|text| (span, text));
            }
            Slot::Effort
                if text
                    .as_deref()
                    .is_some_and(|effort| CLIENT_EFFORTS.contains(&effort)) =>
            {
                self.efforts.push(span);
            }
            _ => {}
        }
    }
}

const NOT_JSON: RefusalCode = RefusalCode::BodyNotJson;

/// Scans `body` and reports the relay's view of it.
///
/// # Errors
/// [`RefusalCode::BodyNotJson`] for anything but one well-formed UTF-8 JSON object, or one that
/// names `model` twice at its top level; [`RefusalCode::ModelAbsent`] for an object whose
/// top-level `model` is missing or not a string.
pub(crate) fn analyse(body: &[u8]) -> Result<Analysis, RefusalCode> {
    let text = std::str::from_utf8(body).map_err(|_| NOT_JSON)?;
    let bytes = text.as_bytes();
    let mut position = skip_whitespace(bytes, 0);
    if bytes.get(position) != Some(&b'{') {
        return Err(NOT_JSON);
    }
    let mut stack: Vec<Frame> = Vec::new();
    let mut found = Found::default();
    let mut next = Next::Value(Slot::Root);
    loop {
        // Where the last value ended, before the whitespace that follows it.
        let value_end = position;
        position = skip_whitespace(bytes, position);
        let byte = bytes.get(position).copied();
        next = match next {
            Next::Value(slot) => match byte {
                Some(b'{') => {
                    found.value(slot, position..position + 1, None);
                    stack.push(Frame::Object(slot.role()));
                    position += 1;
                    Next::FirstMember
                }
                Some(b'[') => {
                    found.output_config_not_object |= slot == Slot::OutputConfig;
                    found.value(slot, position..position + 1, None);
                    found.tools_opened = slot == Slot::Tools;
                    stack.push(Frame::Array);
                    position += 1;
                    Next::FirstElement
                }
                Some(b'"') => {
                    found.output_config_not_object |= slot == Slot::OutputConfig;
                    let (end, value) = string(text, position)?;
                    found.value(slot, position..end, Some(value));
                    position = end;
                    Next::AfterValue
                }
                Some(_) => {
                    found.output_config_not_object |= slot == Slot::OutputConfig;
                    let end = scalar(bytes, position)?;
                    found.value(slot, position..end, None);
                    position = end;
                    Next::AfterValue
                }
                None => return Err(NOT_JSON),
            },
            Next::FirstMember if byte == Some(b'}') => {
                if stack.last() == Some(&Frame::Object(Role::OutputConfig)) {
                    found.output_config = Some(EffortSlot::Object {
                        at: position,
                        after_member: false,
                    });
                }
                stack.pop();
                position += 1;
                Next::AfterValue
            }
            Next::FirstMember | Next::Member => {
                let Some(Frame::Object(role)) = stack.last().copied() else {
                    return Err(NOT_JSON);
                };
                if byte != Some(b'"') {
                    return Err(NOT_JSON);
                }
                let (end, key) = string(text, position)?;
                position = skip_whitespace(bytes, end);
                if bytes.get(position) != Some(&b':') {
                    return Err(NOT_JSON);
                }
                position += 1;
                Next::Value(Slot::member(role, &key))
            }
            Next::FirstElement if byte == Some(b']') => {
                found.tools_opened = false;
                stack.pop();
                position += 1;
                Next::AfterValue
            }
            Next::FirstElement => {
                found.offers_tools |= std::mem::take(&mut found.tools_opened);
                Next::Value(Slot::Other)
            }
            Next::AfterValue => match (stack.last(), byte) {
                (None, None) => break,
                (Some(Frame::Object(_)), Some(b',')) => {
                    position += 1;
                    Next::Member
                }
                (Some(Frame::Array), Some(b',')) => {
                    position += 1;
                    Next::Value(Slot::Other)
                }
                (Some(Frame::Object(role)), Some(b'}')) => {
                    if *role == Role::OutputConfig {
                        found.output_config = Some(EffortSlot::Object {
                            at: value_end,
                            after_member: true,
                        });
                    }
                    if stack.len() == 1 {
                        found.top_end = Some(value_end);
                    }
                    stack.pop();
                    position += 1;
                    Next::AfterValue
                }
                (Some(Frame::Array), Some(b']')) => {
                    stack.pop();
                    position += 1;
                    Next::AfterValue
                }
                _ => return Err(NOT_JSON),
            },
        };
    }
    if found.models > 1 {
        return Err(NOT_JSON);
    }
    let (model_span, model) = found.model.ok_or(RefusalCode::ModelAbsent)?;
    let effort_slot = if found.names_effort || found.output_config_not_object {
        None
    } else {
        found
            .output_config
            .or_else(|| found.top_end.map(|at| EffortSlot::Top { at }))
    };
    Ok(Analysis {
        model,
        model_span,
        efforts: found.efforts,
        offers_tools: found.offers_tools,
        effort_slot,
    })
}

fn skip_whitespace(bytes: &[u8], mut position: usize) -> usize {
    while matches!(bytes.get(position), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        position += 1;
    }
    position
}

/// Scans the string token starting at the `"` at `start`; returns the position after its
/// closing quote and its unescaped value.
fn string(text: &str, start: usize) -> Result<(usize, String), RefusalCode> {
    let bytes = text.as_bytes();
    let mut value = String::new();
    let mut position = start + 1;
    let mut run = position;
    loop {
        match bytes.get(position).copied() {
            None => return Err(NOT_JSON),
            Some(b'"') => {
                value.push_str(&text[run..position]);
                return Ok((position + 1, value));
            }
            Some(b'\\') => {
                value.push_str(&text[run..position]);
                position = escape(bytes, position + 1, &mut value)?;
                run = position;
            }
            Some(byte) if byte < 0x20 => return Err(NOT_JSON),
            Some(_) => position += 1,
        }
    }
}

/// Decodes the escape whose letter is at `position`, appends it, and returns where the string
/// continues.
fn escape(bytes: &[u8], position: usize, value: &mut String) -> Result<usize, RefusalCode> {
    let simple = match bytes.get(position).copied() {
        Some(b'"') => '"',
        Some(b'\\') => '\\',
        Some(b'/') => '/',
        Some(b'b') => '\u{8}',
        Some(b'f') => '\u{c}',
        Some(b'n') => '\n',
        Some(b'r') => '\r',
        Some(b't') => '\t',
        Some(b'u') => {
            let first = hex4(bytes, position + 1)?;
            let (code, end) = if (0xD800..0xDC00).contains(&first) {
                // A leading surrogate must be followed by an escaped trailing one.
                if bytes.get(position + 5..position + 7) != Some(b"\\u".as_slice()) {
                    return Err(NOT_JSON);
                }
                let second = hex4(bytes, position + 7)?;
                if !(0xDC00..0xE000).contains(&second) {
                    return Err(NOT_JSON);
                }
                (
                    0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00),
                    position + 11,
                )
            } else {
                (first, position + 5)
            };
            // A lone trailing surrogate is not a scalar value, so `from_u32` refuses it.
            value.push(char::from_u32(code).ok_or(NOT_JSON)?);
            return Ok(end);
        }
        _ => return Err(NOT_JSON),
    };
    value.push(simple);
    Ok(position + 1)
}

fn hex4(bytes: &[u8], position: usize) -> Result<u32, RefusalCode> {
    let digits = bytes.get(position..position + 4).ok_or(NOT_JSON)?;
    let mut code = 0;
    for digit in digits {
        let nibble = char::from(*digit).to_digit(16).ok_or(NOT_JSON)?;
        code = code * 16 + nibble;
    }
    Ok(code)
}

/// Scans a number or a literal starting at `start` and returns where it ends.
fn scalar(bytes: &[u8], start: usize) -> Result<usize, RefusalCode> {
    for literal in [b"true".as_slice(), b"false", b"null"] {
        if bytes[start..].starts_with(literal) {
            return Ok(start + literal.len());
        }
    }
    let digits = |mut position: usize| {
        let from = position;
        while bytes.get(position).is_some_and(u8::is_ascii_digit) {
            position += 1;
        }
        (position, position > from)
    };
    let mut position = start;
    if bytes.get(position) == Some(&b'-') {
        position += 1;
    }
    match bytes.get(position) {
        Some(b'0') => position += 1,
        Some(b'1'..=b'9') => position = digits(position).0,
        _ => return Err(NOT_JSON),
    }
    if bytes.get(position) == Some(&b'.') {
        let (end, any) = digits(position + 1);
        if !any {
            return Err(NOT_JSON);
        }
        position = end;
    }
    if matches!(bytes.get(position), Some(b'e' | b'E')) {
        position += 1;
        if matches!(bytes.get(position), Some(b'+' | b'-')) {
            position += 1;
        }
        let (end, any) = digits(position);
        if !any {
            return Err(NOT_JSON);
        }
        position = end;
    }
    Ok(position)
}

#[cfg(test)]
mod tests {
    use super::analyse;
    use crate::{error::RefusalCode, relay::ReasoningEffort};

    fn model(body: &str) -> Result<String, RefusalCode> {
        analyse(body.as_bytes()).map(|analysis| analysis.model)
    }

    #[test]
    fn well_formed_documents_are_read_and_malformed_ones_are_not() {
        for body in [
            r#"{"model":"m"}"#,
            r#" {"a":[1,-0.5e+3,true,false,null,{"b":[]}],"model":"m","c":{}} "#,
            r#"{"model":"\u006d","s":"\ud83d\ude00 \" \\ \/ \b\f\n\r\t"}"#,
            "{\"model\":\"m\",\"x\":0E1}",
        ] {
            assert_eq!(model(body), Ok("m".to_string()), "{body}");
        }
        for body in [
            "{",
            "{}x",
            "[]",
            "{\"model\":\"m\",}",
            "{\"model\":\"m\" \"x\":1}",
            "{\"model\":01}",
            "{\"model\":\"m\",\"x\":1.}",
            "{\"model\":\"m\",\"x\":-}",
            "{\"model\":\"m\",\"x\":1e}",
            "{\"model\":\"m\",\"x\":tru}",
            "{\"model\":\"m\",\"x\":\"\\ud83d\"}",
            "{\"model\":\"m\",\"x\":\"\\ude00\"}",
            "{\"model\":\"m\",\"x\":\"\\q\"}",
            "{\"model\":\"m\",\"x\":\"\t\"}",
            "{\"model\":\"m\",\"x\":[1,]}",
            "{\"model\":\"m\",\"x\":[1 2]}",
            "{\"model\":\"m\"]",
            "{\"model\":\"m\",\"model\":\"m\"}",
        ] {
            assert_eq!(model(body), Err(RefusalCode::BodyNotJson), "{body}");
        }
    }

    #[test]
    fn deep_nesting_is_scanned_without_recursion() {
        let depth = 1_000_000;
        let body = format!(
            "{{\"model\":\"m\",\"x\":{}{}}}",
            "[".repeat(depth),
            "]".repeat(depth)
        );
        assert_eq!(model(&body), Ok("m".to_string()));
        let open = format!("{{\"model\":\"m\",\"x\":{}}}", "[".repeat(depth));
        assert_eq!(model(&open), Err(RefusalCode::BodyNotJson));
    }

    #[test]
    fn only_the_recorded_values_are_rewritten() {
        let body = br#"{"output_config":{"effort":"hig\u0068"},"model":"m","x":{"effort":"high"}}"#;
        let analysis = analyse(body).unwrap();
        assert_eq!(
            String::from_utf8(analysis.rewrite(body, "up\"s", true, None)).unwrap(),
            r#"{"output_config":{"effort":"xhigh"},"model":"up\"s","x":{"effort":"high"}}"#
        );
        assert_eq!(
            String::from_utf8(analysis.rewrite(body, "up", false, None)).unwrap(),
            r#"{"output_config":{"effort":"hig\u0068"},"model":"up","x":{"effort":"high"}}"#
        );
    }

    #[test]
    fn an_effort_of_max_is_recorded_as_high_is_and_no_other() {
        let body = br#"{"model":"m","output_config":{"effort":"max"},"chat_template_kwargs":{"reasoning_effort":"max"}}"#;
        let analysis = analyse(body).unwrap();
        assert_eq!(
            String::from_utf8(analysis.rewrite(body, "up", true, None)).unwrap(),
            r#"{"model":"up","output_config":{"effort":"xhigh"},"chat_template_kwargs":{"reasoning_effort":"xhigh"}}"#
        );
        for effort in ["low", "medium", "xhigh", "MAX", "maximum", " max", "none"] {
            let body = format!(r#"{{"model":"m","output_config":{{"effort":"{effort}"}}}}"#);
            let analysis = analyse(body.as_bytes()).unwrap();
            assert_eq!(
                String::from_utf8(analysis.rewrite(body.as_bytes(), "m", true, None)).unwrap(),
                body,
                "{effort}"
            );
        }
    }

    #[test]
    fn a_declared_effort_is_inserted_only_into_a_body_naming_none() {
        let declared = |body: &str| {
            let analysis = analyse(body.as_bytes()).unwrap();
            String::from_utf8(analysis.rewrite(
                body.as_bytes(),
                "up",
                true,
                Some(ReasoningEffort::Medium),
            ))
            .unwrap()
        };
        for (body, expected) in [
            (
                r#"{"model":"m"}"#,
                r#"{"model":"up","output_config":{"effort":"medium"}}"#,
            ),
            (
                "{\"model\":\"m\" ,\n \"x\" : [1] \n}",
                "{\"model\":\"up\" ,\n \"x\" : [1],\"output_config\":{\"effort\":\"medium\"} \n}",
            ),
            (
                r#"{"output_config":{ },"model":"m"}"#,
                r#"{"output_config":{ "effort":"medium"},"model":"up"}"#,
            ),
            (
                r#"{"output_config":{"format":{"type":"text"}} ,"model":"m"}"#,
                r#"{"output_config":{"format":{"type":"text"},"effort":"medium"} ,"model":"up"}"#,
            ),
            (
                r#"{"model":"m","chat_template_kwargs":{"other":1}}"#,
                r#"{"model":"up","chat_template_kwargs":{"other":1},"output_config":{"effort":"medium"}}"#,
            ),
            (
                r#"{"model":"m","output_config":{"effort":"high"}}"#,
                r#"{"model":"up","output_config":{"effort":"xhigh"}}"#,
            ),
            (
                r#"{"model":"m","chat_template_kwargs":{"reasoning_effort":7}}"#,
                r#"{"model":"up","chat_template_kwargs":{"reasoning_effort":7}}"#,
            ),
            (
                r#"{"model":"m","output_config":null}"#,
                r#"{"model":"up","output_config":null}"#,
            ),
            (
                r#"{"model":"m","output_config":[{"effort":"low"}]}"#,
                r#"{"model":"up","output_config":[{"effort":"low"}]}"#,
            ),
        ] {
            assert_eq!(declared(body), expected, "{body}");
        }
    }
}
