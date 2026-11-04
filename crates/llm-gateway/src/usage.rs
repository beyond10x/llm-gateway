//! Token counts read from a relayed answer as it streams (story:live-dashboard,
//! story:token-and-cache-metrics).
//!
//! The scanner looks at the answer's decoded body bytes, a JSON body or server-sent events, for
//! the usage keys of the three wires and keeps the largest value each one reported:
//!
//! - messages: `input_tokens`, `output_tokens`, `cache_read_input_tokens`,
//!   `cache_creation_input_tokens` (in `message_start` and again, cumulative, in `message_delta`);
//! - chat: `prompt_tokens`, `completion_tokens`, `cached_tokens` (in `prompt_tokens_details`);
//! - responses: `input_tokens`, `output_tokens`, `cached_tokens` (in `input_tokens_details`).
//!
//! It parses nothing else and holds at most [`TAIL`] bytes between pieces, so a key split
//! across two pieces is still read. A key is read only where it stands as a JSON member name: a
//! quote not escaped by a backslash, the name, a closing quote, optional space, a colon, optional
//! space and digits. A key that appears inside a string value is written there with escaped
//! quotes and is not read. A value the answer reported as zero is zero; a key it never sent
//! stays `None`.

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

/// The keys read, each with the field it feeds.
const KEYS: [(&[u8], Field); 7] = [
    (b"input_tokens", Field::Input),
    (b"prompt_tokens", Field::Input),
    (b"output_tokens", Field::Output),
    (b"completion_tokens", Field::Output),
    (b"cache_read_input_tokens", Field::CacheRead),
    (b"cached_tokens", Field::CacheRead),
    (b"cache_creation_input_tokens", Field::CacheCreation),
];

/// The bytes kept from one piece to the next: the longest key with its quotes, space, colon
/// and a 20-digit value.
const TAIL: usize = 64;

#[derive(Debug, Clone, Copy)]
enum Field {
    Input,
    Output,
    CacheRead,
    CacheCreation,
}

/// Reads [`Usage`] from an answer fed to it piece by piece.
#[derive(Debug, Default)]
pub struct UsageScanner {
    usage: Usage,
    carry: Vec<u8>,
}

impl UsageScanner {
    /// Reads `piece`, the next decoded body bytes of the answer.
    pub fn feed(&mut self, piece: &[u8]) {
        let mut window = std::mem::take(&mut self.carry);
        window.extend_from_slice(piece);
        // A value cut off at the end of the window is read again, whole, with the next piece.
        let complete = complete_prefix(&window);
        scan(&window[..complete], &mut self.usage);
        // The last TAIL bytes before the trailing digits, and the digits: a key cut off at the
        // end, or a value still being written, is read whole with the next piece. A value in
        // them already read is read again, which keeps the same maximum. A run of digits longer
        // than any value is no value, and only its end is kept.
        let keep = complete
            .saturating_sub(TAIL)
            .max(window.len().saturating_sub(4 * TAIL));
        self.carry = window[keep..].to_vec();
    }

    /// What the answer reported so far.
    pub fn usage(&self) -> Usage {
        let mut usage = self.usage;
        scan(&self.carry, &mut usage);
        usage
    }
}

/// The length of `window` up to its last byte that cannot be the start of a value still being
/// written: a window ending in digits may continue them.
fn complete_prefix(window: &[u8]) -> usize {
    let trailing = window
        .iter()
        .rev()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    window.len() - trailing
}

fn scan(window: &[u8], usage: &mut Usage) {
    let mut at = 0;
    while let Some(offset) = window[at..].iter().position(|&byte| byte == b'"') {
        let quote = at + offset;
        at = quote + 1;
        if quote > 0 && window[quote - 1] == b'\\' {
            continue;
        }
        for (key, field) in KEYS {
            let rest = &window[quote + 1..];
            let Some(after) = rest.strip_prefix(key) else {
                continue;
            };
            let Some(after) = after.strip_prefix(b"\"") else {
                continue;
            };
            let after = skip_space(after);
            let Some(after) = after.strip_prefix(b":") else {
                continue;
            };
            let after = skip_space(after);
            let digits = after
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            if digits == 0 || digits > 19 {
                continue;
            }
            let Some(value) = std::str::from_utf8(&after[..digits])
                .ok()
                .and_then(|text| text.parse::<u64>().ok())
            else {
                continue;
            };
            let slot = match field {
                Field::Input => &mut usage.input_tokens,
                Field::Output => &mut usage.output_tokens,
                Field::CacheRead => &mut usage.cache_read_tokens,
                Field::CacheCreation => &mut usage.cache_creation_tokens,
            };
            *slot = Some(slot.map_or(value, |seen| seen.max(value)));
        }
    }
}

fn skip_space(bytes: &[u8]) -> &[u8] {
    let spaces = bytes
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        .count();
    &bytes[spaces..]
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
}
