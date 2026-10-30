//! Security review of `story:credential-buffers-zeroed` (wave 2026-10-30-w06): what is left of a
//! model's vLLM key in this process's memory once the Runpod readiness probe has sent it.
//!
//! The story's outcome covers every buffer of the binary that holds a credential, and the
//! binary hands each model's vLLM key to `ConnectorsRunpod`, whose readiness probe writes it
//! into the request it sends the pod (`src/transport/probe.rs`, `models`). The key is handed
//! over in a vector no constant here holds; the probe runs against a loopback port nobody
//! listens on (the request is built before the connection is tried); the transport, and with it
//! the key's owner, is dropped; then every private writable mapping of the process (heap, thread
//! arenas, stacks) is read through `/proc/self/mem` and searched for the key.
//!
//! What a pass proves: at the time of the scan no memory of this process holds the key. A chunk
//! the allocator reused before the scan is overwritten either way, so a pass is evidence and not
//! proof. What a failure proves: a copy was left, and the scan names the mapping it is in.
//! `the_scan_sees_an_allocation_freed_holding_its_bytes` shows the scan finds a copy that was left.

use llm_runpod::{ConnectorsBinding, ConnectorsRunpod, Probe, ProbeTarget, RunpodTransport};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    net::TcpListener,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const SENTINEL_BYTES: usize = 40;
/// The needle is held masked, so the search holds no copy of what it searches for.
const MASK: u8 = 0x80;

/// A credential of 40 bytes of `[a-z0-9]` that no constant of this file holds, from a generator
/// seeded per run and per test: its only copies are the ones the code under test makes.
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

/// What this proves: the scan finds a copy of a credential that was freed without being
/// overwritten, inside a chunk and at a chunk's start, so that a scan that finds nothing is not a
/// scan that cannot see.
#[test]
fn the_scan_sees_an_allocation_freed_holding_its_bytes() {
    let inside = Sentinel::new(0x71);
    let mut left = Vec::with_capacity(700);
    left.extend_from_slice(&[b'-'; 300]);
    inside.append(&mut left);
    drop(left);
    assert!(
        !holders(inside).is_empty(),
        "the scan does not see a credential freed inside a chunk"
    );
    let started = Sentinel::new(0x72);
    let mut left = Vec::with_capacity(100);
    started.append(&mut left);
    left.extend_from_slice(b"\r\n");
    drop(left);
    assert!(
        !holders(started).is_empty(),
        "the scan does not see a credential freed at a chunk's start"
    );
}

/// What this proves: once the readiness probe has written a model's vLLM key into its request
/// and the transport holding the key is dropped, the key is nowhere in this process's writable
/// memory. `probe::models` builds the request in a `String` grown by `push_str` around the key
/// and ends it with `request.clear()`, which sets the length to zero and overwrites nothing.
#[test]
fn the_readiness_probe_leaves_the_vllm_key_nowhere_in_this_process_once_the_transport_is_gone() {
    let sentinel = Sentinel::new(0x81);
    let closed = TcpListener::bind("127.0.0.1:0").expect("loopback");
    let endpoint = format!("http://{}/v1/", closed.local_addr().expect("address"));
    drop(closed);
    let mut transport = ConnectorsRunpod::new(ConnectorsBinding {
        executable: Path::new(env!("CARGO_TARGET_TMPDIR")).join("security_w06_no_connectors"),
        adapter: "gpu".to_owned(),
        connection: "conn-1".to_owned(),
        work_directory: Path::new(env!("CARGO_TARGET_TMPDIR")).join("security_w06_probe"),
        timeout: Duration::from_secs(1),
    });
    let mut key = Vec::with_capacity(SENTINEL_BYTES);
    sentinel.append(&mut key);
    transport.set_vllm_key("qwen", key);
    let probed = transport.probe_ready(&ProbeTarget {
        pod_id: "abc123",
        model: Some("qwen"),
        endpoint: Some(&endpoint),
    });
    assert_eq!(probed, Probe::Unreachable, "nobody listens there");
    drop(transport);
    let left = holders(sentinel);
    assert!(
        left.is_empty(),
        "the vLLM key is still in this process's memory once the probe has sent it and the \
         transport is gone: {left:?}"
    );
}
