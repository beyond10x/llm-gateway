//! Reading a relayed request body as JSON without deserialising it.
//!
//! The relay changes at most three string values of a body (rows W2 and W4), may add one
//! `output_config.effort` member (a model's declared effort), may lower its top-level
//! `max_tokens` (the output clamp), and must pass every other byte through unchanged. So the
//! body is scanned, not parsed into a tree: the scan validates the whole document against RFC
//! 8259, records where the values it may rewrite lie, whether the body names an effort and where
//! one would be added, where a top-level integer `max_tokens` lies, and whether a top-level
//! `tools` array is non-empty (the tool-calling refusal). The rewrite then replaces exactly those
//! byte ranges and inserts at most that one member. The same scan reads the `input_tokens` of a
//! target's count answer ([`input_tokens`]).
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
    /// The top-level `max_tokens` when the body names it once and it is one non-negative JSON
    /// integer: where its token lies, and its value (`u64::MAX` past `u64`).
    max_tokens: Option<(Range<usize>, u64)>,
}

/// A body as the relay rewrote it, with where its top-level integer `max_tokens` lies in it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Rewritten {
    pub(crate) bytes: Vec<u8>,
    max_tokens: Option<(Range<usize>, u64)>,
}

impl Rewritten {
    /// The top-level `max_tokens`, when the body names it once as one non-negative integer.
    pub(crate) fn max_tokens(&self) -> Option<u64> {
        self.max_tokens.as_ref().map(|(_, value)| *value)
    }

    /// Lowers that `max_tokens` to `room` in place when it asks for more. Nothing else changes,
    /// and a body without one is unchanged.
    pub(crate) fn clamp_max_tokens(&mut self, room: u64) {
        let Some((span, value)) = self.max_tokens.clone() else {
            return;
        };
        if value <= room {
            return;
        }
        let digits = room.to_string().into_bytes();
        let start = span.start;
        let end = start + digits.len();
        self.bytes.splice(span, digits);
        self.max_tokens = Some((start..end, room));
    }
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
    #[cfg(test)]
    pub(crate) fn rewrite(
        &self,
        body: &[u8],
        upstream_model: &str,
        map_effort: bool,
        declared: Option<ReasoningEffort>,
    ) -> Vec<u8> {
        self.rewritten(body, upstream_model, map_effort, declared)
            .bytes
    }

    /// As [`Self::rewrite`], also keeping where the top-level integer `max_tokens` lies in the
    /// output, so the output clamp can lower it in place.
    pub(crate) fn rewritten(
        &self,
        body: &[u8],
        upstream_model: &str,
        map_effort: bool,
        declared: Option<ReasoningEffort>,
    ) -> Rewritten {
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
        // No replacement overlaps the `max_tokens` token (it is a number, every replaced value
        // a string, and an insertion lands after a value or at a brace), so in the output it
        // moves by what the replacements before it added or removed.
        let max_tokens = self.max_tokens.clone().map(|(span, value)| {
            let mut start = span.start;
            for (replaced, with) in &replacements {
                if replaced.end <= span.start {
                    start = start + with.len() - replaced.len();
                }
            }
            (start..start + span.len(), value)
        });
        let mut out = Vec::with_capacity(body.len() + model.len());
        let mut from = 0;
        for (span, value) in replacements {
            out.extend_from_slice(&body[from..span.start]);
            out.extend_from_slice(value);
            from = span.end;
        }
        out.extend_from_slice(&body[from..]);
        Rewritten {
            bytes: out,
            max_tokens,
        }
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
    /// The top-level `max_tokens` of a request.
    MaxTokens,
    /// The top-level `input_tokens` of a count answer.
    InputTokens,
    Other,
}

impl Slot {
    fn member(role: Role, key: &str) -> Self {
        match (role, key) {
            (Role::Top, "model") => Self::Model,
            (Role::Top, "tools") => Self::Tools,
            (Role::Top, "max_tokens") => Self::MaxTokens,
            (Role::Top, "input_tokens") => Self::InputTokens,
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
            Self::Model
            | Self::Effort
            | Self::Tools
            | Self::MaxTokens
            | Self::InputTokens
            | Self::Other => Role::Other,
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

/// What the scan knows of the top-level `output_config` members.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum OutputConfig {
    #[default]
    Absent,
    /// Where an effort would be written into the last one, an object.
    Object(EffortSlot),
    /// One of them was not an object, so no effort is written.
    NotObject,
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
    output_config: OutputConfig,
    /// Where the top-level object's last member's value ends.
    top_end: Option<usize>,
    max_tokens: Integer,
    input_tokens: Integer,
}

/// What the scan has seen of one top-level member read as a count of tokens.
#[derive(Default)]
struct Integer {
    /// How often the top level names it.
    named: usize,
    /// The last one's token and value, when it is one non-negative JSON integer.
    value: Option<(Range<usize>, u64)>,
}

impl Integer {
    /// Notes one more member whose value token is `raw`, at `span`. Digits alone are a
    /// non-negative integer in JSON (`scalar` has refused a leading zero); one past `u64` reads
    /// as `u64::MAX`, which asks for more than any window.
    fn named(&mut self, span: Range<usize>, raw: &[u8]) {
        self.named += 1;
        self.value = (!raw.is_empty() && raw.iter().all(u8::is_ascii_digit)).then(|| {
            let value = std::str::from_utf8(raw)
                .ok()
                .and_then(|digits| digits.parse().ok())
                .unwrap_or(u64::MAX);
            (span, value)
        });
    }

    /// The member, when it is named once and is one non-negative integer.
    fn once(self) -> Option<(Range<usize>, u64)> {
        if self.named == 1 { self.value } else { None }
    }
}

impl Found {
    fn value(&mut self, slot: Slot, span: Range<usize>, raw: &[u8], text: Option<String>) {
        if slot == Slot::Effort {
            self.names_effort = true;
        }
        match slot {
            Slot::MaxTokens => self.max_tokens.named(span, raw),
            Slot::InputTokens => self.input_tokens.named(span, raw),
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

    /// Notes that the value about to be scanned for `slot` starts with `byte`.
    fn opens(&mut self, slot: Slot, byte: Option<u8>) {
        if slot == Slot::OutputConfig && byte != Some(b'{') {
            self.output_config = OutputConfig::NotObject;
        }
    }

    /// Notes that the frame on top of `stack` closes. For an object, `at` is where a member
    /// would be added, after its last member when `after_member`.
    fn closes(&mut self, stack: &[Frame], at: usize, after_member: bool) {
        match stack.last() {
            Some(Frame::Object(Role::OutputConfig)) => {
                if self.output_config != OutputConfig::NotObject {
                    self.output_config =
                        OutputConfig::Object(EffortSlot::Object { at, after_member });
                }
            }
            Some(Frame::Object(_)) if stack.len() == 1 => self.top_end = Some(at),
            _ => {}
        }
    }

    /// Where a declared effort is written, or `None` when the body names one or its top-level
    /// `output_config` is not an object.
    fn effort_slot(&self) -> Option<EffortSlot> {
        if self.names_effort {
            return None;
        }
        match self.output_config {
            OutputConfig::Object(slot) => Some(slot),
            OutputConfig::NotObject => None,
            OutputConfig::Absent => self.top_end.map(|at| EffortSlot::Top { at }),
        }
    }
}

const NOT_JSON: RefusalCode = RefusalCode::BodyNotJson;

/// Scans the value starting at `position` for `slot`; returns where the scan continues and
/// what it expects there.
fn value_token(
    text: &str,
    position: usize,
    slot: Slot,
    stack: &mut Vec<Frame>,
    found: &mut Found,
) -> Result<(usize, Next), RefusalCode> {
    let bytes = text.as_bytes();
    let byte = bytes.get(position).copied();
    found.opens(slot, byte);
    match byte {
        Some(b'{') => {
            found.value(slot, position..position + 1, b"{", None);
            stack.push(Frame::Object(slot.role()));
            Ok((position + 1, Next::FirstMember))
        }
        Some(b'[') => {
            found.value(slot, position..position + 1, b"[", None);
            found.tools_opened = slot == Slot::Tools;
            stack.push(Frame::Array);
            Ok((position + 1, Next::FirstElement))
        }
        Some(b'"') => {
            let (end, value) = string(text, position)?;
            found.value(slot, position..end, &bytes[position..end], Some(value));
            Ok((end, Next::AfterValue))
        }
        Some(_) => {
            let end = scalar(bytes, position)?;
            found.value(slot, position..end, &bytes[position..end], None);
            Ok((end, Next::AfterValue))
        }
        None => Err(NOT_JSON),
    }
}

/// Scans `body` and reports the relay's view of it.
///
/// # Errors
/// [`RefusalCode::BodyNotJson`] for anything but one well-formed UTF-8 JSON object, or one that
/// names `model` twice at its top level; [`RefusalCode::ModelAbsent`] for an object whose
/// top-level `model` is missing or not a string.
pub(crate) fn analyse(body: &[u8]) -> Result<Analysis, RefusalCode> {
    let found = scan(body)?;
    if found.models > 1 {
        return Err(NOT_JSON);
    }
    let effort_slot = found.effort_slot();
    let (model_span, model) = found.model.ok_or(RefusalCode::ModelAbsent)?;
    Ok(Analysis {
        model,
        model_span,
        efforts: found.efforts,
        offers_tools: found.offers_tools,
        effort_slot,
        max_tokens: found.max_tokens.once(),
    })
}

/// The count a target answered `POST /v1/messages/count_tokens` with: the top-level
/// `input_tokens` of a body that is one JSON object naming it once, as one non-negative integer.
/// `None` for any other body.
pub(crate) fn input_tokens(body: &[u8]) -> Option<u64> {
    scan(body)
        .ok()
        .and_then(|found| found.input_tokens.once())
        .map(|(_, count)| count)
}

/// Scans `body`, one well-formed UTF-8 JSON object, and records what the relay looks at.
fn scan(body: &[u8]) -> Result<Found, RefusalCode> {
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
            Next::Value(slot) => {
                let (end, after) = value_token(text, position, slot, &mut stack, &mut found)?;
                position = end;
                after
            }
            Next::FirstMember if byte == Some(b'}') => {
                found.closes(&stack, position, false);
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
                (Some(Frame::Object(_)), Some(b'}')) | (Some(Frame::Array), Some(b']')) => {
                    found.closes(&stack, value_end, true);
                    stack.pop();
                    position += 1;
                    Next::AfterValue
                }
                _ => return Err(NOT_JSON),
            },
        };
    }
    Ok(found)
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
    use super::{analyse, input_tokens};
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

    /// The output clamp lowers the top-level integer `max_tokens` in place, wherever the rewrite
    /// before it moved it, and only when it asks for more than the room.
    #[test]
    fn a_top_level_integer_max_tokens_is_lowered_in_place_after_the_rewrite() {
        let clamped = |body: &str, room: u64| {
            let analysis = analyse(body.as_bytes()).unwrap();
            let mut rewritten = analysis.rewritten(
                body.as_bytes(),
                "a-longer-upstream-name",
                true,
                Some(ReasoningEffort::Low),
            );
            rewritten.clamp_max_tokens(room);
            (
                rewritten.max_tokens(),
                String::from_utf8(rewritten.bytes).unwrap(),
            )
        };
        assert_eq!(
            clamped(r#"{"model":"m","max_tokens":32000}"#, 31_999),
            (
                Some(31_999),
                r#"{"model":"a-longer-upstream-name","max_tokens":31999,"output_config":{"effort":"low"}}"#
                    .to_owned()
            )
        );
        assert_eq!(
            clamped(
                r#"{"output_config":{"effort":"max"},"max_tokens" : 900 ,"model":"m"}"#,
                9
            ),
            (
                Some(9),
                r#"{"output_config":{"effort":"xhigh"},"max_tokens" : 9 ,"model":"a-longer-upstream-name"}"#
                    .to_owned()
            )
        );
        assert_eq!(
            clamped(r#"{"max_tokens":0,"model":"m"}"#, 1).1,
            r#"{"max_tokens":0,"model":"a-longer-upstream-name","output_config":{"effort":"low"}}"#
        );
        for body in [
            r#"{"model":"m"}"#,
            r#"{"model":"m","max_tokens":-1}"#,
            r#"{"model":"m","max_tokens":1.0}"#,
            r#"{"model":"m","max_tokens":1e3}"#,
            r#"{"model":"m","max_tokens":"9"}"#,
            r#"{"model":"m","max_tokens":[9]}"#,
            r#"{"model":"m","max_tokens":9,"max_tokens":9}"#,
            r#"{"model":"m","x":{"max_tokens":9}}"#,
        ] {
            assert_eq!(clamped(body, 1).0, None, "{body}");
        }
        assert_eq!(
            clamped(r#"{"model":"m","max_tokens":99999999999999999999999}"#, 5).1,
            r#"{"model":"a-longer-upstream-name","max_tokens":5,"output_config":{"effort":"low"}}"#
        );
    }

    #[test]
    fn a_count_is_one_top_level_non_negative_integer_input_tokens() {
        assert_eq!(input_tokens(br#"{"input_tokens":33537}"#), Some(33_537));
        assert_eq!(input_tokens(br#" {"x":[1],"input_tokens":0} "#), Some(0));
        for body in [
            r#"{"tokens":10}"#,
            r#"{"input_tokens":-1}"#,
            r#"{"input_tokens":10.0}"#,
            r#"{"input_tokens":1e1}"#,
            r#"{"input_tokens":"10"}"#,
            r#"{"input_tokens":null}"#,
            r#"{"input_tokens":10,"input_tokens":10}"#,
            r#"{"usage":{"input_tokens":10}}"#,
            r#"[{"input_tokens":10}]"#,
            "input_tokens: 10",
            r#"{"input_tokens":10"#,
        ] {
            assert_eq!(input_tokens(body.as_bytes()), None, "{body}");
        }
    }
}
