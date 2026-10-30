//! Security review of `story:credential-buffers-zeroed` (wave 2026-10-30-w06): what is left of a
//! key file's credential in this process's memory once every reader and holder of it is gone.
//!
//! The unit's own tests prove, through a test-only ledger inside the crate, that the
//! trusted-file reader and `read_token` overwrite each allocation they let go. These check the
//! same outcome from outside the crate, through the process's own memory. Key files holding a
//! credential that no constant here holds are read through the public readers
//! (`vllm_keys`, `owner_verifier`, `metrics_bearer`), taken or refused; every holder is dropped;
//! then every private writable mapping of the process (heap, thread arenas, stacks) is read
//! through `/proc/self/mem` and searched for it.
//!
//! What a pass proves: at the time of the scan no memory of this process holds the credential.
//! A chunk the allocator reused before the scan is overwritten either way, so a pass is evidence
//! and not proof. What a failure proves: a copy was left, and the scan names the mapping it is in.
//! `the_scan_sees_an_allocation_freed_holding_its_bytes` shows the scan finds a copy that was left.

use llm_gateway_cli::{metrics_bearer, owner_verifier, parse, vllm_keys};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const SENTINEL_BYTES: usize = 40;
/// The needle is held masked, so the search holds no copy of what it searches for.
const MASK: u8 = 0x80;

/// A credential of 40 bytes of `[a-z0-9]` that no constant of this file holds, from a generator
/// seeded per run and per test: its only copies are the ones the code under test makes and the
/// ones this file makes and overwrites.
#[derive(Clone, Copy)]
struct Sentinel(u64);

impl Sentinel {
    fn new(salt: u64) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        let process = u64::from(std::process::id());
        Self(((u64::from(nanos) << 20) ^ (process << 40) ^ salt.wrapping_mul(0x9e37_79b9)) | 1)
    }

    fn bytes(self) -> impl Iterator<Item = u8> {
        const ALPHABET: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let mut state = self.0;
        (0..SENTINEL_BYTES).map(move |_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ALPHABET[usize::try_from(state % 36).unwrap_or(0)]
        })
    }

    /// Appends the credential to `out`, which has room for it.
    fn append(self, out: &mut Vec<u8>) {
        for byte in self.bytes() {
            out.push(byte);
        }
    }
}

/// Overwrites this file's own copy of a credential before it is freed, as the binary does.
fn wipe(mut bytes: Vec<u8>) {
    bytes.fill(0);
    std::hint::black_box(&bytes);
}

/// Every offset at which `needle`, held masked, starts in `haystack`.
fn matches(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    let mut found = Vec::new();
    if haystack.len() < needle.len() {
        return found;
    }
    for offset in 0..=haystack.len() - needle.len() {
        if haystack[offset..offset + needle.len()]
            .iter()
            .zip(needle)
            .all(|(byte, masked)| byte ^ MASK == *masked)
        {
            found.push(offset);
        }
    }
    found
}

/// Where the credential lies in this process's private writable memory, as `<mapping> +<offset>`,
/// outside the scan's own buffer. It searches for the credential's last 16 bytes: the allocator
/// writes its own bookkeeping, up to 32 bytes in a large free chunk, over the start of a chunk it
/// frees, so a copy that started a chunk is found by its end.
fn holders(sentinel: Sentinel) -> Vec<String> {
    const CHUNK: usize = 1 << 20;
    const NEEDLE_BYTES: usize = 16;
    let needle: Vec<u8> = sentinel
        .bytes()
        .skip(SENTINEL_BYTES - NEEDLE_BYTES)
        .map(|byte| byte ^ MASK)
        .collect();
    let maps = fs::read_to_string("/proc/self/maps").expect("/proc/self/maps");
    let mut memory = File::open("/proc/self/mem").expect("/proc/self/mem");
    let mut chunk = vec![0_u8; CHUNK];
    let own = chunk.as_ptr().addr()..chunk.as_ptr().addr() + CHUNK;
    let mut found = Vec::new();
    for line in maps.lines() {
        let fields: Vec<&str> = line.split_ascii_whitespace().collect();
        let (Some(range), Some(perms)) = (fields.first(), fields.get(1)) else {
            continue;
        };
        if !perms.starts_with("rw") {
            continue;
        }
        let name = fields.get(5).copied().unwrap_or("[anonymous]");
        let Some((start, end)) = range.split_once('-') else {
            continue;
        };
        let (Ok(start), Ok(end)) = (
            usize::from_str_radix(start, 16),
            usize::from_str_radix(end, 16),
        ) else {
            continue;
        };
        let mut at = start;
        while at < end {
            let wanted = (end - at).min(CHUNK);
            chunk.fill(0);
            let Ok(position) = u64::try_from(at) else {
                break;
            };
            let read = memory
                .seek(SeekFrom::Start(position))
                .and_then(|_| memory.read(&mut chunk[..wanted]));
            let Ok(read) = read else {
                break;
            };
            if read == 0 {
                break;
            }
            for offset in matches(&chunk[..read], &needle) {
                if !own.contains(&(at + offset)) {
                    found.push(format!("{name} +{:#x}", at + offset - start));
                }
            }
            if read < needle.len() || at + read >= end {
                break;
            }
            at += read - (needle.len() - 1);
        }
    }
    chunk.fill(0);
    std::hint::black_box(&chunk);
    found
}

/// A directory of this test's own under cargo's temporary target directory.
fn directory(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("security_w06_buffers")
        .join(format!("{name}-{}", std::process::id()));
    drop(fs::remove_dir_all(&dir));
    fs::create_dir_all(&dir).expect("directory");
    dir
}

/// `before`, the credential, then `after`, written to `path` with mode 0600; this file's copy is
/// overwritten once written.
fn key_file(path: &Path, sentinel: Sentinel, before: &[u8], after: &[u8]) {
    let mut bytes = Vec::with_capacity(before.len() + SENTINEL_BYTES + after.len());
    bytes.extend_from_slice(before);
    sentinel.append(&mut bytes);
    bytes.extend_from_slice(after);
    let written = fs::write(path, &bytes);
    wipe(bytes);
    written.expect("write");
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("mode");
}

fn deployment(dir: &Path) -> llm_gateway_cli::Deployment {
    parse(&format!(
        "listen = \"127.0.0.1:0\"\nmetrics_listen = \"127.0.0.1:9464\"\n\
         owner_secret_file = \"{dir}/owner\"\nmetrics_bearer_file = \"{dir}/metrics\"\n\n\
         [providers.runpod]\nkind = \"runpod-vllm\"\n\n[models.small]\nprovider = \"runpod\"\n\
         wires = [\"chat\"]\ncontext_window = 65536\nhf_model = \"example/small-model\"\n\
         image = \"vllm/vllm-openai:v0.27.1\"\ngpu_types = [\"NVIDIA L40S\"]\n\
         max_model_len = 1024\nvllm_api_key_file = \"{dir}/vllm\"\n",
        dir = dir.display()
    ))
    .expect("the document")
}

/// What this proves: the scan finds a copy of a credential that was freed without being
/// overwritten, inside a chunk and at a chunk's start, so that a scan that finds nothing is not a
/// scan that cannot see.
#[test]
fn the_scan_sees_an_allocation_freed_holding_its_bytes() {
    let inside = Sentinel::new(0x41);
    let mut left = Vec::with_capacity(700);
    left.extend_from_slice(&[b'-'; 300]);
    inside.append(&mut left);
    drop(left);
    assert!(
        !holders(inside).is_empty(),
        "the scan does not see a credential freed inside a chunk"
    );
    let started = Sentinel::new(0x42);
    let mut left = Vec::with_capacity(100);
    started.append(&mut left);
    left.extend_from_slice(b"\r\n");
    drop(left);
    assert!(
        !holders(started).is_empty(),
        "the scan does not see a credential freed at a chunk's start"
    );
}

/// What this proves: once the vLLM key, the owner secret and the metrics bearer have been read
/// from their files through the trusted-file reader, taken, and every holder dropped, the
/// credential is nowhere in this process's writable memory.
#[test]
fn a_key_file_taken_leaves_its_credential_nowhere_in_this_process_once_its_holders_are_gone() {
    let dir = directory("taken");
    let sentinel = Sentinel::new(0x51);
    for name in ["owner", "metrics", "vllm"] {
        key_file(&dir.join(name), sentinel, b"", b"\r\n");
    }
    let deployment = deployment(&dir);
    let keys = vllm_keys(&deployment).expect("the vLLM key");
    assert_eq!(keys.len(), 1);
    drop(keys);
    drop(owner_verifier(&deployment).expect("the owner secret"));
    drop(metrics_bearer(&deployment).expect("the metrics bearer"));
    let left = holders(sentinel);
    assert!(
        left.is_empty(),
        "a key file's credential is still in this process's memory once every holder is gone: \
         {left:?}"
    );
}

/// What this proves: a key file the rules refuse after it was read (not a token, not UTF-8, past
/// the token bound after trimming) leaves its credential nowhere in this process's writable
/// memory.
#[test]
fn a_key_file_refused_after_it_was_read_leaves_its_credential_nowhere_in_this_process() {
    let sentinel = Sentinel::new(0x61);
    let long = vec![b'k'; 4097 - SENTINEL_BYTES];
    for (case, after, code) in [
        ("not a token", &b" second\n"[..], "vllm-api-key:not-a-token"),
        ("not UTF-8", b"\xff\n", "vllm-api-key:not-utf8"),
        ("past the bound", &long[..], "vllm-api-key:too-large"),
    ] {
        let dir = directory(&format!("refused-{}", case.replace(' ', "-")));
        key_file(&dir.join("vllm"), sentinel, b"", after);
        let refusal = vllm_keys(&deployment(&dir)).expect_err(case);
        assert_eq!(refusal.code(), code, "{case}");
        let left = holders(sentinel);
        assert!(
            left.is_empty(),
            "{case}: a refused key file's credential is still in this process's memory: {left:?}"
        );
    }
}
