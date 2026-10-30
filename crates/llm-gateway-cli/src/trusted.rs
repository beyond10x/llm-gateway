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

/// The refusal of `rule` for `policy`'s source. Its message is `detail` alone: no refusal quotes
/// the path it was given or the file a link resolves to, because a secret written by mistake as
/// a path would be printed and logged with it. The code names the source, and a caller that
/// knows the setting names it ([`read_named`]).
fn refuse(policy: &Policy, rule: FileRule, detail: &str) -> Refusal {
    Refusal::new(rule.refusal(policy.source), detail.to_owned())
}

/// Opens `target` without following a final symlink.
fn open_final(target: &Path, policy: &Policy) -> Result<File, Refusal> {
    // Non-blocking, so a FIFO cannot hold the open; it is then refused as not a regular file.
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
    let descriptor = open(target, flags, Mode::empty()).map_err(|errno| {
        if errno == Errno::LOOP {
            refuse(policy, FileRule::Symlink, "is a symbolic link")
        } else {
            refuse(policy, FileRule::Unreadable, &errno.to_string())
        }
    })?;
    Ok(File::from(descriptor))
}

/// Resolves `path` through every symbolic link and opens the file it names. The resolved file
/// must lie inside the directory `path` names, resolved too, as a projected volume's `..data`
/// links do; one outside it is refused as a symbolic link, naming neither file. A rotation can
/// remove the directory a resolution named before the open reaches it, so a resolved file that
/// has vanished is resolved once more.
fn open_resolved(path: &Path, policy: &Policy) -> Result<File, Refusal> {
    let unreadable =
        |error: std::io::Error| refuse(policy, FileRule::Unreadable, &error.to_string());
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
                "resolves outside the directory that holds it",
            ))
        }
    };
    match open_final(&resolve()?, policy) {
        Err(_) if fs::metadata(path).is_ok() => open_final(&resolve()?, policy),
        opened => opened,
    }
}

/// Reads `path` as UTF-8 text after checking the opened handle against `policy`. The text is
/// exactly the bytes read. A refusal's message is the rule's detail alone.
pub(crate) fn read(path: &Path, policy: &Policy) -> Result<String, Refusal> {
    let mut file = if policy.resolve_links {
        open_resolved(path, policy)?
    } else {
        open_final(path, policy)?
    };
    let metadata = file
        .metadata()
        .map_err(|error| refuse(policy, FileRule::Unreadable, &error.to_string()))?;
    if !metadata.file_type().is_file() {
        return Err(refuse(
            policy,
            FileRule::NotRegular,
            "is not a regular file",
        ));
    }
    let owner = metadata.uid();
    if owner != geteuid().as_raw() && owner != 0 {
        return Err(refuse(
            policy,
            FileRule::UntrustedOwner,
            "is owned by neither this user nor root",
        ));
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode & policy.unsafe_mode != 0 {
        return Err(refuse(
            policy,
            FileRule::UnsafeMode,
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
        .map_err(|error| refuse(policy, FileRule::Unreadable, &error.to_string()))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > policy.limit {
        return Err(too_large());
    }
    String::from_utf8(bytes).map_err(|_| refuse(policy, FileRule::NotUtf8, "is not UTF-8"))
}

/// [`read`] for a file a setting names: a refusal's message is `<setting>: <detail>`, naming
/// the setting and never the path.
pub(crate) fn read_named(path: &Path, setting: &str, policy: &Policy) -> Result<String, Refusal> {
    read(path, policy).map_err(|refusal| named(&refusal, setting))
}

/// `refusal` with `setting` in front of its message.
fn named(refusal: &Refusal, setting: &str) -> Refusal {
    Refusal::new(refusal.kind(), format!("{setting}: {}", refusal.message()))
}

/// Bytes that may hold a credential: what this reader read, a key read through it, the request
/// a probe sends one in. They are overwritten, spare capacity included, when dropped, so every
/// path that lets them go overwrites them first, a refusal's included. Best effort without a
/// `zeroize` dependency, as `OwnerToken` does: the black box keeps the writes observable.
pub(crate) struct Wiped(Vec<u8>);

impl Wiped {
    /// `size` zero bytes, allocated before anything is read into them.
    pub(crate) fn zeroed(size: usize) -> Self {
        let bytes = vec![0; size];
        #[cfg(test)]
        ledger::taken(&bytes);
        Self(bytes)
    }

    /// No bytes yet, in an allocation of exactly `capacity` bytes, made before any is written.
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        let bytes = Vec::with_capacity(capacity);
        #[cfg(test)]
        ledger::taken(&bytes);
        Self(bytes)
    }

    /// `bytes`, overwritten from now on when dropped.
    pub(crate) fn adopt(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub(crate) fn capacity(&self) -> usize {
        self.0.capacity()
    }

    /// Appends `bytes`; a caller that sized the allocation checks it did not move.
    pub(crate) fn extend(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    /// The first `length` bytes as text, in the same allocation; bytes that are not UTF-8 come
    /// back as they are, still overwritten when dropped.
    pub(crate) fn into_text(mut self, length: usize) -> Result<String, Self> {
        let mut bytes = std::mem::take(&mut self.0);
        bytes.truncate(length);
        String::from_utf8(bytes).map_err(|error| Self(error.into_bytes()))
    }
}

impl Drop for Wiped {
    fn drop(&mut self) {
        self.0.fill(0);
        self.0.resize(self.0.capacity(), 0);
        std::hint::black_box(&self.0);
        #[cfg(test)]
        ledger::released(&self.0);
    }
}

/// Each [`Wiped`] allocation this thread took and let go, so that a test can check every one was
/// overwritten before it was freed and none was freed behind its back.
#[cfg(test)]
pub(crate) mod ledger {
    use std::{cell::RefCell, collections::BTreeSet};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Entry {
        /// An allocation's address and size, as it was taken.
        Taken(usize, usize),
        /// An allocation's address and size as it was let go, and whether every byte read zero.
        Released(usize, usize, bool),
    }

    thread_local! {
        static ENTRIES: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
    }

    pub(crate) fn taken(bytes: &Vec<u8>) {
        if bytes.capacity() > 0 {
            let entry = Entry::Taken(bytes.as_ptr().addr(), bytes.capacity());
            ENTRIES.with_borrow_mut(|entries| entries.push(entry));
        }
    }

    pub(crate) fn released(bytes: &Vec<u8>) {
        if bytes.capacity() > 0 {
            let zeroed = bytes.len() == bytes.capacity() && bytes.iter().all(|byte| *byte == 0);
            let entry = Entry::Released(bytes.as_ptr().addr(), bytes.capacity(), zeroed);
            ENTRIES.with_borrow_mut(|entries| entries.push(entry));
        }
    }

    /// This thread's entries since the last call, oldest first.
    pub(crate) fn drain() -> Vec<Entry> {
        ENTRIES.with_borrow_mut(std::mem::take)
    }

    /// Every allocation taken since the last [`drain`] was let go once, read zero in full, and
    /// nothing was let go that was not taken, so none was freed by a reallocation behind the
    /// buffer's back; at least one was taken.
    pub(crate) fn assert_every_allocation_overwritten(case: &str) {
        let mut live = BTreeSet::new();
        let mut taken = 0;
        for entry in drain() {
            match entry {
                Entry::Taken(address, size) => {
                    assert!(live.insert((address, size)), "{case}: taken twice");
                    taken += 1;
                }
                Entry::Released(address, size, zeroed) => {
                    assert!(
                        live.remove(&(address, size)),
                        "{case}: let go an allocation that was never taken, so the one that was \
                         was freed by a reallocation without being overwritten"
                    );
                    assert!(zeroed, "{case}: an allocation was let go holding its bytes");
                }
            }
        }
        assert!(
            live.is_empty(),
            "{case}: {} allocation(s) were freed without being overwritten",
            live.len()
        );
        assert!(taken > 0, "{case}: the bytes were held in no allocation the ledger saw");
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CONTAINER_TOKEN, OWNER_SECRET, Policy,
        ledger::{self, Entry},
        read,
    };
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };

    /// `bytes` in a file of mode 0600, in a directory of its own under the build directory.
    fn file(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!("gateway-cli-unit-{name}-{}", std::process::id()));
        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("credential");
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }

    fn bound(policy: &Policy) -> usize {
        usize::try_from(policy.limit).unwrap() + 1
    }

    /// What this proves: the text a credential file is read into is the allocation the reader
    /// took at the bound, plus one byte to see a file past it, before its first read (the same
    /// address and size), and none was taken or let go beside it, so no smaller allocation held
    /// the credential and was freed by a reallocation. The reader's caller overwrites the text.
    #[test]
    fn a_credential_file_is_read_into_one_allocation_taken_at_the_bound_before_its_first_byte() {
        for (name, policy) in [("owner", &OWNER_SECRET), ("container", &CONTAINER_TOKEN)] {
            let mut sent = vec![b'k'; 3000];
            sent.push(b'\n');
            let path = file(name, &sent);
            ledger::drain();
            let text = read(&path, policy).unwrap();
            assert_eq!(text.as_bytes(), sent, "{name}");
            assert_eq!(
                ledger::drain(),
                [Entry::Taken(text.as_ptr().addr(), bound(policy))],
                "{name}"
            );
            assert_eq!(text.capacity(), bound(policy), "{name}");
        }
    }

    /// What this proves: a credential file refused after it was read was held in allocations
    /// that each read zero in full as they were let go. That freed memory stays zero is not
    /// observable from safe Rust.
    #[test]
    fn a_credential_file_refused_after_it_was_read_is_overwritten_first() {
        let path = file("not-utf8", b"sk-\xff-credential\n");
        ledger::drain();
        let refused = read(&path, &OWNER_SECRET).unwrap_err();
        assert_eq!(refused.code(), "owner-secret:not-utf8");
        ledger::assert_every_allocation_overwritten("not UTF-8");
    }
}
