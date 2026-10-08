//! The same-handle, bounded trusted-file reader (rows K30 and B9; llmgw `src/trusted.rs:21-42`).
//!
//! The file is opened once without following a final symlink, and every check reads that handle:
//! a path swapped between a check and the read cannot substitute another file.

use crate::refusal::{FileRule, Refusal, Source};
use rustix::{
    fs::{Mode, OFlags, open},
    io::Errno,
    process::geteuid,
};
use std::{
    fs::File,
    io::Read,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

/// What one source admits.
pub(crate) struct Policy {
    pub(crate) source: Source,
    /// Size bound in bytes, inclusive.
    pub(crate) limit: u64,
    /// Permission bits that refuse the file when any is set.
    pub(crate) unsafe_mode: u32,
}

/// The deployment document: 256 KiB, refused when group- or world-writable (as llmgw).
pub(crate) const CONFIG: Policy = Policy {
    source: Source::Config,
    limit: 256 * 1024,
    unsafe_mode: 0o022,
};

/// The owner secret: a token of at most 4096 bytes plus the trailing CRLF or newline an editor
/// or a shell adds, so the file is at most 4098 bytes; the token bound itself is applied after
/// trailing whitespace is trimmed. Refused with any group or world permission, because it is a
/// credential.
pub(crate) const OWNER_SECRET: Policy = Policy {
    source: Source::OwnerSecret,
    limit: 4 * 1024 + 2,
    unsafe_mode: 0o077,
};

/// A model's vLLM key (row B9): a credential sent to the pod as a bearer, held to the owner
/// secret's rules.
pub(crate) const VLLM_API_KEY: Policy = Policy {
    source: Source::VllmApiKey,
    ..OWNER_SECRET
};

fn refuse(policy: &Policy, rule: FileRule, path: &Path, detail: &str) -> Refusal {
    Refusal::new(
        rule.refusal(policy.source),
        format!("{}: {detail}", path.display()),
    )
}

/// Reads `path` as UTF-8 text after checking the opened handle against `policy`. The text is
/// exactly the bytes read.
pub(crate) fn read(path: &Path, policy: &Policy) -> Result<String, Refusal> {
    // Non-blocking, so a FIFO cannot hold the open; it is then refused as not a regular file.
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
    let descriptor = open(path, flags, Mode::empty()).map_err(|errno| {
        if errno == Errno::LOOP {
            refuse(policy, FileRule::Symlink, path, "is a symbolic link")
        } else {
            refuse(policy, FileRule::Unreadable, path, &errno.to_string())
        }
    })?;
    let mut file = File::from(descriptor);
    let metadata = file
        .metadata()
        .map_err(|error| refuse(policy, FileRule::Unreadable, path, &error.to_string()))?;
    if !metadata.file_type().is_file() {
        return Err(refuse(
            policy,
            FileRule::NotRegular,
            path,
            "is not a regular file",
        ));
    }
    let owner = metadata.uid();
    if owner != geteuid().as_raw() && owner != 0 {
        return Err(refuse(
            policy,
            FileRule::UntrustedOwner,
            path,
            "is owned by neither this user nor root",
        ));
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode & policy.unsafe_mode != 0 {
        return Err(refuse(
            policy,
            FileRule::UnsafeMode,
            path,
            &format!(
                "has mode {mode:03o}; none of {:03o} may be set",
                policy.unsafe_mode
            ),
        ));
    }
    let too_large = || {
        refuse(
            policy,
            FileRule::TooLarge,
            path,
            &format!("exceeds {} bytes", policy.limit),
        )
    };
    if metadata.len() > policy.limit {
        return Err(too_large());
    }
    // The bound holds for the bytes actually read, so a file that grows after the check is
    // refused rather than read past the bound.
    let mut bytes = Vec::new();
    (&mut file)
        .take(policy.limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| refuse(policy, FileRule::Unreadable, path, &error.to_string()))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > policy.limit {
        return Err(too_large());
    }
    String::from_utf8(bytes).map_err(|_| refuse(policy, FileRule::NotUtf8, path, "is not UTF-8"))
}
