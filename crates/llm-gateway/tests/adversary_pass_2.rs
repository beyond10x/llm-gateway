//! Second adversarial pass over `story:gateway-auth`.
//!
//! Every case here drives the shipped code and the shipped documents against each other. Nothing
//! in this file copies an implementation: the bound cases call the crate's real public API, and
//! the document cases read the published markdown with `include_str!`.

use llm_gateway::{
    AuthKind, BillingKind, Gateway, GatewayConfig, GatewayHandle, InventoryError, Label,
    OwnerToken, RouteInventory, RouteSummary, SharedSecretVerifier, TargetLimits, TargetProvenance,
    TargetSummary,
};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};

const OWNER_SECRET: &str = "owner-token-0123456789abcdef0123456789abcdef";

const CONTRACT: &str = include_str!("../../../docs/gateway.md");
const RECORD: &str = include_str!("../../../docs/verification/gateway.md");

// ---------------------------------------------------------------------------------------------
// Shared fixtures
// ---------------------------------------------------------------------------------------------

fn label(value: &str) -> Label {
    Label::new(value).unwrap()
}

fn one_route() -> RouteSummary {
    RouteSummary::new(
        label("coding"),
        label("code"),
        false,
        vec![TargetSummary {
            target_id: label("coding-primary"),
            position: 0,
            provenance: TargetProvenance {
                protocol: label("chat-completions"),
                provider: label("my-lab"),
                account: label("remote"),
                endpoint: label("remote-models"),
                model: label("large"),
                binding_revision: label("rev-1"),
            },
            auth_kind: AuthKind::Bearer,
            billing_kind: BillingKind::Metered,
            limits: TargetLimits::default(),
        }],
    )
    .unwrap()
}

fn inventory() -> RouteInventory {
    RouteInventory::new(vec![one_route()]).unwrap()
}

fn bind(configure: impl FnOnce(&mut GatewayConfig)) -> GatewayHandle {
    let token = OwnerToken::new(OWNER_SECRET.as_bytes().to_vec()).unwrap();
    let verifier = Arc::new(SharedSecretVerifier::new(token).unwrap());
    let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
    config.read_timeout = Duration::from_secs(5);
    configure(&mut config);
    Gateway::bind(config, verifier, inventory()).unwrap()
}

#[derive(Debug)]
struct Response {
    status: u16,
    body: String,
    raw: String,
}

fn exchange(addr: SocketAddr, request: &[u8]) -> Response {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(request).unwrap();
    stream.flush().unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    let raw = String::from_utf8_lossy(&bytes).into_owned();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_str(), ""));
    let status = head
        .split("\r\n")
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    Response {
        status,
        body: body.to_string(),
        raw: raw.clone(),
    }
}

/// The first cell of every row of the named markdown table section, for rows written `| `x` |`.
fn first_cells(document: &str, heading: &str) -> Vec<String> {
    let section = document
        .split_once(heading)
        .unwrap_or_else(|| panic!("no section {heading}"))
        .1
        .split("\n## ")
        .next()
        .unwrap_or_default();
    section
        .lines()
        .filter(|line| line.starts_with("| `"))
        .filter_map(|line| line.trim_matches('|').split('|').next())
        .map(|cell| cell.trim().trim_matches('`').to_string())
        .collect()
}

// ---------------------------------------------------------------------------------------------
// 1. docs/gateway.md:161 — "Every bound the crate enforces, with the value it enforces."
//
//    `RouteInventory::with_config_digest` is public API and enforces a bound of exactly 64
//    characters (`DIGEST_CHARACTERS` in src/inventory.rs:26). The Bounds table does not list it,
//    and it cannot: `every_published_bound_flips_at_exactly_the_published_number` compares the
//    table against `enforced_bounds()`, a list hand-written in tests/gateway.rs:684, so the
//    table's completeness is checked against another hand-written list and never against the
//    source. A bound absent from both is invisible to the whole mechanism.
// ---------------------------------------------------------------------------------------------

#[test]
fn the_published_bounds_table_names_every_bound_the_crate_enforces() {
    // Measured first, on public API, at literal numbers -- not against the crate's constant.
    assert_eq!(
        inventory().with_config_digest(&"0".repeat(63)).unwrap_err(),
        InventoryError::MalformedDigest,
        "63 digest characters must be refused"
    );
    assert!(
        inventory().with_config_digest(&"0".repeat(64)).is_ok(),
        "64 digest characters must be admitted"
    );
    assert_eq!(
        inventory().with_config_digest(&"0".repeat(65)).unwrap_err(),
        InventoryError::MalformedDigest,
        "65 digest characters must be refused"
    );

    assert!(
        CONTRACT.contains("Every bound the crate enforces, with the value it enforces."),
        "the completeness claim this case is written against has moved"
    );
    let published = first_cells(CONTRACT, "\n## Bounds\n");
    assert!(
        published.iter().any(|name| name.contains("digest")),
        "the crate enforces a 64-character bound on `config_digest`, measured above at 63, 64 \
         and 65 through the public `RouteInventory::with_config_digest`. docs/gateway.md:161 \
         says its Bounds table lists every bound the crate enforces; it lists {published:?}. \
         `every_published_bound_flips_at_exactly_the_published_number` cannot notice, because \
         it compares the table against `enforced_bounds()` in tests/gateway.rs, another list \
         written by hand -- never against the source."
    );
}

// ---------------------------------------------------------------------------------------------
// 2. docs/verification/gateway.md:27-32 names four cases as shipped and red. They are not in the
//    tree: the coordinator retired them after the record was written.
// ---------------------------------------------------------------------------------------------

#[test]
fn every_case_the_verification_record_names_as_shipped_exists_in_the_suite() {
    let sources = [
        include_str!("gateway.rs"),
        include_str!("adversary.rs"),
        include_str!("dependency_boundary.rs"),
        include_str!("../src/auth.rs"),
        include_str!("../src/inventory.rs"),
        include_str!("../src/json.rs"),
    ];
    let named: Vec<String> = first_cells(RECORD, "\n## Counted coverage\n")
        .into_iter()
        .filter(|cell| {
            cell.contains('_') && !cell.contains('-') && !cell.contains('.') && !cell.contains(' ')
        })
        .collect();
    assert!(
        !named.is_empty(),
        "the record's table of shipped cases could not be parsed"
    );
    let missing: Vec<&String> = named
        .iter()
        .filter(|name| {
            !sources
                .iter()
                .any(|source| source.contains(&format!("fn {name}(")))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "docs/verification/gateway.md describes these cases as shipped and red, and no source \
         file in the crate declares them: {missing:?}. The record is the artefact that makes a \
         passing suite mean something, and this one describes a suite that does not exist."
    );
}

#[test]
fn the_verification_records_case_count_is_the_count_the_suite_declares() {
    // The six sources the record's own lane table covers. This case's own file is deliberately
    // not among them, so adding cases here cannot move the number.
    let sources = [
        include_str!("../src/auth.rs"),
        include_str!("../src/inventory.rs"),
        include_str!("../src/json.rs"),
        include_str!("gateway.rs"),
        include_str!("adversary.rs"),
        include_str!("dependency_boundary.rs"),
    ];
    let declared: usize = sources
        .iter()
        .map(|source| {
            source
                .lines()
                .filter(|line| line.trim() == "#[test]")
                .count()
        })
        .sum();
    let marker = "runs **";
    let start = RECORD.find(marker).expect("no case count in the record") + marker.len();
    let claimed: usize = RECORD[start..]
        .split_once(' ')
        .expect("no case count in the record")
        .0
        .trim_end_matches('*')
        .parse()
        .expect("unparsable case count in the record");
    assert_eq!(
        claimed, declared,
        "docs/verification/gateway.md:9 says the suite runs {claimed} cases; the six sources its \
         lane table covers declare {declared}. Its adversary lane claims `2 passed, 4 failed`, \
         and tests/adversary.rs declares two cases and no failing one."
    );
}

// ---------------------------------------------------------------------------------------------
// 3. docs/gateway.md:154 — "a connection the report counts as completed must not have been
//    answered with a refusal". That sentence is this unit's answer to pass 1's shutdown blocker.
//    As written it is an unqualified invariant over the report, and the report counts every
//    ordinary refusal as completed.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_connection_the_report_counts_as_completed_was_not_answered_with_a_refusal() {
    let handle = bind(|_| {});
    handle.mark_ready();
    let addr = handle.local_addr();

    // One connection, no credential. Nothing about this is a shutdown.
    let refused = exchange(addr, b"GET /v1/routes HTTP/1.1\r\nhost: h\r\n\r\n");
    assert_eq!(refused.status, 401, "{}", refused.raw);

    let report = handle.shutdown();
    assert_eq!(report.accepted, 1, "{report:?}");
    assert_eq!(report.completed, 1, "{report:?}");

    // Re-pointed by the coordinator. As written this case asserted the status was 401 and then
    // that it was below 400, on the same value, so no behaviour could satisfy it. It was a
    // demonstration that the documented invariant was false as written, and it succeeded: the
    // sentence is now narrowed. `completed` counts every accepted connection, including ones
    // refused on their own merits. What the shutdown ordering provides is that none was refused
    // *because of the stop*, and `unavailable` is the only code the stop can produce.
    assert!(
        !refused.body.contains("\"code\":\"unavailable\""),
        "a connection the report counts as completed ({} of {}) was refused because of the \
         stop, which docs/gateway.md:154 says cannot happen: {}",
        report.completed,
        report.accepted,
        refused.body
    );
}

// ---------------------------------------------------------------------------------------------
// 4. docs/gateway.md:161-164 — the bounds table's numbers "are checked against measured
//    behaviour, at the number itself and at one past it". For `request-head-bytes` and
//    `concurrent-requests` the suite's `admits` closure reads `GatewayConfig::new(..)` and
//    compares the default field to the number (tests/gateway.rs:744 and :753). No case measures
//    the enforcement. These two cases measure it.
// ---------------------------------------------------------------------------------------------

#[test]
fn the_published_request_head_bound_is_enforced_at_the_published_number() {
    let handle = bind(|_| {});
    handle.mark_ready();
    let addr = handle.local_addr();
    assert_eq!(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()).max_head_bytes,
        8192,
        "docs/gateway.md publishes this number"
    );

    // A head of exactly 8192 bytes, and one of 8193, both otherwise valid and authenticated.
    let head = |total: usize| {
        let prefix = format!(
            "GET /v1/routes HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER_SECRET}\r\nx-pad: "
        );
        let suffix = "\r\n\r\n";
        let pad = total - prefix.len() - suffix.len();
        let raw = format!("{prefix}{}{suffix}", "p".repeat(pad));
        assert_eq!(raw.len(), total);
        raw
    };

    let at = exchange(addr, head(8192).as_bytes());
    assert_eq!(
        at.status, 200,
        "a head of exactly 8192 bytes must be served: {}",
        at.raw
    );
    let past = exchange(addr, head(8193).as_bytes());
    assert_eq!(
        past.status, 431,
        "a head of 8193 bytes must be refused as request-too-large: {}",
        past.raw
    );
    assert!(past.body.contains("request-too-large"), "{}", past.body);
}

#[test]
fn the_published_concurrency_bound_is_enforced_at_the_published_number() {
    let handle = bind(|_| {});
    handle.mark_ready();
    let addr = handle.local_addr();
    assert_eq!(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()).max_concurrent_requests,
        64,
        "docs/gateway.md publishes this number"
    );

    // Occupy 63 slots with connections whose head is not yet terminated.
    let mut held = Vec::new();
    for _ in 0..63 {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .write_all(b"GET /v1/routes HTTP/1.1\r\nhost: h\r\n")
            .unwrap();
        stream.flush().unwrap();
        held.push(stream);
    }
    std::thread::sleep(Duration::from_millis(400));

    // The 64th concurrent connection is inside the published bound.
    let inside = exchange(
        addr,
        format!(
            "GET /v1/routes HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
        )
        .as_bytes(),
    );
    assert_eq!(
        inside.status, 200,
        "the 64th concurrent connection is the published maximum and must be served: {}",
        inside.raw
    );

    // One more held connection, and the next is one past the bound.
    let mut extra = TcpStream::connect(addr).unwrap();
    extra
        .write_all(b"GET /v1/routes HTTP/1.1\r\nhost: h\r\n")
        .unwrap();
    extra.flush().unwrap();
    held.push(extra);
    std::thread::sleep(Duration::from_millis(400));

    let outside = exchange(
        addr,
        format!(
            "GET /v1/routes HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
        )
        .as_bytes(),
    );
    assert_eq!(
        outside.status, 503,
        "the 65th concurrent connection is one past the published maximum: {}",
        outside.raw
    );
    assert!(outside.body.contains("overloaded"), "{}", outside.body);

    drop(held);
    handle.shutdown();
}

// ---------------------------------------------------------------------------------------------
// 5. docs/gateway.md:131-134 — "`RefusalCode`, `RefusalCode::ALL`, `wire`, `status` and `reason`
//    are generated from a single list by the `refusal_codes!` macro, so a variant cannot exist
//    without all five -- that is a compile error, not a test."
//
//    The status line's *reason phrase* is not one of the five. `reason_phrase` in
//    src/server.rs:497 is a hand-written match over the seven statuses in use, and nothing
//    compares it to `RefusalCode::status()`. A variant added to the generating list with a
//    status outside that match compiles, is forced into the published table and into `provoke`,
//    and still answers `HTTP/1.1 <status> Unknown`. This is the case that would catch it.
// ---------------------------------------------------------------------------------------------

#[test]
fn every_refusal_answers_with_a_reason_phrase_rather_than_unknown() {
    let handle = bind(|_| {});
    handle.mark_ready();
    let addr = handle.local_addr();
    // One request per distinct refusal status the crate can answer with.
    for raw in [
        "GET /v1/routes HTTP/1.1\r\nhost: h\r\n\r\n".to_string(),
        "not-a-request-line\r\n\r\n".to_string(),
        format!(
            "DELETE /v1/routes HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
        ),
        format!(
            "GET /v1/nothing HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
        ),
    ] {
        let response = exchange(addr, raw.as_bytes());
        let status_line = response.raw.split("\r\n").next().unwrap_or_default();
        assert!(
            !status_line.contains("Unknown"),
            "the status line carries no reason phrase for this refusal, because \
             `reason_phrase` in src/server.rs is not generated by `refusal_codes!` and is \
             compared to nothing: {status_line}"
        );
    }
    handle.shutdown();
}
