# Credential buffers zeroed, 2026-10-30

The record of `story:credential-buffers-zeroed` on `impl/credential-buffers-zeroed` (base
`f3c076f4`). It adds to the statement in [gateway.md](gateway.md), "Zeroisation of `OwnerToken` is
best effort … Not a guarantee against a reallocation that already copied the bytes", which stays
as written. That statement still holds for what it names. What this record adds: the buffers
below no longer make such a copy. Each is sized before a credential enters it, or moves it by an
explicit copy that overwrites the allocation it leaves. Each is overwritten, spare capacity
included, on every path that lets it go.

The overwrite stays best effort. Neither crate gains a `zeroize` dependency, and
`crates/llm-gateway` still declares none. Each overwrite is `fill(0)` over the length, then
`resize(capacity, 0)` over the spare capacity, then `std::hint::black_box`. No `unsafe`.

## Per buffer

| Buffer | Where | How it holds | Test or assertion | Level |
| --- | --- | --- | --- | --- |
| The client's request head | `crates/llm-gateway/src/server.rs`: `read_head`, `HeadBuffer`, `Head` | Each read writes into the buffer in place. To grow, it takes a new zeroed allocation (twice the size, at most the head bound), copies the bytes read and overwrites the old one before freeing it. Its `Drop` overwrites the last allocation. `read_head` returns the head inside its buffer as a `Head`, which derefs to `str`. | `head_tests::a_head_is_overwritten_in_every_allocation_it_used_when_it_is_answered`; `head_tests::a_refused_head_is_overwritten_in_every_allocation_it_used_on_every_path` | ledger |
| The trusted-file reader, which reads every credential file | `crates/llm-gateway-cli/src/trusted.rs`: `read`, `Wiped` | Reads in place into one allocation of the policy's bound plus one byte, taken before the first read. It never reallocates. A refusal after the read (too large, not UTF-8, unreadable) drops it as a `Wiped`, which overwrites it. Text it returns uses that same allocation, and what no read wrote stays zero. | `trusted::tests::a_credential_file_is_read_into_one_allocation_taken_at_the_bound_before_its_first_byte`; `trusted::tests::a_credential_file_refused_after_it_was_read_is_overwritten_first` | ledger |
| A key read from a file (vLLM keys, hosted endpoint credentials, route secrets, the metrics bearer) | `crates/llm-gateway-cli/src/keys.rs`: `read_token` | Takes the reader's text as a `Wiped`, so it is overwritten whichever way the rules go. Returns a copy of exactly the key's length, which the caller wraps. | `keys::tests::a_key_file_read_is_overwritten_whichever_way_the_rules_go` | ledger |
| The hosted probe's request | `crates/llm-gateway-cli/src/hosted.rs`: `probe_request`, `exchange` | Its exact length is computed before the key is written. A `debug_assert` checks the capacity did not move. As a `Wiped`, it is dropped right after the write, before the answer is read. | `hosted::tests::a_probe_request_is_sized_before_its_key_is_written_and_overwritten_when_dropped` | ledger and debug assertion |
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
- **Debug assertion.** The assertion holds in debug builds, which are the builds the tests and the
  integration tests' binary use. Release builds compile it out; the sizing that makes it hold is in
  both. Before the fix, it failed two prompt tests in `credential::tests`, the hosted probe test
  and 13 of the 17 `credential_set` cases.
- **Neither level proves:**
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

At the commit that adds this record these are unchanged. Each lies outside the files this story
owned:

- `crates/llm-gateway/src/auth.rs` `sha256`, which `OwnerToken::digest` calls: copies the token
  into a vector that grows when its padding is pushed. The allocation it leaves is freed holding
  the token, unzeroed; the vector it grows into is overwritten once hashed.
- `crates/llm-gateway-cli/src/serve.rs` `owner_verifier`: drops the owner secret file's text
  unzeroed.
- `crates/llm-gateway-cli/src/bedrock.rs` `TokenFile::resolve`: a refusal of the version string
  drops the token's text unzeroed before llm's `Secret` takes it.

## Residuals

- The standard-input buffer that `std` keeps for the whole process. It is never freed.
- rustls's plaintext and record buffers on the probe's and the relay's TLS connections.
- The SHA-256 state inside `hex_digest` when the Bedrock token's version is computed.
- The bytes `read_head` read past the head, which it returns as a separate vector. They hold no
  head bytes, but can hold the start of a request body.
- The reader's allocation is now as large as its bound for every read: 2 MiB plus one byte for
  the routes document and 256 KiB plus one for the deployment document, once at start.
