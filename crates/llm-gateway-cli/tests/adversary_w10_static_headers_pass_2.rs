//! Second adversarial pass over story:hosted-static-headers, against the correction in 26bbe6c:
//! `[hosted.<alias>.headers]` is read through `header_table`, which is to refuse anything but a
//! table of names to strings as `config:schema` and quote no value. Measured through `parse`,
//! which reads no file the document names.

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

/// `headers` is a table of names to strings; a TOML datetime is neither. Each of the four
/// datetime shapes must be refused `config:schema`, as a string or an integer is.
#[test]
fn adversary_pass2_headers_written_as_a_toml_datetime_is_refused() {
    let cases = [
        ("an offset datetime", "headers = 1979-05-27T07:32:00Z\n"),
        ("a local datetime", "headers = 1979-05-27T07:32:00\n"),
        ("a local date", "headers = 1979-05-27\n"),
        ("a local time", "headers = 07:32:00\n"),
    ];
    let mut wrong = Vec::new();
    for (case, extra) in cases {
        match parse(&document(extra)) {
            Ok(deployment) => {
                let sent = deployment.hosted["remote"]
                    .headers
                    .iter()
                    .map(|header| format!("{}: {}", header.name(), header.value()))
                    .collect::<Vec<_>>();
                wrong.push(format!("{case}: accepted, every request is sent {sent:?}"));
            }
            Err(refusal) if refusal.code() != "config:schema" => {
                wrong.push(format!("{case}: {} {refusal}", refusal.code()));
            }
            Err(_) => {}
        }
    }
    assert!(
        wrong.is_empty(),
        "headers written as a datetime:\n{}",
        wrong.join("\n")
    );
}

/// `headers` written as a bare scalar that is not a string: the refusal names the shape and
/// never the value, as the commit 26bbe6c says of a string.
#[test]
fn adversary_pass2_headers_written_as_a_non_string_scalar_quotes_no_value() {
    let cases = [
        (
            "an integer",
            "headers = 4111111111111111\n",
            "4111111111111111",
        ),
        (
            "a float",
            "headers = 31415926.5358979\n",
            "31415926.5358979",
        ),
        ("a boolean", "headers = true\n", "true"),
    ];
    let mut quoted = Vec::new();
    for (case, extra, secret) in cases {
        match parse(&document(extra)) {
            Ok(_) => quoted.push(format!("{case}: accepted")),
            Err(refusal) => {
                let message = refusal.to_string();
                if refusal.code() != "config:schema" || message.contains(secret) {
                    quoted.push(format!("{case}: {} {message}", refusal.code()));
                }
            }
        }
    }
    assert!(
        quoted.is_empty(),
        "refusals of a scalar headers that quote it:\n{}",
        quoted.join("\n")
    );
}

/// The other shapes the brief names: arrays of tables, nested inline tables, a datetime or an
/// integer as one value, and very long strings. Each is refused and none quotes a value.
#[test]
fn adversary_pass2_other_shapes_are_refused_without_quoting() {
    let long_value = "v".repeat(1 << 20);
    let long_name = "n".repeat(100_000);
    let cases = [
        (
            "an array of tables",
            format!("[[hosted.remote.headers]]\nx-title = \"{LEAK}\"\n"),
        ),
        (
            "an array of inline tables",
            format!("headers = [{{ x-title = \"{LEAK}\" }}]\n"),
        ),
        (
            "a nested inline table",
            format!("headers = {{ x-title = {{ inner = \"{LEAK}\" }} }}\n"),
        ),
        (
            "a doubly nested inline table beside a string",
            format!("headers = {{ a = \"{LEAK}\", b = {{ c = {{ d = \"{LEAK}\" }} }} }}\n"),
        ),
        (
            "an array of strings as one value",
            format!("[hosted.remote.headers]\nx-title = [\"{LEAK}\"]\n"),
        ),
        (
            "a datetime as one value beside a string",
            format!(
                "[hosted.remote.headers]\nx-title = \"{LEAK}\"\nx-date = 1979-05-27T07:32:00Z\n"
            ),
        ),
        (
            "an integer as one value beside a string",
            format!("[hosted.remote.headers]\nx-title = \"{LEAK}\"\nx-pin = 4111111111111111\n"),
        ),
        (
            "a value of one MiB",
            format!("[hosted.remote.headers]\nx-title = \"{LEAK}{long_value}\"\n"),
        ),
        (
            "a name of 100000 bytes",
            format!("[hosted.remote.headers]\n{long_name} = \"{LEAK}\"\n"),
        ),
        (
            "a multi-line string holding a header line",
            format!("[hosted.remote.headers]\nx-title = \"\"\"{LEAK}\nx-injected: b\"\"\"\n"),
        ),
        (
            "a quoted name holding CR LF",
            format!("[hosted.remote.headers]\n\"x-a\\r\\nx-b\" = \"{LEAK}\"\n"),
        ),
    ];
    let mut wrong = Vec::new();
    for (case, extra) in cases {
        match parse(&document(&extra)) {
            Ok(_) => wrong.push(format!("{case}: accepted")),
            Err(refusal) => {
                let message = refusal.to_string();
                if message.contains(LEAK) || message.contains(&long_value[..64]) {
                    wrong.push(format!("{case}: {} {message}", refusal.code()));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
