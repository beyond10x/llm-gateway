use sha2::{Digest, Sha256};
use std::fmt::Write;
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn ess(args: &[&str]) -> Result<(), Box<dyn Error>> {
    if !Command::new("ess").args(args).status()?.success() {
        return Err("ESS projection refused; see diagnostics above".into());
    }
    Ok(())
}

fn files(root: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>, Box<dyn Error>> {
    fn visit(
        root: &Path,
        path: &Path,
        out: &mut BTreeMap<PathBuf, Vec<u8>>,
    ) -> Result<(), Box<dyn Error>> {
        for entry in fs::read_dir(path)? {
            let path = entry?.path();
            if path.is_dir() {
                visit(root, &path, out)?;
            } else {
                out.insert(path.strip_prefix(root)?.to_owned(), fs::read(path)?);
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out)?;
    Ok(out)
}

fn identity() -> Result<String, Box<dyn Error>> {
    let mut inputs = BTreeMap::new();
    for root in ["crates", "checks"] {
        for (path, bytes) in files(Path::new(root))? {
            if path
                .extension()
                .is_some_and(|ext| ext == "rs" || ext == "toml")
            {
                inputs.insert(Path::new(root).join(path), bytes);
            }
        }
    }
    for file in ["Cargo.toml", "Cargo.lock"] {
        inputs.insert(PathBuf::from(file), fs::read(file)?);
    }
    let mut hash = Sha256::new();
    for (path, bytes) in inputs {
        hash.update(path.to_str().ok_or("non-UTF8 source path")?.as_bytes());
        hash.update([0]);
        hash.update(u64::try_from(bytes.len())?.to_be_bytes());
        hash.update(bytes);
    }
    let mut identity = String::from("sources-sha256:");
    for byte in hash.finalize() {
        write!(identity, "{byte:02x}")?;
    }
    Ok(identity)
}

/// Every authored scenario on disk must be named in the inputs manifest ESS reads.
///
/// `--scenarios contracts` selects what `contracts/ess-inputs.yaml` lists, not what the tree
/// holds. A scenario file that exists but is unlisted is never selected, and the suite that
/// skipped it still exits 0 — so a whole domain's evidence can be absent and indistinguishable
/// from green. This refuses that state instead of leaving it to be noticed.
fn every_authored_scenario_is_declared() -> Result<(), Box<dyn Error>> {
    let manifest = fs::read_to_string("contracts/ess-inputs.yaml")?;
    let declared: BTreeSet<&str> = manifest
        .lines()
        .filter_map(|line| line.strip_prefix("- "))
        .map(str::trim)
        .collect();
    let mut missing = Vec::new();
    for path in files(Path::new("contracts"))?.keys() {
        if path
            .extension()
            .is_some_and(|extension| extension == "yaml")
            && path
                .components()
                .any(|part| part.as_os_str() == "scenarios")
            && !path.to_str().is_some_and(|name| declared.contains(name))
        {
            missing.push(path.display().to_string());
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "authored scenarios absent from contracts/ess-inputs.yaml: {}",
            missing.join(", ")
        )
        .into());
    }
    Ok(())
}

pub fn check() -> Result<(), Box<dyn Error>> {
    // Require the repository root; never infer success from a suite beside another checkout.
    if fs::canonicalize(".")?
        != fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))?
    {
        return Err("run conformance from this build's repository root".into());
    }
    every_authored_scenario_is_declared()?;
    // Unique projection directory prevents stale generated files from masking drift.
    let projection = format!("target/conformance/projection-{}", std::process::id());
    fs::create_dir_all(&projection)?;
    let suite = format!("{projection}/suite.json");
    ess(&[
        "verify",
        "conform",
        "synthesize",
        "--path",
        "spec",
        "--suite-format",
        "5",
        "--target",
        "ir",
        "--scenarios",
        "contracts",
        "--out",
        &suite,
    ])?;
    if fs::read(&suite)? != fs::read("contracts/suite.json")? {
        return Err("suite drift: regenerate contracts/suite.json with ESS".into());
    }
    let schemas = format!("{projection}/schema");
    ess(&[
        "generate", "--path", "spec", "--kind", "schema", "--out", &schemas,
    ])?;
    // `.ess-output` is ESS's destination-specific ownership ledger, not a projected artifact.
    if files(&Path::new(&schemas).join("schema"))? != files(Path::new("contracts/schema/schema"))? {
        return Err("schema drift: regenerate contracts/schema with ESS".into());
    }
    let identity = identity()?;
    let mut previous = None;
    for iteration in 1..=3 {
        let output = PathBuf::from(format!("target/conformance/run-{iteration}"));
        crate::execute(&suite, "contracts/baseline.json", &output, &identity)?;
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(output.join("report.json"))?)?;
        let counts = report["counts"].clone();
        if previous.as_ref().is_some_and(|old| old != &counts) {
            return Err("conformance counts changed across identical consecutive runs".into());
        }
        previous = Some(counts);
    }
    println!("serving conformance and deterministic ESS projections passed ({identity})");
    Ok(())
}
