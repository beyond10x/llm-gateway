//! Adversarial pass over story:hosted-static-headers: `[hosted.<alias>.headers]` as the closed
//! document reads it, measured through `parse`, which reads no file the document names.
//!
//! The story's rule: every refusal of a static header names `hosted.<alias>.headers` and quotes
//! no value (`spec/domains/deployment.yaml` `HostedDeclaration` `headers`,
//! `website/docs/reference/deployment-document.md`).

use llm_gateway_cli::parse;

/// A value an operator should never see echoed back by a refusal.
const LEAK: &str = "leaked-value-0123456789abcdef";

/// A document whose one hosted endpoint, `remote`, is an `api-key` endpoint on `x-api-key`, with
/// `extra` appended to its `[hosted.remote]` table.
fn document(extra: &str) -> String {
    format!(
        "listen = \"127.0.0.1:0\"\nowner_secret_file = \"/nonexistent/owner\"\n\n\
         [hosted.remote]\nwires = [\"chat\", \"messages\"]\nbase_url = \"https://api.example.test/\"\n\
         auth_kind = \"api-key\"\ncredential_file = \"/nonexistent/key\"\n\
         credential_header = \"x-api-key\"\n{extra}"
    )
}

/// Each way an operator can get the `headers` table wrong while a value sits in it: the refusal
/// quotes none of them.
#[test]
fn adversary_no_refusal_of_a_headers_table_quotes_a_value() {
    let cases = [
        (
            "headers written as one string instead of a table",
            format!("headers = \"authorization: Bearer {LEAK}\"\n"),
        ),
        (
            "headers written as an array of lines",
            format!("headers = [\"authorization: Bearer {LEAK}\"]\n"),
        ),
        (
            "an inline table with one non-string value beside a string",
            format!("headers = {{ x-title = \"{LEAK}\", x-count = 5 }}\n"),
        ),
        (
            "a dotted name, which TOML reads as a nested table",
            format!("[hosted.remote.headers]\nx.title = \"{LEAK}\"\n"),
        ),
        (
            "a name repeated exactly",
            format!("[hosted.remote.headers]\nx-title = \"{LEAK}\"\nx-title = \"{LEAK}\"\n"),
        ),
        (
            "a value with an escape TOML does not know",
            format!("[hosted.remote.headers]\nx-title = \"{LEAK}\\q\"\n"),
        ),
        (
            "a value holding a raw DEL byte",
            format!("[hosted.remote.headers]\nx-title = \"{LEAK}\u{7f}\"\n"),
        ),
        (
            "a value holding a raw CR LF",
            format!("[hosted.remote.headers]\nx-title = \"{LEAK}\r\nx-injected: b\"\n"),
        ),
    ];
    let mut quoted = Vec::new();
    for (case, extra) in cases {
        match parse(&document(&extra)) {
            Ok(_) => quoted.push(format!("{case}: accepted")),
            Err(refusal) => {
                let message = refusal.to_string();
                if message.contains(LEAK) {
                    quoted.push(format!("{case}: {} {message}", refusal.code()));
                }
            }
        }
    }
    assert!(
        quoted.is_empty(),
        "refusals that quote a header value:\n{}",
        quoted.join("\n")
    );
}

/// Every reserved name of the story, in upper case, is refused `config:value` naming
/// `hosted.remote.headers` and quoting no value; the test the unit wrote names six of the
/// thirteen.
#[test]
fn adversary_every_reserved_name_in_any_case_is_refused() {
    let reserved = [
        "HOST",
        "Authorization",
        "Content-Type",
        "CONTENT-LENGTH",
        "Connection",
        "Transfer-Encoding",
        "TE",
        "Trailer",
        "Upgrade",
        "Expect",
        "Keep-Alive",
        "Proxy-Authorization",
        "Proxy-Connection",
        "X-API-KEY",
    ];
    for name in reserved {
        let refusal = parse(&document(&format!(
            "[hosted.remote.headers]\n\"{name}\" = \"{LEAK}\"\n"
        )))
        .expect_err(name);
        let message = refusal.to_string();
        assert_eq!(refusal.code(), "config:value", "{name}: {message}");
        assert!(
            message.contains("hosted.remote.headers"),
            "{name}: {message}"
        );
        assert!(!message.contains(LEAK), "{name}: {message}");
    }
}

/// A name of every RFC 9110 `tchar` loads, lowercased; the 0x7e and 0x20 ends of a value load.
#[test]
fn adversary_every_token_character_and_both_value_ends_load() {
    let deployment = parse(&document(
        "[hosted.remote.headers]\n\"X-!#$%&'*+-.^_`|~09AZ\" = \"~ !\"\n",
    ))
    .unwrap();
    let headers: Vec<(&str, &str)> = deployment.hosted["remote"]
        .headers
        .iter()
        .map(|header| (header.name(), header.value()))
        .collect();
    assert_eq!(headers, [("x-!#$%&'*+-.^_`|~09az", "~ !")]);
}
