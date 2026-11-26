//! The gate's test step, one command locally and in CI:
//! `cargo run --quiet --locked -p b10x-llm-gateway-xtask -- test`.
//!
//! `test` builds the workspace's test binaries first, outside the bound, then runs them with
//! `cargo nextest run --workspace --locked` under `.config/nextest.toml`, which kills a test at
//! 60 s and counts it failed, and ends the run at 120 s. It then reads the `JUnit` report the run
//! wrote and prints the ten slowest tests, slowest first, and the time the run took by its own
//! report, failing when that time is over 120 s. The time is read from the report, not from a
//! clock around the command, so a build or a wait for a build slot is not counted.
//!
//! `report <JUnit report>` does the last part alone, over a report already written.

use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

/// What the test run may take, in seconds.
const BOUND_SECONDS: f64 = 120.0;

/// How many of the slowest tests the step prints.
const SLOWEST: usize = 10;

/// Where cargo-nextest writes the `JUnit` report of its default profile, under the target
/// directory (`.config/nextest.toml`).
const JUNIT: &str = "nextest/default/junit.xml";

const USAGE: &str = "usage: b10x-llm-gateway-xtask test\n       \
                     b10x-llm-gateway-xtask report <JUnit report>";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let result = match arguments.as_slice() {
        [command] if command == "test" => test(),
        [command, report] if command == "report" => {
            read(Path::new(report)).and_then(|run| verdict(&run))
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("tests: {failure}");
            ExitCode::FAILURE
        }
    }
}

/// Why the step failed.
#[derive(Debug, PartialEq)]
enum Failure {
    Command(String),
    Report(String),
    Failed,
    OverBound(f64),
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(message) | Self::Report(message) => formatter.write_str(message),
            Self::Failed => formatter.write_str("`cargo nextest run --workspace --locked` failed"),
            Self::OverBound(seconds) => write!(
                formatter,
                "the run took {seconds:.1} s, longer than its bound of {BOUND_SECONDS:.0} s"
            ),
        }
    }
}

/// One test as the report names it.
#[derive(Debug, PartialEq)]
struct Case {
    seconds: f64,
    suite: String,
    name: String,
}

/// What one report says: the run's own time and each test's.
#[derive(Debug, PartialEq)]
struct Run {
    seconds: f64,
    cases: Vec<Case>,
}

fn test() -> Result<(), Failure> {
    let report = target_directory()?.join(JUNIT);
    cargo(&["nextest", "run", "--workspace", "--locked", "--no-run"]).and_then(|built| {
        if built {
            Ok(())
        } else {
            Err(Failure::Command("building the tests failed".to_owned()))
        }
    })?;
    match fs::remove_file(&report) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            return Err(Failure::Report(format!("{}: {error}", report.display())));
        }
        _ => {}
    }
    let passed = cargo(&["nextest", "run", "--workspace", "--locked"])?;
    let run = read(&report)?;
    println!(
        "tests: {} tests in {:.1} s, {}",
        run.cases.len(),
        run.seconds,
        if passed { "passed" } else { "failed" }
    );
    let bound = verdict(&run);
    if passed { bound } else { Err(Failure::Failed) }
}

/// Runs `cargo` with `arguments` and says whether it ended with success.
fn cargo(arguments: &[&str]) -> Result<bool, Failure> {
    Command::new("cargo")
        .args(arguments)
        .status()
        .map(|status| status.success())
        .map_err(|error| Failure::Command(format!("cargo does not run: {error}")))
}

/// The workspace's target directory, as `cargo metadata` names it.
fn target_directory() -> Result<PathBuf, Failure> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps", "--locked"])
        .output()
        .map_err(|error| Failure::Command(format!("cargo metadata does not run: {error}")))?;
    if !output.status.success() {
        return Err(Failure::Command(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| Failure::Command(format!("cargo metadata wrote no JSON: {error}")))?;
    metadata["target_directory"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| Failure::Command("cargo metadata names no target directory".to_owned()))
}

/// Reads the `JUnit` report cargo-nextest wrote.
fn read(report: &Path) -> Result<Run, Failure> {
    let place = report.display();
    let xml =
        fs::read_to_string(report).map_err(|error| Failure::Report(format!("{place}: {error}")))?;
    parse(&xml).map_err(|message| Failure::Report(format!("{place}: {message}")))
}

/// The run time on `<testsuites>` and every `<testcase>`'s time, suite and name.
fn parse(xml: &str) -> Result<Run, String> {
    let seconds = tags(xml, "testsuites")
        .first()
        .and_then(|tag| attribute(tag, "time"))
        .and_then(|time| time.parse::<f64>().ok())
        .ok_or_else(|| "no run time on <testsuites>".to_owned())?;
    let mut cases = Vec::new();
    for tag in tags(xml, "testcase") {
        let time = attribute(tag, "time").and_then(|time| time.parse::<f64>().ok());
        let (Some(seconds), Some(suite), Some(name)) =
            (time, attribute(tag, "classname"), attribute(tag, "name"))
        else {
            return Err("a <testcase> without a time, classname or name".to_owned());
        };
        cases.push(Case {
            seconds,
            suite: unescape(suite),
            name: unescape(name),
        });
    }
    Ok(Run { seconds, cases })
}

/// The attribute text of every start tag `<name ...>` in `xml`.
fn tags<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let open = format!("<{name} ");
    xml.match_indices(open.as_str())
        .filter_map(|(at, _)| {
            let rest = &xml[at + open.len()..];
            rest.find('>').map(|end| &rest[..end])
        })
        .collect()
}

/// The value of attribute `name` in a tag's attribute text, as written. A value holds no `"`
/// (XML writes it `&quot;`), so the first `name="` after white space and outside a value starts
/// the attribute.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let mut from = 0;
    while let Some(found) = tag[from..].find(&key) {
        let at = from + found;
        let start = at + key.len();
        let quoted_before = tag[..at].matches('"').count();
        let separated = at == 0 || tag[..at].ends_with(char::is_whitespace);
        if separated && quoted_before.is_multiple_of(2) {
            return tag[start..].find('"').map(|end| &tag[start..start + end]);
        }
        from = start;
    }
    None
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The ten slowest tests of `run`, slowest first.
fn slowest(run: &Run) -> Vec<&Case> {
    let mut cases: Vec<&Case> = run.cases.iter().collect();
    cases.sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
    cases.truncate(SLOWEST);
    cases
}

/// Prints the ten slowest tests of `run` and the time it took, and fails over the bound.
fn verdict(run: &Run) -> Result<(), Failure> {
    println!("tests: the {SLOWEST} slowest tests");
    for case in slowest(run) {
        println!("{:>10.3} s  {} {}", case.seconds, case.suite, case.name);
    }
    println!(
        "tests: the run took {:.1} s of its bound of {BOUND_SECONDS:.0} s",
        run.seconds
    );
    if run.seconds > BOUND_SECONDS {
        return Err(Failure::OverBound(run.seconds));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{BOUND_SECONDS, Failure, parse, slowest, verdict};

    fn report(run: f64, cases: &[(f64, &str)]) -> String {
        use std::fmt::Write as _;
        let mut xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites name=\"nextest-run\" tests=\"{}\" failures=\"0\" errors=\"0\" uuid=\"u\" timestamp=\"t\" time=\"{run}\">\n<testsuite name=\"b10x-llm-gateway::main\" tests=\"{}\" disabled=\"0\" errors=\"0\" failures=\"0\">\n",
            cases.len(),
            cases.len()
        );
        for (seconds, name) in cases {
            write!(
                xml,
                "<testcase name=\"{name}\" classname=\"b10x-llm-gateway::main\" timestamp=\"t\" time=\"{seconds}\">\n</testcase>\n"
            )
            .unwrap();
        }
        xml.push_str("</testsuite>\n</testsuites>\n");
        xml
    }

    #[test]
    fn the_ten_slowest_come_slowest_first_and_no_more() {
        let cases: Vec<(f64, String)> = (0..12_u32)
            .map(|index| (f64::from(index) / 10.0, format!("module::case_{index}")))
            .collect();
        let borrowed: Vec<(f64, &str)> = cases
            .iter()
            .map(|(seconds, name)| (*seconds, name.as_str()))
            .collect();
        let run = parse(&report(3.5, &borrowed)).unwrap();
        assert!((run.seconds - 3.5).abs() < f64::EPSILON);
        let names: Vec<&str> = slowest(&run)
            .into_iter()
            .map(|case| case.name.as_str())
            .collect();
        let expected: Vec<String> = (2..12)
            .rev()
            .map(|index| format!("module::case_{index}"))
            .collect();
        assert_eq!(names, expected);
    }

    #[test]
    fn a_run_over_its_bound_fails_and_one_at_the_bound_passes() {
        let at = parse(&report(BOUND_SECONDS, &[(1.0, "a")])).unwrap();
        assert_eq!(verdict(&at), Ok(()));
        let over = parse(&report(BOUND_SECONDS + 0.1, &[(1.0, "a")])).unwrap();
        assert!(matches!(verdict(&over), Err(Failure::OverBound(_))));
    }

    #[test]
    fn an_escaped_name_reads_back_and_a_case_without_a_time_is_refused() {
        let run = parse(&report(1.0, &[(0.5, "a&lt;b&gt; &amp; &quot;c&quot;")])).unwrap();
        assert_eq!(run.cases[0].name, "a<b> & \"c\"");
        let broken = report(1.0, &[(0.5, "a")]).replace(" time=\"0.5\"", "");
        assert!(parse(&broken).is_err());
        assert!(parse("<testsuites name=\"x\">").is_err());
    }
}
