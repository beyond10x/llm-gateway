//! The specification diff gate.
//!
//! `spec/` in the working tree is compared with `spec/` at the merge base of a named revision and
//! `HEAD`, by `ess verify diff --fail-on breaking-or-unknown`. ESS classifies every change, binds
//! `contracts/diff-acknowledgements.json` to both endpoint digests and refuses a file naming any
//! other pair; this module only materialises the base, chooses whether the acknowledgements apply
//! and relays ESS's answer and exit status.

use std::{
    error::Error,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

const SPEC: &str = "spec";
const ACKNOWLEDGEMENTS: &str = "contracts/diff-acknowledgements.json";
const FAIL_ON: &str = "breaking-or-unknown";

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, Box<dyn Error>> {
    let output = Command::new("git").args(args).current_dir(root).output()?;
    if !output.status.success() {
        return Err(format!(
            "git {} refused: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(output.stdout)
}

fn text(bytes: Vec<u8>) -> Result<String, Box<dyn Error>> {
    Ok(String::from_utf8(bytes)?.trim().to_owned())
}

/// Writes every file under `spec/` at `commit` into `out`, keeping its path.
fn materialise(root: &Path, commit: &str, out: &Path) -> Result<(), Box<dyn Error>> {
    let listing = git(
        root,
        &["ls-tree", "-r", "-z", "--name-only", commit, "--", SPEC],
    )?;
    let mut count = 0;
    for path in listing
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let path = std::str::from_utf8(path)?;
        let target = out.join(path);
        fs::create_dir_all(target.parent().ok_or("a spec path without a parent")?)?;
        fs::write(
            &target,
            git(root, &["cat-file", "blob", &format!("{commit}:{path}")])?,
        )?;
        count += 1;
    }
    if count == 0 {
        return Err(format!("{commit} holds no {SPEC}/ to compare with").into());
    }
    Ok(())
}

/// The acknowledgements to hand ESS: the working tree's file, unless the base holds the same
/// bytes. Each acknowledgement is bound to the one comparison it shipped with, so a file inherited
/// unchanged from the base belongs to an earlier change and can only be refused here.
fn acknowledgements(root: &Path, commit: &str) -> Result<Option<PathBuf>, Box<dyn Error>> {
    let path = root.join(ACKNOWLEDGEMENTS);
    if !path.exists() {
        return Ok(None);
    }
    let inherited = Command::new("git")
        .args(["cat-file", "blob", &format!("{commit}:{ACKNOWLEDGEMENTS}")])
        .current_dir(root)
        .output()?;
    if inherited.status.success() && inherited.stdout == fs::read(&path)? {
        println!(
            "{ACKNOWLEDGEMENTS} is unchanged since {commit}: it acknowledges an earlier comparison and is not applied"
        );
        return Ok(None);
    }
    Ok(Some(path))
}

/// The change ids ESS names, one per `fails --fail-on` line.
fn failing(stderr: &str) -> Vec<&str> {
    let prefix = format!("fails --fail-on {FAIL_ON}: ");
    stderr
        .lines()
        .filter_map(|line| line.strip_prefix(prefix.as_str()))
        .map(str::trim)
        .collect()
}

pub fn run(base: &str) -> Result<ExitCode, Box<dyn Error>> {
    let root = PathBuf::from(text(git(
        Path::new("."),
        &["rev-parse", "--show-toplevel"],
    )?)?);
    let target = text(
        git(
            &root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{base}^{{commit}}"),
            ],
        )
        .map_err(|_| format!("base revision {base} does not resolve to a commit"))?,
    )?;
    let commit = text(git(&root, &["merge-base", &target, "HEAD"])?)?;
    let out = root
        .join("target/spec-diff")
        .join(format!("base-{}", std::process::id()));
    if out.exists() {
        fs::remove_dir_all(&out)?;
    }
    materialise(&root, &commit, &out)?;
    println!("comparing {SPEC}/ with {SPEC}/ at {commit} (merge base of {base} and HEAD)");
    let from = out.join(SPEC);
    let to = root.join(SPEC);
    let mut command = Command::new("ess");
    command
        .args(["verify", "diff", "--fail-on", FAIL_ON, "--from"])
        .arg(&from)
        .arg("--to")
        .arg(&to);
    if let Some(path) = acknowledgements(&root, &commit)? {
        command.arg("--acknowledgements").arg(path);
    }
    let output = command.current_dir(&root).output()?;
    std::io::stdout().write_all(&output.stdout)?;
    std::io::stderr().write_all(&output.stderr)?;
    fs::remove_dir_all(&out)?;
    let code = output
        .status
        .code()
        .ok_or("ess verify diff was stopped by a signal")?;
    if code == 4 {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!(
            "spec diff failed on: {}; acknowledge a reviewed change in {ACKNOWLEDGEMENTS}",
            failing(&stderr).join(", ")
        );
    }
    Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
}
