//! Security review of `story:credential-buffers-zeroed` (wave 2026-10-30-w06): what is left of a
//! presented credential in this process's memory once the gateway has answered it.
//!
//! The unit's own tests prove, through a test-only ledger inside the crate, that every
//! allocation `read_head` took read zero as it was let go. These check the same outcome from
//! outside the crate, through the process's own memory. A credential that no constant here
//! holds is presented over loopback; every holder is dropped and the gateway stopped; then every
//! private writable mapping of the process (heap, thread arenas, stacks) is read through
//! `/proc/self/mem` and searched for it.
//!
//! What a pass proves: at the time of the scan no memory of this process holds the credential.
//! A chunk the allocator reused before the scan is overwritten either way, so a pass is evidence
//! and not proof. What a failure proves: a copy was left, and the scan names the mapping it is in.
//! `the_scan_sees_an_allocation_freed_holding_its_bytes` shows the scan finds a copy that was left.

use llm_gateway::{Gateway, GatewayConfig, OwnerToken, RouteInventory, SharedSecretVerifier};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    net::{Shutdown, SocketAddr, TcpStream},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
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

/// Overwrites this file's own copy of a credential before it is freed, as the gateway does.
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

/// `parts` in one allocation of exactly their length, the credential written where `None` stands.
fn request(parts: &[Option<&[u8]>], sentinel: Sentinel) -> Vec<u8> {
    let length = parts
        .iter()
        .map(|part| part.map_or(SENTINEL_BYTES, <[u8]>::len))
        .sum();
    let mut bytes = Vec::with_capacity(length);
    for part in parts {
        match part {
            Some(text) => bytes.extend_from_slice(text),
            None => sentinel.append(&mut bytes),
        }
    }
    assert_eq!(bytes.capacity(), length, "the request moved");
    bytes
}

/// One connection: `request` written whole and overwritten, the write side closed when `close`
/// says so, then the answer read to its end.
fn exchange(addr: SocketAddr, request: Vec<u8>, close: bool) -> String {
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let written = stream.write_all(&request);
    wipe(request);
    written.expect("write");
    if close {
        drop(stream.shutdown(Shutdown::Write));
    }
    let mut answer = Vec::new();
    drop(stream.read_to_end(&mut answer));
    String::from_utf8_lossy(&answer).into_owned()
}

fn status(answer: &str) -> &str {
    answer.get(9..12).unwrap_or("")
}

fn owner_token(sentinel: Sentinel) -> OwnerToken {
    let mut material = Vec::with_capacity(SENTINEL_BYTES);
    sentinel.append(&mut material);
    OwnerToken::new(material).expect("a token")
}

fn gateway(owner: Sentinel) -> llm_gateway::GatewayHandle {
    let verifier = Arc::new(SharedSecretVerifier::new(owner_token(owner)).expect("a verifier"));
    let mut config = GatewayConfig::new("127.0.0.1:0".parse().expect("loopback"));
    config.read_timeout = Duration::from_secs(5);
    config.max_head_bytes = 8192;
    let handle = Gateway::bind(
        config,
        verifier,
        RouteInventory::new(Vec::new()).expect("empty"),
    )
    .expect("bind");
    handle.mark_ready();
    handle
}

/// What this proves: the scan finds a copy of a credential that was freed without being
/// overwritten, inside a chunk and at a chunk's start, so that a scan that finds nothing is not a
/// scan that cannot see.
#[test]
fn the_scan_sees_an_allocation_freed_holding_its_bytes() {
    let inside = Sentinel::new(0x11);
    let mut left = Vec::with_capacity(700);
    left.extend_from_slice(&[b'-'; 300]);
    inside.append(&mut left);
    drop(left);
    assert!(
        !holders(inside).is_empty(),
        "the scan does not see a credential freed inside a chunk"
    );
    let started = Sentinel::new(0x12);
    let mut left = Vec::with_capacity(100);
    started.append(&mut left);
    left.extend_from_slice(b"\r\n");
    drop(left);
    assert!(
        !holders(started).is_empty(),
        "the scan does not see a credential freed at a chunk's start"
    );
}

/// What this proves: once the gateway has answered requests carrying the owner's credential
/// (accepted, on the metrics listener, in a head that grew past one read, refused past the bound,
/// refused for a stray line byte, and cut off by the peer) and a credential it rejected, and has
/// stopped, neither credential is anywhere in this process's writable memory.
#[test]
fn a_presented_credential_is_nowhere_in_this_process_once_the_gateway_has_answered() {
    let owner = Sentinel::new(0x21);
    let rejected = Sentinel::new(0x22);
    let mut handle = gateway(owner);
    let metrics = handle
        .serve_metrics(
            "127.0.0.1:0".parse().expect("loopback"),
            Some(owner_token(owner)),
        )
        .expect("metrics");
    let addr = handle.local_addr();
    let start: &[u8] = b"GET /v1/routes HTTP/1.1\r\nhost: 127.0.0.1\r\nauthorization: Bearer ";
    let end: &[u8] = b"\r\n\r\n";
    let pad: &[u8] = b"\r\nx-pad: ";
    let grown = [pad, &[b'a'; 3000][..], end].concat();
    // Exactly the bound and no terminator, so the gateway reads every byte before it refuses
    // and the client reads its answer rather than a reset.
    let oversized = [
        pad,
        &vec![b'a'; 8192 - start.len() - SENTINEL_BYTES - pad.len()][..],
    ]
    .concat();
    let answered = [
        (
            "accepted",
            addr,
            request(&[Some(start), None, Some(end)], owner),
            false,
            "200",
        ),
        (
            "rejected",
            addr,
            request(&[Some(start), None, Some(end)], rejected),
            false,
            "401",
        ),
        (
            "grown past one read",
            addr,
            request(&[Some(start), None, Some(&grown[..])], owner),
            false,
            "200",
        ),
        (
            "past the head bound",
            addr,
            request(&[Some(start), None, Some(&oversized[..])], owner),
            false,
            "431",
        ),
        (
            "a stray line byte",
            addr,
            request(&[Some(start), None, Some(b"\r\nx-note: a\nb")], owner),
            false,
            "400",
        ),
        (
            "the peer stops",
            addr,
            request(&[Some(start), None, Some(b"\r\nx-more: a")], owner),
            true,
            "400",
        ),
        (
            "a scrape",
            metrics,
            request(
                &[
                    Some(b"GET /metrics HTTP/1.1\r\nhost: 127.0.0.1\r\nauthorization: Bearer "),
                    None,
                    Some(end),
                ],
                owner,
            ),
            false,
            "200",
        ),
    ];
    for (case, to, bytes, close, expected) in answered {
        let answer = exchange(to, bytes, close);
        assert_eq!(status(&answer), expected, "{case}: {answer}");
    }
    let _report = handle.shutdown();
    for (which, sentinel) in [("the owner's", owner), ("a rejected", rejected)] {
        let left = holders(sentinel);
        assert!(
            left.is_empty(),
            "{which} credential is still in this process's memory after every request was \
             answered and the gateway stopped: {left:?}"
        );
    }
}

/// What this proves: a client that sends its next request before the first is answered (HTTP/1.1
/// pipelining, each request carrying the owner's credential) leaves no copy of the credential in
/// this process once the gateway has answered and stopped. `read_head` returns the bytes past the
/// first head as a vector of their own (`server.rs`, `buffer.0[end + 4..filled].to_vec()`), which
/// `serve` drops as it is; here those bytes are the second request's head.
#[test]
fn a_pipelined_requests_credential_is_nowhere_in_this_process_once_the_gateway_has_answered() {
    let owner = Sentinel::new(0x31);
    let handle = gateway(owner);
    let addr = handle.local_addr();
    let start: &[u8] = b"GET /v1/routes HTTP/1.1\r\nhost: 127.0.0.1\r\nauthorization: Bearer ";
    let between: &[u8] =
        b"\r\n\r\nGET /v1/routes HTTP/1.1\r\nhost: 127.0.0.1\r\nauthorization: Bearer ";
    let end: &[u8] = b"\r\n\r\n";
    for round in 0..4 {
        let answer = exchange(
            addr,
            request(&[Some(start), None, Some(between), None, Some(end)], owner),
            false,
        );
        assert_eq!(status(&answer), "200", "round {round}: {answer}");
    }
    let _report = handle.shutdown();
    let left = holders(owner);
    assert!(
        left.is_empty(),
        "a pipelined request's credential is still in this process's memory after the gateway \
         answered and stopped: {left:?}"
    );
}
