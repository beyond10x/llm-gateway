//! Adversary pass 1 of wave 2026-11-26-w05 (story:cli-test-port-race, story:test-budget).
//!
//! A port chosen by binding a listener and dropping it is free for any other socket on the host
//! to take before it is used: nextest runs every test in a process of its own, all on one
//! loopback, so the race is between processes as much as between threads. These cases read the
//! sources as text and name every place that binds a listener, takes its address, and then lets
//! the listener go (an explicit `drop` right after, or the address as the tail of its block)
//! while the address lives on.
//!
//! They also hold the README's table of the binary's own lines to the line this wave added.

use std::path::{Path, PathBuf};

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root")
}

/// Every `.rs` file under `directory`, recursively, sorted.
fn rust_files(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return found;
    };
    for entry in entries {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            found.extend(rust_files(&path));
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            found.push(path);
        }
    }
    found.sort();
    found
}

/// The byte offset just past the statement that starts at `from`: the `;` at depth 0, or the
/// position of the `}` that closes the enclosing block (a tail expression). String literals and
/// nested brackets are skipped. Returns the end and whether it was a `}`.
fn statement_end(text: &[u8], from: usize) -> Option<(usize, bool)> {
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut index = from;
    while index < text.len() {
        let byte = text[index];
        if in_string {
            if byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == b'"' {
                in_string = false;
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b'(' | b'[' | b'{' => depth += 1,
                b'}' if depth == 0 => return Some((index, true)),
                b')' | b']' | b'}' => depth -= 1,
                b';' if depth == 0 => return Some((index + 1, false)),
                _ => {}
            }
        }
        index += 1;
    }
    None
}

/// `file:line` of every listener bound with `let <name> = TcpListener::bind(…)` whose next
/// statement takes `<name>.local_addr()` and after which the listener is let go at once: the
/// statement is the block's tail, or the one after it is `drop(<name>);`.
fn released_ports(path: &Path) -> Vec<String> {
    let source = std::fs::read_to_string(path).expect("a source file");
    let text = source.as_bytes();
    let mut found = Vec::new();
    for (at, _) in source.match_indices("TcpListener::bind(") {
        let Some(let_at) = source[..at].rfind("let ") else {
            continue;
        };
        let binding = source[let_at + 4..at].trim_start_matches("mut ");
        let Some((name, rest)) = binding.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if !rest.trim().is_empty() && rest.trim() != "std::net::" {
            continue;
        }
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            continue;
        }
        let Some((bound_end, false)) = statement_end(text, at) else {
            continue;
        };
        let Some((next_end, tail)) = statement_end(text, bound_end) else {
            continue;
        };
        let next = &source[bound_end..next_end];
        if !next.contains(&format!("{name}.local_addr()")) {
            continue;
        }
        let dropped = source[next_end..]
            .trim_start()
            .starts_with(&format!("drop({name});"));
        if tail || dropped {
            let line = source[..at].matches('\n').count() + 1;
            let relative = path
                .strip_prefix(repository())
                .unwrap_or(path)
                .display()
                .to_string();
            found.push(format!("{relative}:{line}"));
        }
    }
    found
}

/// story:cli-test-port-race Acceptance 2: no test of this crate hands a process an address whose
/// listener it already let go. `security_w07_hosted.rs`'s `closed_address` binds port 0 and
/// returns the address with the listener dropped at the end of the function; the trace test
/// names that address as the `closed` endpoint in the document the shipped binary serves, so a
/// listener of any other test may hold that port when the gateway connects there.
#[test]
fn adversary_w05_no_test_of_the_cli_crate_hands_on_a_released_port() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let released: Vec<String> = rust_files(&tests)
        .iter()
        .filter(|path| !path.ends_with("adversary_w05_port_race.rs"))
        .flat_map(|path: &PathBuf| released_ports(path))
        .collect();
    assert!(
        released.is_empty(),
        "a listener is bound, its address taken, and the listener let go before the address is \
         used: {released:?}"
    );
}

/// The same race in every crate's tests and in-source test modules: a probe of a "closed"
/// loopback port whose listener was dropped can reach a listener another test bound there.
#[test]
fn adversary_w05_no_test_of_the_workspace_probes_a_released_port() {
    let root = repository();
    let mut released = Vec::new();
    for top in ["crates", "checks"] {
        for path in rust_files(&root.join(top)) {
            if path.ends_with("adversary_w05_port_race.rs") {
                continue;
            }
            released.extend(released_ports(&path));
        }
    }
    assert!(
        released.is_empty(),
        "a listener is bound, its address taken, and the listener let go before the address is \
         used: {released:?}"
    );
}

/// The scanner itself sees each shape it claims to: a tail-expression release, an explicit
/// `drop`, and it passes a listener that is held while its address is used.
#[test]
fn adversary_w05_the_released_port_scan_sees_both_shapes_and_passes_a_held_listener() {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("adversary-w05-port-scan");
    std::fs::create_dir_all(&directory).unwrap();
    let file = directory.join("shapes.rs");
    std::fs::write(
        &file,
        "fn a() -> SocketAddr {\n    let listener = TcpListener::bind(\"127.0.0.1:0\").unwrap();\n    listener.local_addr().unwrap()\n}\n\
         fn b() {\n    let closed = TcpListener::bind(\"127.0.0.1:0\").expect(\"x\");\n    let endpoint = format!(\"http://{}/v1/\", closed.local_addr().expect(\"addr\"));\n    drop(closed);\n    use_it(&endpoint);\n}\n\
         fn c() {\n    let holder = TcpListener::bind(\"127.0.0.1:0\").unwrap();\n    let config = valid(&holder.local_addr().unwrap().to_string());\n    run(&config);\n    drop(holder);\n}\n\
         fn d() -> (TcpListener, SocketAddr) {\n    let listener = TcpListener::bind(\"127.0.0.1:0\").unwrap();\n    let address = listener.local_addr().unwrap();\n    (listener, address)\n}\n",
    )
    .unwrap();
    let found: Vec<String> = released_ports(&file)
        .into_iter()
        .map(|place| place.rsplit(':').next().unwrap().to_owned())
        .collect();
    assert_eq!(found, ["2", "6"]);
}

/// The README's table of the lines the binary writes on standard error, and the sentence before
/// it that `RUST_LOG=warn` leaves only those lines, still list `listening on` alone for a
/// serving process; this wave added `metrics listening on <address>`, written at every level
/// before `listening on` when the document sets `metrics_listen` (spec/domains/deployment.yaml,
/// website/docs/reference/cli.md), and the README names it nowhere.
#[test]
fn adversary_w05_the_readme_line_table_names_the_metrics_listening_line() {
    let readme = std::fs::read_to_string(repository().join("README.md")).expect("README.md");
    let table: Vec<&str> = readme
        .lines()
        .skip_while(|line| !line.starts_with("| Event | Line on standard error |"))
        .take_while(|line| line.starts_with('|'))
        .collect();
    assert!(
        !table.is_empty(),
        "the README has no table of the binary's lines"
    );
    assert!(
        table
            .iter()
            .any(|row| row.contains("`b10x-llm-gateway: metrics listening on <address>`")),
        "{table:#?}"
    );
}
