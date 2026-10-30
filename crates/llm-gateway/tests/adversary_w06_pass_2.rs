//! Adversary pass 2 on `story:credential-buffers-zeroed` (wave 2026-10-30-w06).
//!
//! `docs/verification/gateway.md:9-11` makes two counts. The first, the six sources of its lane
//! table, is held by `adversary_pass_2::the_verification_records_case_count_is_the_count_the_suite_declares`,
//! and the unit kept it at 47 by folding a case into `the_digest_is_sha_256`. The second, "52 in
//! the whole crate once `src/server.rs` and `tests/adversary_pass_2.rs` are included", names
//! `src/server.rs` explicitly, and the unit added four `#[test]`s to it (`head_tests`). Nothing
//! holds that number, so it moves with every case added there and no gate says so.

const RECORD: &str = include_str!("../../../docs/verification/gateway.md");

fn declared(source: &str) -> usize {
    source
        .lines()
        .filter(|line| line.trim() == "#[test]")
        .count()
}

/// What this proves: the record's second count is the number of cases the eight files it names
/// declare: the six sources of its lane table, `src/server.rs` and `tests/adversary_pass_2.rs`.
#[test]
fn adv_w06_the_records_count_with_server_rs_included_is_what_those_files_declare() {
    let sources = [
        include_str!("../src/auth.rs"),
        include_str!("../src/inventory.rs"),
        include_str!("../src/json.rs"),
        include_str!("gateway.rs"),
        include_str!("adversary.rs"),
        include_str!("dependency_boundary.rs"),
        include_str!("../src/server.rs"),
        include_str!("adversary_pass_2.rs"),
    ];
    let declared: usize = sources.into_iter().map(declared).sum();
    let marker = "cases across the six sources this lane\ntable covers, and ";
    let start = RECORD
        .find(marker)
        .expect("the record no longer states a whole-crate count after its lane count")
        + marker.len();
    let claimed: usize = RECORD[start..]
        .split_once(' ')
        .expect("no whole-crate count")
        .0
        .parse()
        .expect("unparsable whole-crate count");
    assert_eq!(
        claimed,
        declared,
        "docs/verification/gateway.md:10 says {claimed} cases once src/server.rs and \
         tests/adversary_pass_2.rs are included; the eight files it names declare {declared} \
         (src/server.rs alone declares {})",
        declared_in_server()
    );
}

fn declared_in_server() -> usize {
    declared(include_str!("../src/server.rs"))
}
