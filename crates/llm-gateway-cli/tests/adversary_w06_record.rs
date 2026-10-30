//! Adversary cases for story:credential-buffers-zeroed, pass 1. The unit's verification record
//! `docs/verification/2026-10-30-credential-buffers-zeroed.md` is the contract it wrote about
//! itself: these cases hold it to the code of the branch it is merged with.

use std::{fs, path::Path};

const RECORD: &str = "docs/verification/2026-10-30-credential-buffers-zeroed.md";

fn root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn read(relative: &str) -> String {
    fs::read_to_string(root().join(relative)).unwrap()
}

/// The text under `heading` up to the next second-level heading.
fn section<'a>(text: &'a str, heading: &str) -> &'a str {
    let start = text
        .find(heading)
        .unwrap_or_else(|| panic!("{RECORD} has no {heading:?}"));
    let body = &text[start + heading.len()..];
    body.find("\n## ").map_or(body, |end| &body[..end])
}

/// Every test the branch added to hold a credential buffer, by the file it lives in. The record
/// says what each buffer's guarantee rests on, so each one is named in its table, the two the
/// branch's last commit added (owner-token hashing, the owner secret's text) included.
#[test]
fn adv_w06_the_record_names_the_test_of_every_buffer_the_branch_sized() {
    let record = read(RECORD);
    let table = section(&record, "## Per buffer");
    let tests = [
        (
            "crates/llm-gateway/src/server.rs",
            "a_head_is_overwritten_in_every_allocation_it_used_when_it_is_answered",
        ),
        (
            "crates/llm-gateway/src/server.rs",
            "a_refused_head_is_overwritten_in_every_allocation_it_used_on_every_path",
        ),
        (
            "crates/llm-gateway-cli/src/trusted.rs",
            "a_credential_file_is_read_into_one_allocation_taken_at_the_bound_before_its_first_byte",
        ),
        (
            "crates/llm-gateway-cli/src/trusted.rs",
            "a_credential_file_refused_after_it_was_read_is_overwritten_first",
        ),
        (
            "crates/llm-gateway-cli/src/keys.rs",
            "a_key_file_read_is_overwritten_whichever_way_the_rules_go",
        ),
        (
            "crates/llm-gateway-cli/src/hosted.rs",
            "a_probe_request_is_sized_before_its_key_is_written_and_overwritten_when_dropped",
        ),
        (
            "crates/llm-gateway-cli/src/credential.rs",
            "a_typed_key_never_outgrows_the_allocation_taken_before_its_first_byte",
        ),
        (
            "crates/llm-gateway/src/auth.rs",
            "a_message_is_hashed_in_one_allocation_whatever_its_length",
        ),
        (
            "crates/llm-gateway-cli/src/serve.rs",
            "the_owner_secret_files_text_is_overwritten_whichever_way_the_rules_go",
        ),
    ];
    let mut unnamed = Vec::new();
    for (file, test) in tests {
        assert!(
            read(file).contains(&format!("fn {test}()")),
            "{file} has no test {test}"
        );
        if !table.contains(test) {
            unnamed.push(format!("{file}: {test}"));
        }
    }
    assert!(
        unnamed.is_empty(),
        "{RECORD}'s per-buffer table names no row for these buffer tests in the tree: {unnamed:#?}"
    );
}

/// The record's "Not changed by this story" says three sites still free a credential unzeroed.
/// The branch's last commit changed each of them; the record still lists them as they were.
#[test]
fn adv_w06_the_record_lists_as_unchanged_no_site_the_branch_changed() {
    let record = read(RECORD);
    let unchanged = section(&record, "## Not changed by this story");
    let auth = read("crates/llm-gateway/src/auth.rs");
    let serve = read("crates/llm-gateway-cli/src/serve.rs");
    let bedrock = read("crates/llm-gateway-cli/src/bedrock.rs");
    let secret_first = match (
        bedrock.find("Secret::new(text.into_bytes())"),
        bedrock.find("SecretVersion::new(hex_digest(secret.expose()))"),
    ) {
        (Some(secret), Some(version)) => secret < version,
        _ => false,
    };
    let sites = [
        (
            "`sha256`",
            auth.contains("let mut padded = Vec::with_capacity(length);"),
            "auth.rs sizes the padded buffer before the message enters it",
        ),
        (
            "`owner_verifier`",
            serve.contains("trusted::Wiped::adopt(text.into_bytes())"),
            "serve.rs holds the owner secret's text in a Wiped",
        ),
        (
            "`TokenFile::resolve`",
            secret_first,
            "bedrock.rs moves the token into llm's Secret before the version check",
        ),
    ];
    let stale: Vec<String> = sites
        .into_iter()
        .filter(|(name, changed, _)| *changed && unchanged.contains(name))
        .map(|(name, _, how)| format!("{name}: listed as unchanged, but {how}"))
        .collect();
    assert!(stale.is_empty(), "{RECORD}: {stale:#?}");
}
