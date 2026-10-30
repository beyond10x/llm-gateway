# Credential buffers zeroed, 2026-10-30

The record of `story:credential-buffers-zeroed` on `impl/credential-buffers-zeroed` (base
`f3c076f4`). It adds to the statement in [gateway.md](gateway.md), "Zeroisation of `OwnerToken` is
best effort … Not a guarantee against a reallocation that already copied the bytes", which stays
as written. That statement still holds for what it names. What this record adds: the buffers
below no longer make such a copy. Each is sized before a credential enters it, or moves it by an
explicit copy that overwrites the allocation it leaves. Each is overwritten, spare capacity
included, on every path that lets it go.

The overwrite stays best effort. No crate gains a `zeroize` dependency, and
`crates/llm-gateway` still declares none. Each overwrite is `fill(0)` over the length, then
`resize(capacity, 0)` over the spare capacity, then `std::hint::black_box`. No `unsafe`.

## Per buffer

| Buffer | Where | How it holds | Test or assertion | Level |
| --- | --- | --- | --- | --- |
| The client's request head | `crates/llm-gateway/src/server.rs`: `read_head`, `HeadBuffer`, `Head` | Each read writes into the buffer in place. To grow, it takes a new zeroed allocation (twice the size, at most the head bound), copies the bytes read and overwrites the old one before freeing it. Its `Drop` overwrites the last allocation. `read_head` returns the head inside its buffer as a `Head`, which derefs to `str`. | `head_tests::a_head_is_overwritten_in_every_allocation_it_used_when_it_is_answered`; `head_tests::a_refused_head_is_overwritten_in_every_allocation_it_used_on_every_path`; `head_tests::a_head_never_moves_into_an_allocation_past_its_bound` | ledger |
| The bytes read past the head | `crates/llm-gateway/src/server.rs`: `read_head`, `Leftover` | `read_head` copies what arrived after the head into a vector of exactly that length and returns it as a `Leftover`, which overwrites it when dropped. From a pipelining client these bytes are the next request's head, its credential included; otherwise they are the start of a body. A request the gateway answers itself drops the `Leftover`, overwritten. A relayed request hands the same vector to the relay unchanged (see "Not changed by this story"). | `head_tests::the_bytes_past_a_head_are_overwritten_when_they_are_let_go`; `a_pipelined_requests_credential_is_nowhere_in_this_process_once_the_gateway_has_answered` in `crates/llm-gateway/tests/security_w06_head_buffer.rs` | ledger and process-memory scan |
| Owner-token hashing: the owner token and every presented bearer | `crates/llm-gateway/src/auth.rs`: `sha256`, which `OwnerToken::digest` calls | The padded message is allocated at its padded length before the token enters it, so the padding never moves it. A `debug_assert` checks its length and capacity are as sized. It is overwritten once hashed, as before. | `auth::tests::the_digest_is_sha_256`, through its part `a_message_is_hashed_in_one_allocation_whatever_its_length`, which hashes every length from 0 to 130 bytes | debug assertion |
| The owner secret file's text | `crates/llm-gateway-cli/src/serve.rs`: `owner_verifier` | Takes the reader's text as a `Wiped`, so it is overwritten whichever way the rules go. The token is an exact-length copy, which `OwnerToken` overwrites. | `serve::tests::the_owner_secret_files_text_is_overwritten_whichever_way_the_rules_go` | ledger |
| The Bedrock container token, read again for each fetch | `crates/llm-gateway-cli/src/bedrock.rs`: `TokenFile::resolve` | Reads through the trusted reader. The text moves into llm's `Secret`, a `Zeroizing` vector, before its version is computed, so a refused version drops it overwritten. | None. No test reaches a refused version, because a 64-character hex digest is never refused, and the overwrite is llm's `zeroize`. | none |
| The trusted-file reader, which reads every credential file | `crates/llm-gateway-cli/src/trusted.rs`: `read`, `Wiped` | Reads in place into one allocation of the policy's bound plus one byte, taken before the first read. It never reallocates. A refusal after the read (too large, not UTF-8, unreadable) drops it as a `Wiped`, which overwrites it. Text it returns uses that same allocation, and what no read wrote stays zero. | `trusted::tests::a_credential_file_is_read_into_one_allocation_taken_at_the_bound_before_its_first_byte`; `trusted::tests::a_credential_file_refused_after_it_was_read_is_overwritten_first` | ledger |
| A key read from a file (vLLM keys, hosted endpoint credentials, route secrets, the metrics bearer) | `crates/llm-gateway-cli/src/keys.rs`: `read_token` | Takes the reader's text as a `Wiped`, so it is overwritten whichever way the rules go. Returns a copy of exactly the key's length, which the caller wraps. | `keys::tests::a_key_file_read_is_overwritten_whichever_way_the_rules_go` | ledger |
| The hosted probe's request | `crates/llm-gateway-cli/src/hosted.rs`: `probe_request`, `exchange` | Its exact length is computed before the key is written. A `debug_assert` checks the capacity did not move. As a `Wiped`, it is dropped right after the write, before the answer is read. | `hosted::tests::a_probe_request_is_sized_before_its_key_is_written_and_overwritten_when_dropped` | ledger and debug assertion |
| The Runpod readiness probe's request | `crates/llm-runpod/src/transport/probe.rs`: `models`, `Request` | Written into one allocation of exactly its length, taken before the key is written, as the hosted probe's is. A `debug_assert` checks the capacity did not move. As a `Request`, it is overwritten when dropped, as soon as the exchange returns, before the answer is parsed. Before, it was a `String` grown by `push_str` around the key and ended with `clear()`, which overwrote nothing. | `the_readiness_probe_leaves_the_vllm_key_nowhere_in_this_process_once_the_transport_is_gone` in `crates/llm-runpod/tests/security_w06_probe_buffer.rs` | process-memory scan and debug assertion |
| `credential set`'s standard input | `crates/llm-gateway-cli/src/credential.rs`: `read_stdin` | Reads in place into one allocation of the key bound plus two bytes, taken before the first byte. Its length stays its capacity, so the `Key` overwrites every byte when it is dropped. A `debug_assert` checks that neither moved. | The 17 cases of `crates/llm-gateway-cli/tests/credential_set.rs`, which drive the debug binary through it | debug assertion |
| `credential set`'s prompt | `crates/llm-gateway-cli/src/credential.rs`: `read_hidden_line` | Takes one allocation of the key bound plus one byte before the first byte. A `debug_assert` after each typed byte checks it did not move. Backspace and Ctrl-U already overwrite the bytes they remove. | `credential::tests::a_typed_key_never_outgrows_the_allocation_taken_before_its_first_byte` | debug assertion |

## What each level proves

- **Ledger.** Test builds only (`#[cfg(test)]`) keep a thread-local ledger. It records each
  allocation's address and size when the buffer takes it and when the buffer lets it go, and
  whether every byte read zero at release, spare capacity included. A test passes only if:
  - every allocation that held the bytes was taken by the buffer itself, at most the bound where
    there is one;
  - each was let go exactly once and read zero in full when it was;
  - no address was let go that was never taken, which is what a hidden reallocation looks like;
  - at least one allocation was taken.

  The head cases cover:
  - an answered head that grows at least once, with an interrupted read first;
  - an answered head under a bound of 3000 bytes, which doubling from 1024 does not reach
    exactly, so its last move must take the bound itself;
  - a stray line byte;
  - a head past the bound;
  - a head that is not UTF-8;
  - a peer that stops;
  - a peer that resets;
  - a deadline that passes.

  The trusted-reader and key cases cover:
  - an accepted key;
  - a key that is not a token;
  - a blank key;
  - a key past 4096 bytes;
  - a file that is not UTF-8.

  The owner secret cases cover a secret that is taken, one under the owner secret's floor, and
  one that is not a token.
- **Debug assertion.** The assertion holds in debug builds, which are the builds the tests and the
  integration tests' binary use. Release builds compile it out; the sizing that makes it hold is in
  both. Before the fix, it failed two prompt tests in `credential::tests`, the hosted probe test
  and 13 of the 17 `credential_set` cases. The hashing and Runpod probe assertions came with
  their fixes, so they have no run against the code before.
- **Process-memory scan.** The security review's integration tests drop every holder of a
  credential that no constant holds, then read every private writable mapping of the process
  (heap, thread arenas, stacks) through `/proc/self/mem` and search it for the credential's last
  16 bytes. Each file's `the_scan_sees_an_allocation_freed_holding_its_bytes` shows the scan
  finds a copy freed without an overwrite. A pass is evidence and not proof: the allocator may
  have reused a chunk before the scan. Before the fix, the pipelined-request case and the Runpod
  probe case failed.
- **No level proves:**
  - that freed memory stays zero, which safe Rust cannot observe;
  - that the compiler keeps the writes, because `black_box` is a hint and not a guarantee;
  - that no copy exists outside the buffer: stack temporaries, registers, or the kernel's socket
    and page buffers.

## Sites read and left as they are

- `crates/llm-gateway-cli/src/routes.rs` `read_credentials`: reads through `read_token` into a
  `TargetBearer`.
- `crates/llm-gateway-cli/src/tls.rs`: reads no file and holds no key buffer.
- `crates/llm-gateway-cli/src/bedrock.rs` `check_token`: reads through the trusted reader. Its
  `fill(0)` covers every byte a read wrote, and the spare capacity was never written.
- `crates/llm-gateway-cli/src/credential.rs`: the endpoint passes an exact-length copy into a
  `TargetBearer`, and the write wraps what it reads back in a `Key`.
- The deployment and routes documents: they name credential files and hold no credential.
- Exact-length copies (`to_vec()`) into a `TargetBearer`, the translator's `TargetSecret` or a
  Runpod `BearerKey`: their owners overwrite them on drop, and nothing in this path grows them.

## Not changed by this story

These still hold a credential's bytes, and this story leaves them as they are:

- The standard-input buffer that `std` keeps for the whole process. It is never freed.
- rustls's plaintext and record buffers on the probe's and the relay's TLS connections.
- The SHA-256 state inside `hex_digest` when the Bedrock token's version is computed.
- The bytes read past the head of a request the gateway relays. `serve` hands the `Leftover`'s
  vector unchanged to the relay's `Buffered` reader (`crates/llm-gateway/src/relay.rs`), which
  drains and grows it as it reads the body and frees it without overwriting it. They are the
  start of the body and, from a pipelining client, can be the next request's head with its
  credential. The relay is outside this story's files.
- The copy llm makes when it prepares a provider's credential: llm-providers `src/auth.rs:95`
  (llm 0.19.0, which this workspace pins) builds the header with `HeaderValue::from_bytes`, which
  copies the credential into memory llm does not overwrite. That code is in another repository,
  so this story cannot fix it here.
- The SHA-256 message schedule that owner-token hashing keeps on the stack
  (`crates/llm-gateway/src/auth.rs:189`). Its first 16 words are each block's bytes read as
  big-endian words, the token's among them, and nothing overwrites the array when the function
  returns.

## Cost

The reader's allocation is now as large as its bound for every read: 2 MiB plus one byte for the
routes document and 256 KiB plus one for the deployment document, once at start.
