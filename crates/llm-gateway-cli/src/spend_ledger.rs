//! The spend ledger's file (story:spend-cap-counts-earlier-pods): where the pool of a provider
//! with a spend ceiling keeps what its pods cost (`llm_runpod::LedgerStore`), so a restarted
//! gateway counts the pods it ran before.
//!
//! The file is `spend_ledger_file` of the provider that declares the ceiling, or
//! `spend-ledger.json` in the gateway's state directory, beside the heartbeat
//! ([`crate::heartbeat::state_directory`]). It is read once, when the pool is composed, through
//! the trusted-file rules ([`crate::trusted::SPEND_LEDGER`]): opened without following a final
//! symbolic link, a regular file owned by this user or root, writable by neither group nor
//! others, at most [`MAX_LEDGER_BYTES`]. A missing file is an empty ledger. It is written whole,
//! to a temporary file beside it, mode 0600, renamed over it, its directory created mode 0700.
//! A read or a write that fails is logged once with its rule or its kind, never the path, and
//! the pool then refuses every start as `unknown`.

use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

use llm_runpod::{LedgerStore, LedgerUnavailable};

use crate::{heartbeat, trusted};

/// The ledger's file name in the state directory.
pub const LEDGER_FILE_NAME: &str = "spend-ledger.json";

/// The largest ledger file that is read.
pub const MAX_LEDGER_BYTES: u64 = trusted::SPEND_LEDGER.limit;

/// The default ledger file: `spend-ledger.json` in
/// [`heartbeat::state_directory`]; `None` when neither `XDG_STATE_HOME` nor `HOME` is an
/// absolute path.
pub fn default_path(xdg_state_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    heartbeat::state_directory(xdg_state_home, home)
        .map(|directory| directory.join(LEDGER_FILE_NAME))
}

/// The ledger kept in one file.
#[derive(Debug, Clone)]
pub struct FileLedger {
    path: PathBuf,
}

impl FileLedger {
    /// The ledger at `path`.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Where it is kept.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl LedgerStore for FileLedger {
    fn load(&mut self) -> Result<Option<String>, LedgerUnavailable> {
        match std::fs::symlink_metadata(&self.path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            _ => {}
        }
        trusted::checked(&self.path, &trusted::SPEND_LEDGER)
            .map(Some)
            .map_err(|broken| {
                tracing::warn!(
                    rule = broken.rule.code(),
                    "the spend ledger cannot be read; every pod start under a spend ceiling is refused as unknown"
                );
                LedgerUnavailable
            })
    }

    fn save(&mut self, text: &str) -> Result<(), LedgerUnavailable> {
        heartbeat::write_private(&self.path, text.as_bytes()).map_err(|error| {
            tracing::warn!(
                kind = ?error.kind(),
                "the spend ledger could not be written; every pod start under a spend ceiling is refused as unknown until a write succeeds"
            );
            LedgerUnavailable
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{FileLedger, default_path};
    use llm_runpod::LedgerStore as _;
    use std::{ffi::OsString, fs, os::unix::fs::PermissionsExt as _, path::PathBuf};

    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!("spend-ledger-{name}-{}", std::process::id()));
        drop(fs::remove_dir_all(&dir));
        dir
    }

    #[test]
    fn the_default_ledger_sits_beside_the_heartbeat() {
        let some = |text: &str| Some(OsString::from(text));
        assert_eq!(
            default_path(some("/s"), some("/h")),
            Some(PathBuf::from("/s/llm-gateway/spend-ledger.json"))
        );
        assert_eq!(
            default_path(some("relative"), some("/h")),
            Some(PathBuf::from(
                "/h/.local/state/llm-gateway/spend-ledger.json"
            ))
        );
        assert_eq!(default_path(None, some("relative")), None);
        assert_eq!(default_path(None, None), None);
    }

    #[test]
    fn a_missing_ledger_is_empty_and_a_written_one_reads_back() {
        let dir = scratch("roundtrip");
        let mut ledger = FileLedger::new(dir.join("state").join("spend-ledger.json"));
        assert_eq!(ledger.load(), Ok(None));
        ledger.save("{}\n").unwrap();
        assert_eq!(ledger.load(), Ok(Some("{}\n".to_owned())));
        let mode = fs::metadata(ledger.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(fs::read_dir(dir.join("state")).unwrap().count(), 1);
    }

    #[test]
    fn a_ledger_the_trusted_rules_refuse_is_unavailable() {
        let dir = scratch("refused");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("spend-ledger.json");
        fs::write(&path, "{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(
            FileLedger::new(path.clone()).load().is_err(),
            "world-writable"
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.join("link.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(FileLedger::new(link).load().is_err(), "a symbolic link");
        fs::write(&path, vec![b' '; 1 << 20 | 1]).unwrap();
        assert!(FileLedger::new(path).load().is_err(), "past the bound");
        assert!(
            FileLedger::new(dir.clone()).load().is_err(),
            "a directory is not a ledger"
        );
    }
}
