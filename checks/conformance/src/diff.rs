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
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode, Output},
};

const SPEC: &str = "spec";
const ACKNOWLEDGEMENTS: &str = "contracts/diff-acknowledgements.json";
const FAIL_ON: &str = "breaking-or-unknown";

/// Runs an external program, naming it when it cannot be started. Every program this module
/// starts goes through here, so a missing `git` or `ess` is never reported as a bare OS error.
/// Only a program that was not found is pointed at `PATH`; any other start error (one that cannot
/// be executed, say) is the OS's own words.
fn spawn(program: &str, command: &mut Command) -> Result<Output, Box<dyn Error>> {
    command.output().map_err(|error| {
        if error.kind() == ErrorKind::NotFound {
            format!("cannot run `{program}`: {error}; is it on PATH?").into()
        } else {
            format!("cannot run `{program}`: {error}").into()
        }
    })
}

/// The reason a program that refused printed, or, when it printed nothing, its exit status, so a
/// refusal never ends without one.
fn reason(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        format!("{} and printed nothing", output.status)
    } else {
        stderr.to_owned()
    }
}

/// Removes the base copies of runs that are no longer alive: a run killed while `ess` compared
/// never reached its `BaseCopy` drop. A copy whose pid is alive belongs to a concurrent run and is
/// kept. Liveness is read from `/proc`, so where there is none nothing is removed.
fn remove_dead_copies(dir: &Path) {
    if !Path::new("/proc/self").exists() {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        let Some(pid) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("base-"))
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        if !Path::new("/proc").join(pid.to_string()).exists()
            && let Err(error) = fs::remove_dir_all(&path)
        {
            eprintln!("could not remove {}: {error}", path.display());
        }
    }
}

/// The base copy under `target/spec-diff/`, removed on every exit from `run`: a pass, a failure,
/// an error returned by `?` or a panic.
struct BaseCopy(PathBuf);

impl Drop for BaseCopy {
    fn drop(&mut self) {
        if self.0.exists()
            && let Err(error) = fs::remove_dir_all(&self.0)
        {
            eprintln!("could not remove {}: {error}", self.0.display());
        }
    }
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, Box<dyn Error>> {
    let output = spawn("git", Command::new("git").args(args).current_dir(root))?;
    if !output.status.success() {
        return Err(format!("git {} refused: {}", args.join(" "), reason(&output)).into());
    }
    Ok(output.stdout)
}

/// The merge base of `target` (resolved from `base`) and `HEAD`. `git merge-base` exits 1 and
/// prints nothing when the two share no history; that is said in words.
fn merge_base(root: &Path, base: &str, target: &str) -> Result<String, Box<dyn Error>> {
    let output = spawn(
        "git",
        Command::new("git")
            .args(["merge-base", target, "HEAD"])
            .current_dir(root),
    )?;
    if !output.status.success() {
        let why = if output.status.code() == Some(1) && output.stderr.trim_ascii().is_empty() {
            format!(
                "{base} and HEAD share no history (an unrelated branch, or a shallow clone without their common ancestor)"
            )
        } else {
            reason(&output)
        };
        return Err(format!("git merge-base {target} HEAD refused: {why}").into());
    }
    text(output.stdout)
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
    let inherited = spawn(
        "git",
        Command::new("git")
            .args(["cat-file", "blob", &format!("{commit}:{ACKNOWLEDGEMENTS}")])
            .current_dir(root),
    )?;
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
    let commit = merge_base(&root, base, &target)?;
    let copies = root.join("target/spec-diff");
    remove_dead_copies(&copies);
    let out = copies.join(format!("base-{}", std::process::id()));
    if out.exists() {
        fs::remove_dir_all(&out)?;
    }
    let base_copy = BaseCopy(out.clone());
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
    let output = spawn("ess", command.current_dir(&root))?;
    drop(base_copy);
    std::io::stdout().write_all(&output.stdout)?;
    std::io::stderr().write_all(&output.stderr)?;
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
