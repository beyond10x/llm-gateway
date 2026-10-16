//! Adversary pass on story:hosted-base-path (wave 2026-10-16-w01): the `BasePath` rule driven
//! from its own documentation, byte by byte and over every short path, rather than from the
//! handful of examples the unit's tests list.

use llm_gateway::{BasePath, BasePathError};

const WIRE_PATHS: [&str; 4] = [
    "/v1/chat/completions",
    "/v1/responses",
    "/v1/messages",
    "/v1/messages/count_tokens",
];

fn allowed(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

/// "`%`, `\` or any other byte is `config:value`": every byte outside `A-Z a-z 0-9 - . _ ~`
/// and the separator `/`, at the start, the middle and the end of a segment, is refused
/// `NotAPath`. The unit's own negative list holds seven of the 190 such bytes, and none of the
/// URL delimiters `@ : ; = + , ! $ & ' ( ) * [ ]` that the brief names.
#[test]
fn every_byte_outside_the_rule_is_refused_wherever_it_stands() {
    for byte in (0_u8..=255).filter(|byte| !allowed(*byte) && *byte != b'/') {
        let byte = byte as char;
        for path in [
            format!("/{byte}a"),
            format!("/a{byte}b"),
            format!("/a{byte}"),
            format!("/api/v1{byte}"),
        ] {
            assert_eq!(
                BasePath::new(&path).err(),
                Some(BasePathError::NotAPath),
                "{path:?}"
            );
        }
    }
}

/// "it can neither end the request line it is written into nor address anything outside the
/// prefix it names": over every path of up to six bytes drawn from `/ . a ~`, an accepted base
/// path joined to each wire path starts with the base path, carries no dot segment, no empty
/// segment and no byte outside the rule, and keeps the rest of the wire path verbatim.
#[test]
fn no_accepted_base_path_joins_to_a_path_that_leaves_its_prefix() {
    let alphabet = [b'/', b'.', b'a', b'~'];
    let mut accepted = 0_usize;
    for length in 0..=6_u32 {
        for index in 0..alphabet.len().pow(length) {
            let mut rest = index;
            let path: String = (0..length)
                .map(|_| {
                    let byte = alphabet[rest % alphabet.len()];
                    rest /= alphabet.len();
                    byte as char
                })
                .collect();
            let Ok(base) = BasePath::new(&path) else {
                continue;
            };
            accepted += 1;
            let held = path.strip_suffix('/').unwrap_or(&path);
            assert_eq!(base.as_str(), held, "{path:?}");
            for wire in WIRE_PATHS {
                let joined = base.join(wire);
                assert_eq!(joined, format!("{held}{}", &wire[3..]), "{path:?} {wire}");
                assert!(joined.starts_with(base.as_str()), "{path:?} {wire}");
                let segments: Vec<&str> = joined[1..].split('/').collect();
                assert!(
                    segments
                        .iter()
                        .all(|segment| !segment.is_empty() && *segment != "." && *segment != ".."),
                    "{path:?} {wire} -> {joined}"
                );
                assert!(
                    joined.bytes().all(|byte| allowed(byte) || byte == b'/'),
                    "{path:?} {wire} -> {joined}"
                );
            }
        }
    }
    assert!(accepted > 0);
}
