//! The same-handle, bounded trusted-file reader (rows K30 and B9; llmgw `src/trusted.rs:21-42`),
//! for the document, the routes document it names, the owner secret, each vLLM key, each
//! endpoint credential and the metrics bearer.
//!
//! The file is opened once without following a final symlink, and every check reads that handle:
//! a path swapped between a check and the read cannot substitute another file. The one exception
//! is the container token, which an orchestrator mounts behind symbolic links: its path is
//! resolved first, must stay inside the path's directory, and the file it resolves to is opened
//! and checked the same way.

use crate::refusal::{FileRule, Refusal, Source};
use rustix::{
    fs::{Mode, OFlags, open},
    io::Errno,
    process::geteuid,
};
use std::{
    fs::{self, File},
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
    /// Whether the path is resolved through its symbolic links before the file it names is
    /// opened. Every other rule holds for the resolved file.
    pub(crate) resolve_links: bool,
}

/// The deployment document: 256 KiB, refused when group- or world-writable (as llmgw).
pub(crate) const CONFIG: Policy = Policy {
    source: Source::Config,
    limit: 256 * 1024,
    unsafe_mode: 0o022,
    resolve_links: false,
};

/// The routes document a deployment names (story:routes-document): a `config` file in every rule
/// but its bound, which is llm's own `MAX_CONFIG_BYTES`, 2 MiB, the bound `Routes::parse` applies
/// (`routes::tests::the_routes_bound_is_the_bound_llm_parses_to`).
pub(crate) const ROUTES: Policy = Policy {
    limit: 2 * 1024 * 1024,
    ..CONFIG
};

/// The owner secret: a token of at most 4096 bytes plus the trailing CRLF or newline an editor
/// or a shell adds, so the file is at most 4098 bytes; the token bound itself is applied after
/// trailing whitespace is trimmed. Refused with any group or world permission, because it is a
/// credential.
pub(crate) const OWNER_SECRET: Policy = Policy {
    source: Source::OwnerSecret,
    limit: 4 * 1024 + 2,
    unsafe_mode: 0o077,
    resolve_links: false,
};

/// A model's vLLM key (row B9): a credential sent to the pod as a bearer, held to the owner
/// secret's rules.
pub(crate) const VLLM_API_KEY: Policy = Policy {
    source: Source::VllmApiKey,
    ..OWNER_SECRET
};

/// A hosted endpoint's credential: sent upstream as a bearer or an API key, held to the owner
/// secret's rules.
pub(crate) const ENDPOINT_CREDENTIAL: Policy = Policy {
    source: Source::EndpointCredential,
    ..OWNER_SECRET
};

/// The token a scrape of the metrics listener presents (`metrics_bearer_file`), held to the
/// owner secret's rules.
pub(crate) const METRICS_BEARER: Policy = Policy {
    source: Source::MetricsBearer,
    ..OWNER_SECRET
};

/// The container authorization token a Bedrock model's credentials are fetched with
/// (`AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE`): a token of at most 16 KiB, llm-bedrock's bound,
/// plus a trailing CRLF. A token the orchestrator mounts may be readable by others, so only a
/// group- or world-writable file is refused: a writable token file lets another user choose the
/// identity the gateway signs as.
///
/// Its path is resolved through symbolic links first. A Kubernetes projected volume, as EKS Pod
/// Identity mounts the token, presents the file as `<name> -> ..data/<name>` with `..data` a
/// link to a timestamped directory, which the kubelet replaces on rotation (layout from the
/// AWS and Kubernetes documentation, unverified here against a cluster). Each read resolves the
/// links again, so a rotated token is read without a restart. The resolved file must stay inside
/// the token path's directory, as that layout does.
pub(crate) const CONTAINER_TOKEN: Policy = Policy {
    source: Source::ContainerToken,
    limit: 16 * 1024 + 2,
    unsafe_mode: 0o022,
    resolve_links: true,
};

fn refuse(policy: &Policy, rule: FileRule, path: &Path, detail: &str) -> Refusal {
    Refusal::new(
        rule.refusal(policy.source),
        format!("{}: {detail}", path.display()),
    )
}

/// Opens `target` without following a final symlink; a refusal names `path`.
fn open_final(path: &Path, target: &Path, policy: &Policy) -> Result<File, Refusal> {
    // Non-blocking, so a FIFO cannot hold the open; it is then refused as not a regular file.
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
    let descriptor = open(target, flags, Mode::empty()).map_err(|errno| {
        if errno == Errno::LOOP {
            refuse(policy, FileRule::Symlink, path, "is a symbolic link")
        } else {
            refuse(policy, FileRule::Unreadable, path, &errno.to_string())
        }
    })?;
    Ok(File::from(descriptor))
}

/// Resolves `path` through every symbolic link and opens the file it names. The resolved file
/// must lie inside the directory `path` names, resolved too, as a projected volume's `..data`
/// links do; one outside it is refused as a symbolic link, naming the file it resolves to. A
/// rotation can remove the directory a resolution named before the open reaches it, so a
/// resolved file that has vanished is resolved once more.
fn open_resolved(path: &Path, policy: &Policy) -> Result<File, Refusal> {
    let unreadable =
        |error: std::io::Error| refuse(policy, FileRule::Unreadable, path, &error.to_string());
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let resolve = || {
        let directory = fs::canonicalize(parent).map_err(unreadable)?;
        let resolved = fs::canonicalize(path).map_err(unreadable)?;
        if resolved.starts_with(&directory) {
            Ok(resolved)
        } else {
            Err(refuse(
                policy,
                FileRule::Symlink,
                path,
                &format!(
                    "resolves to {}, outside {}",
                    resolved.display(),
                    directory.display()
                ),
            ))
        }
    };
    match open_final(path, &resolve()?, policy) {
        Err(_) if fs::metadata(path).is_ok() => open_final(path, &resolve()?, policy),
        opened => opened,
    }
}

/// Reads `path` as UTF-8 text after checking the opened handle against `policy`. The text is
/// exactly the bytes read.
pub(crate) fn read(path: &Path, policy: &Policy) -> Result<String, Refusal> {
    let mut file = if policy.resolve_links {
        open_resolved(path, policy)?
    } else {
        open_final(path, path, policy)?
    };
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
