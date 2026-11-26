//! Token counts read from a relayed answer as it streams (story:live-dashboard,
//! story:token-and-cache-metrics).
//!
//! The scanner reads the answer's decoded body bytes, a JSON body or server-sent events, as a
//! stream of JSON objects, and takes the counters from the answer's usage object only:
//!
//! - messages: `usage` of the message (`input_tokens`, `output_tokens`,
//!   `cache_read_input_tokens`, `cache_creation_input_tokens`), on a stream in `message_start`'s
//!   `message.usage` and again, cumulative, in `message_delta`'s `usage`;
//! - chat: the `usage` of the answer or of the stream's last chunk (`prompt_tokens`,
//!   `completion_tokens`, and `cached_tokens` and `cache_creation_input_tokens` in its
//!   `prompt_tokens_details`);
//! - responses: `usage` of the response, which a stream carries as `response.usage`
//!   (`input_tokens`, `output_tokens`, and `cached_tokens` and `cache_creation_input_tokens` in
//!   its `input_tokens_details`).
//!
//! So a usage object is the member `usage` of a top-level object, or of that object's `message`
//! or `response`, and nothing else: a member of the same name anywhere else in the body (a tool
//! call's arguments, a text block, a metadata object, an array's element) is not read. Bytes
//! outside a top-level object (an event's name, `data: `, `[DONE]`) are passed over. A counter
//! is a whole number of at most 19 digits; a value the answer reported as zero is zero, and a
//! counter it never sent stays `None`. Each counter keeps the largest value reported.
//!
//! The scanner keeps no bytes of the answer between pieces: its state is the path to the
//! innermost object it reads (at most five levels; what lies deeper is only counted) and the
//! name or number it is reading, so a key or value split across two pieces is read whole.

/// The token counts of one answer, each `None` when the answer never reported it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// `input_tokens` or `prompt_tokens`.
    pub input_tokens: Option<u64>,
    /// `output_tokens` or `completion_tokens`.
    pub output_tokens: Option<u64>,
    /// `cache_read_input_tokens` or `cached_tokens`.
    pub cache_read_tokens: Option<u64>,
    /// `cache_creation_input_tokens`.
    pub cache_creation_tokens: Option<u64>,
}

/// The longest member name compared: longer names are none the scanner reads.
const NAME_BYTES: usize = 32;

/// The most digits a counter has.
const DIGITS: u8 = 19;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Input,
    Output,
    CacheRead,
    CacheCreation,
}

/// Where a container stands in the answer, as far as the counters care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// A top-level object: one JSON body, or one event's data.
    Root,
    /// The `message` or `response` member of a top-level object.
    Holder,
    /// A usage object.
    Usage,
    /// `prompt_tokens_details` or `input_tokens_details` of a usage object.
    Details,
    /// Anything else: nothing in it, at any depth, is read.
    Other,
}

impl Place {
    /// The place of an object that is the value of the member `name` of an object here.
    fn child(self, name: Option<&[u8]>) -> Self {
        match (self, name) {
            (Self::Root | Self::Holder, Some(b"usage")) => Self::Usage,
            (Self::Root, Some(b"message" | b"response")) => Self::Holder,
            (Self::Usage, Some(b"prompt_tokens_details" | b"input_tokens_details")) => {
                Self::Details
            }
            _ => Self::Other,
        }
    }

    /// The counter the member `name` of an object here feeds, if any.
    fn counter(self, name: &[u8]) -> Option<Field> {
        match (self, name) {
            (Self::Usage, b"input_tokens" | b"prompt_tokens") => Some(Field::Input),
            (Self::Usage, b"output_tokens" | b"completion_tokens") => Some(Field::Output),
            (Self::Usage, b"cache_read_input_tokens") | (Self::Details, b"cached_tokens") => {
                Some(Field::CacheRead)
            }
            (Self::Usage | Self::Details, b"cache_creation_input_tokens") => {
                Some(Field::CacheCreation)
            }
            _ => None,
        }
    }
}

/// A member name, kept up to [`NAME_BYTES`] bytes; one longer or with an escape in it is no name
/// the scanner compares.
#[derive(Debug, Clone, Copy)]
struct Name {
    bytes: [u8; NAME_BYTES],
    len: usize,
    unread: bool,
}

impl Name {
    const EMPTY: Self = Self {
        bytes: [0; NAME_BYTES],
        len: 0,
        unread: false,
    };

    fn push(&mut self, byte: u8) {
        if self.len < NAME_BYTES {
            self.bytes[self.len] = byte;
            self.len += 1;
        } else {
            self.unread = true;
        }
    }

    fn get(&self) -> Option<&[u8]> {
        (!self.unread).then(|| &self.bytes[..self.len])
    }
}

/// What the scanner is reading.
#[derive(Debug, Clone, Copy, Default)]
enum Token {
    /// Between tokens.
    #[default]
    Between,
    /// A member name of an object the scanner reads.
    Name { name: Name, escaped: bool },
    /// Any other string: its bytes are passed over.
    Text { escaped: bool },
    /// A number, the value of `field` when it feeds a counter; `None` once it is no count.
    Number {
        field: Option<Field>,
        value: Option<u64>,
        digits: u8,
    },
    /// `true`, `false`, `null`, or bytes that are no JSON, passed over.
    Bare,
}

/// Reads [`Usage`] from an answer fed to it piece by piece.
#[derive(Debug, Default)]
pub struct UsageScanner {
    usage: Usage,
    /// The open containers the scanner reads, outermost first: a top-level object and the
    /// objects on a usage object's path, and at most one [`Place::Other`] last.
    open: Vec<Place>,
    /// Containers open inside the last [`Place::Other`].
    deeper: usize,
    /// The member name read last in the innermost open object, until its value is read.
    member: Option<Name>,
    token: Token,
}

impl UsageScanner {
    /// Reads `piece`, the next decoded body bytes of the answer.
    pub fn feed(&mut self, piece: &[u8]) {
        for &byte in piece {
            self.byte(byte);
        }
    }

    /// What the answer reported so far.
    pub fn usage(&self) -> Usage {
        self.usage
    }

    /// The innermost open container's place, [`Place::Other`] when none is open.
    fn innermost(&self) -> Place {
        self.open.last().copied().unwrap_or(Place::Other)
    }

    /// Whether the innermost open container is one whose members are read.
    fn reading(&self) -> bool {
        self.deeper == 0 && self.open.last().is_some_and(|place| *place != Place::Other)
    }

    fn byte(&mut self, byte: u8) {
        match &mut self.token {
            Token::Between => self.between(byte),
            Token::Name { name, escaped } => {
                if byte == b'\n' {
                    self.broken();
                } else if *escaped {
                    *escaped = false;
                    name.unread = true;
                } else if byte == b'\\' {
                    *escaped = true;
                    name.unread = true;
                } else if byte == b'"' {
                    self.member = Some(*name);
                    self.token = Token::Between;
                } else {
                    name.push(byte);
                }
            }
            Token::Text { escaped } => {
                if byte == b'\n' {
                    self.broken();
                } else if *escaped {
                    *escaped = false;
                } else if byte == b'\\' {
                    *escaped = true;
                } else if byte == b'"' {
                    self.token = Token::Between;
                }
            }
            Token::Number {
                field,
                value,
                digits,
            } => {
                if byte.is_ascii_digit() {
                    *digits = digits.saturating_add(1);
                    *value = value
                        .filter(|_| *digits <= DIGITS)
                        .and_then(|value| value.checked_mul(10))
                        .and_then(|value| value.checked_add(u64::from(byte - b'0')));
                } else if matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E') {
                    *value = None;
                } else {
                    if let (Some(field), Some(value)) = (*field, *value) {
                        self.count(field, value);
                    }
                    self.token = Token::Between;
                    self.between(byte);
                }
            }
            Token::Bare => {
                if !byte.is_ascii_alphanumeric() {
                    self.token = Token::Between;
                    self.between(byte);
                }
            }
        }
    }

    fn between(&mut self, byte: u8) {
        if self.open.is_empty() {
            // Outside a top-level object only its start is read.
            if byte == b'{' {
                self.open.push(Place::Root);
                self.member = None;
            }
            return;
        }
        match byte {
            b' ' | b'\t' | b'\r' | b'\n' | b':' => {}
            b',' => self.member = None,
            b'"' => {
                self.token = if self.reading() && self.member.is_none() {
                    Token::Name {
                        name: Name::EMPTY,
                        escaped: false,
                    }
                } else {
                    self.member = None;
                    Token::Text { escaped: false }
                };
            }
            b'{' | b'[' => {
                let member = self.member.take();
                if self.reading() {
                    let place = if byte == b'{' {
                        self.innermost().child(member.as_ref().and_then(Name::get))
                    } else {
                        Place::Other
                    };
                    self.open.push(place);
                } else {
                    self.deeper += 1;
                }
            }
            b'}' | b']' => {
                self.member = None;
                if self.deeper > 0 {
                    self.deeper -= 1;
                } else {
                    self.open.pop();
                }
            }
            b'0'..=b'9' | b'-' => {
                let member = self.member.take();
                let field = match (self.reading(), member.as_ref().and_then(Name::get)) {
                    (true, Some(name)) => self.innermost().counter(name),
                    _ => None,
                };
                self.token = Token::Number {
                    field,
                    value: (byte != b'-').then_some(0),
                    digits: 0,
                };
                self.byte(byte);
            }
            _ => {
                self.member = None;
                self.token = Token::Bare;
            }
        }
    }

    /// A raw line end inside a string is no JSON: what was open is dropped, and the next
    /// top-level object is read afresh.
    fn broken(&mut self) {
        self.open.clear();
        self.deeper = 0;
        self.member = None;
        self.token = Token::Between;
    }

    fn count(&mut self, field: Field, value: u64) {
        let slot = match field {
            Field::Input => &mut self.usage.input_tokens,
            Field::Output => &mut self.usage.output_tokens,
            Field::CacheRead => &mut self.usage.cache_read_tokens,
            Field::CacheCreation => &mut self.usage.cache_creation_tokens,
        };
        *slot = Some(slot.map_or(value, |seen| seen.max(value)));
    }
}

#[cfg(test)]
mod tests {
    use super::{Usage, UsageScanner};

    fn read(pieces: &[&[u8]]) -> Usage {
        let mut scanner = UsageScanner::default();
        for piece in pieces {
            scanner.feed(piece);
        }
        scanner.usage()
    }

    const MESSAGES: &[u8] = b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":4096,\"output_tokens\":1}}}\n\nevent: content_block_delta\ndata: {\"delta\":{\"text\":\"say \\\"output_tokens\\\":999\"}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":321}}\n\n";

    #[test]
    fn a_messages_stream_fills_every_field_and_a_reported_zero_is_zero() {
        let usage = read(&[MESSAGES]);
        assert_eq!(
            usage,
            Usage {
                input_tokens: Some(12),
                output_tokens: Some(321),
                cache_read_tokens: Some(4096),
                cache_creation_tokens: Some(0),
            }
        );
    }

    #[test]
    fn every_split_of_the_stream_reads_the_same() {
        let whole = read(&[MESSAGES]);
        for cut in 1..MESSAGES.len() {
            let (left, right) = MESSAGES.split_at(cut);
            assert_eq!(read(&[left, right]), whole, "cut at {cut}");
        }
        let bytes: Vec<&[u8]> = MESSAGES.chunks(1).collect();
        assert_eq!(read(&bytes), whole);
    }

    #[test]
    fn a_chat_answer_reads_prompt_completion_and_cached_tokens() {
        let body = br#"{"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":7,"total_tokens":107,"prompt_tokens_details":{"cached_tokens":64}}}"#;
        assert_eq!(
            read(&[body]),
            Usage {
                input_tokens: Some(100),
                output_tokens: Some(7),
                cache_read_tokens: Some(64),
                cache_creation_tokens: None,
            }
        );
    }

    #[test]
    fn a_responses_stream_reads_its_completed_usage() {
        let body = b"event: response.completed\ndata: {\"response\":{\"usage\":{\"input_tokens\": 50, \"input_tokens_details\": {\"cached_tokens\": 0}, \"output_tokens\": 9}}}\n\n";
        assert_eq!(
            read(&[body]),
            Usage {
                input_tokens: Some(50),
                output_tokens: Some(9),
                cache_read_tokens: Some(0),
                cache_creation_tokens: None,
            }
        );
    }

    #[test]
    fn an_answer_without_usage_reports_nothing() {
        assert_eq!(
            read(&[br#"{"error":{"message":"input_tokens"}}"#]),
            Usage::default()
        );
    }

    /// Arguments naming every counter, with values no usage has.
    const ARGUMENTS: &str = r#"{"input_tokens":900000,"output_tokens":800000,"prompt_tokens":900000,"completion_tokens":800000,"cache_read_input_tokens":700000,"cache_creation_input_tokens":600000,"cached_tokens":700000,"usage":{"input_tokens":900000,"output_tokens":800000},"input_tokens_details":{"cached_tokens":700000}}"#;

    /// The answer's usage: 20 input and 5 output tokens, no cache counter.
    const REPORTED: Usage = Usage {
        input_tokens: Some(20),
        output_tokens: Some(5),
        cache_read_tokens: None,
        cache_creation_tokens: None,
    };

    /// Each wire's answers with a tool call whose arguments, an object, name the counters.
    fn tool_calls() -> Vec<(&'static str, String)> {
        vec![
            (
                "messages, whole",
                format!(
                    r#"{{"id":"m","type":"message","content":[{{"type":"tool_use","id":"t","name":"f","input":{ARGUMENTS}}},{{"type":"text","text":"x","citations":[{{"input_tokens":900000}}]}}],"usage":{{"input_tokens":20,"output_tokens":5}}}}"#
                ),
            ),
            (
                "messages, streamed",
                format!(
                    "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{\"id\":\"m\",\"content\":[],\"usage\":{{\"input_tokens\":20,\"output_tokens\":1}}}}}}\n\nevent: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"tool_use\",\"id\":\"t\",\"name\":\"f\",\"input\":{ARGUMENTS}}}}}\n\nevent: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"tool_use\"}},\"usage\":{{\"output_tokens\":5}}}}\n\n"
                ),
            ),
            (
                "chat, whole",
                format!(
                    r#"{{"id":"c","choices":[{{"index":0,"message":{{"tool_calls":[{{"id":"c1","function":{{"name":"f","arguments":{ARGUMENTS}}}}}]}}}}],"usage":{{"prompt_tokens":20,"completion_tokens":5}}}}"#
                ),
            ),
            (
                "chat, streamed",
                format!(
                    "data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":0,\"function\":{{\"arguments\":{ARGUMENTS}}}}}]}}}}],\"usage\":null}}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":20,\"completion_tokens\":5}}}}\n\ndata: [DONE]\n\n"
                ),
            ),
            (
                "responses, whole",
                format!(
                    r#"{{"id":"r","object":"response","error":null,"metadata":{{"input_tokens":"900000"}},"output":[{{"type":"function_call","arguments":{ARGUMENTS}}}],"usage":{{"input_tokens":20,"output_tokens":5}}}}"#
                ),
            ),
            (
                "responses, streamed",
                format!(
                    "event: response.output_item.done\ndata: {{\"type\":\"response.output_item.done\",\"item\":{{\"type\":\"function_call\",\"arguments\":{ARGUMENTS}}}}}\n\nevent: response.completed\ndata: {{\"type\":\"response.completed\",\"response\":{{\"output\":[{{\"type\":\"function_call\",\"arguments\":{ARGUMENTS}}}],\"usage\":{{\"input_tokens\":20,\"output_tokens\":5}}}}}}\n\n"
                ),
            ),
        ]
    }

    #[test]
    fn a_tool_calls_arguments_are_no_usage_on_any_wire() {
        for (answer, bytes) in tool_calls() {
            assert_eq!(read(&[bytes.as_bytes()]), REPORTED, "{answer}");
        }
    }

    #[test]
    fn every_split_of_an_answer_with_a_tool_call_reads_its_usage_only() {
        for (answer, bytes) in tool_calls() {
            let bytes = bytes.as_bytes();
            for cut in 1..bytes.len() {
                let (left, right) = bytes.split_at(cut);
                assert_eq!(read(&[left, right]), REPORTED, "{answer}, cut at {cut}");
            }
        }
    }

    #[test]
    fn the_usage_object_is_read_where_each_wire_puts_it() {
        let cases: [(&str, &[u8], Usage); 4] = [
            (
                "messages: message_start's message.usage, then message_delta's usage",
                MESSAGES,
                Usage {
                    input_tokens: Some(12),
                    output_tokens: Some(321),
                    cache_read_tokens: Some(4096),
                    cache_creation_tokens: Some(0),
                },
            ),
            (
                "chat: usage and its prompt_tokens_details",
                br#"{"usage":{"prompt_tokens":100,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":64,"cache_creation_input_tokens":8}}}"#,
                Usage {
                    input_tokens: Some(100),
                    output_tokens: Some(7),
                    cache_read_tokens: Some(64),
                    cache_creation_tokens: Some(8),
                },
            ),
            (
                "responses, streamed: response.usage and its input_tokens_details",
                br#"{"type":"response.completed","response":{"usage":{"input_tokens":50,"input_tokens_details":{"cached_tokens":6,"cache_creation_input_tokens":2},"output_tokens":9}}}"#,
                Usage {
                    input_tokens: Some(50),
                    output_tokens: Some(9),
                    cache_read_tokens: Some(6),
                    cache_creation_tokens: Some(2),
                },
            ),
            (
                "a counter outside a usage object, or a usage object nested deeper",
                br#"{"input_tokens":1,"message":{"output_tokens":2},"response":{"cached_tokens":3},"choices":[{"usage":{"prompt_tokens":4}}],"delta":{"usage":{"output_tokens":5}}}"#,
                Usage::default(),
            ),
        ];
        for (case, bytes, usage) in cases {
            assert_eq!(read(&[bytes]), usage, "{case}");
        }
    }
}
