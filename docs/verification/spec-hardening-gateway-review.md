# Design review: gateway, 2026-10-06

This is the findings table of the `ess:hardening` design review (technique 8), recorded in
[spec-hardening.md](spec-hardening.md). One agent compared `docs/gateway.md` (`D`) and the
README's deployment section (`R`) with `spec/domains/gateway.yaml` (`G`) and
`spec/domains/deployment.yaml` (`P`). Every line number refers to llm-gateway `a11ef83`. The
reviewed specification was a copy with one planted deletion: `RequestTooLarge` was removed from
`llm-gateway.gateway.Refusal`.

The rows are quoted as the reviewer returned them. The status column is this record's own:

- `plant`: the planted deletion, caught.
- `fixed`: corrected in the hardening change.
- `open`: owned by `story:gateway-spec-declarations`.

| # | status | classification | design (file:line, quoted) | spec (file:line, quoted) | note |
|---|---|---|---|---|---|
| 1 | fixed | contradicts | R:104 "(a trailing newline or CRLF is trimmed)" | P:50 "Owner secret rules, after trailing ASCII whitespace is trimmed:" | The design trims one LF or CRLF. The spec trims all trailing whitespace, so a secret ending in "··\n\n" is refused by the design and accepted by the spec. Fixed: the code trims all trailing ASCII whitespace (`crates/llm-gateway-cli/src/serve.rs:50`), and README now says so. |
| 2 | fixed | contradicts | R:104-105 "readable by the owner only" | P:38-39 "`owner-secret`: any group or world permission (mode & 0o077)" | The design forbids group or world read (0o044); the spec also refuses write and execute bits. Fixed: the code uses `0o077` (`crates/llm-gateway-cli/src/trusted.rs:42`), and README now says so. |
| 3 | plant | missing | D:196 "`request-too-large` \| 431 \| the request head exceeds its byte bound" | G:49 `variants: [BodyIncomplete, … WireNotServed]` | The planted deletion. G:130 still uses "(`request-too-large`)". |
| 4 | open | missing | D:181-199, the status and message of the 11 codes that are not relay codes; D:173-174 "It quotes nothing the caller sent" | G:22-23 "with the wire code, status and fixed message the crate publishes" | Only the relay's eight codes (G:25-43) carry a status and message. |
| 5 | open | missing | D:100-111, probe match before shedding, shedding before authentication; "an unauthenticated `POST` to an unknown path is refused as `credential-absent`" | G:117-118 "come first, exactly as for inspection" | The relay points at an inspection order that the `Exchange` command (G:190-201) never declares. |
| 6 | open | missing | D:92-95, the `/health`, `/ready` and `/v1/routes[/{alias}]` rows; `GET`/`HEAD`; `{"status":"live"}`; `503 {"status":"unready"}` "before `mark_ready` and while draining"; R:107-108 | G:4 "a closed two-path liveness and readiness surface"; G:97-98 "`<status> <probe status>`" | Paths, methods, `HEAD` and probe bodies are not declared. |
| 7 | open | missing | D:86-88 "`cache-control: no-store` and `connection: close` … `application/json` … one request per connection" | G:102-103 "The challenge and method headers of every exchange" | `no-store` is declared for relayed answers only (G:155). |
| 8 | open | missing | D:111-112 "A query string is discarded; this milestone decodes no request parameter." | G:116-118 | |
| 9 | open | missing | D:63-69, the target summary fields; "no field for an endpoint URL…"; "exactly the identifier bytes the composer supplied" | G:100-101 "`bodies` … exactly as the gateway wrote it" | No inventory or target-summary shape is declared. |
| 10 | open | missing | D:78-82 "never becomes `0` … renders no digest, rather than an empty string" | G:100-101 | |
| 11 | open | missing | D:11-12 "Nothing in the crate's build resolves a credential, opens a connection to an upstream…"; D:25 "never given a credential *source*" | G:3-8 | The `ESS-LIMIT:` pattern at G:17-20 is not used for it. |
| 12 | open | missing | D:43-44 "`OwnerToken` is redacted in `Debug` … bytes are overwritten on drop" | G:104-106 `secret_on_wire` | Covers the wire only. |
| 13 | open | missing | D:132-133 "`content-length` or `transfer-encoding: chunked`, nothing else (`request-malformed`)" | G:124-131 | |
| 14 | open | missing | D:137-138 "The whole body is read under one `read_timeout` deadline" | G:27-28 "stalled past the read timeout" | A per-read timeout would satisfy the spec. |
| 15 | open | missing | D:139 "`expect: 100-continue` is answered for HTTP/1.1." | G:124-131 | |
| 16 | open | missing | D:151-152 "with `host` set to the target's authority" | G:147-148 "with a `content-length`" | |
| 17 | open | missing | D:153-154 "a request that cannot be written, an answer head that never arrives or is malformed" | G:149-150 "cannot be connected to, closes without an answer, or answers 502, 503 or 504" | |
| 18 | open | missing | D:158-160 "as `transfer-encoding: chunked` … an answer cut short on either side ends without its last chunk" | G:154-158 | Only the HTTP/1.0 negative is declared. |
| 19 | open | missing | D:167-169 "A refused relay closes gracefully … read and discarded (up to `request-body-bytes`, within `read_timeout`)" | G:116 "The relay, in the order the gateway decides" | |
| 20 | open | missing | D:217-218 "`begin_drain` reports unready while continuing to serve … still succeed" | G:85-88 "a drain is one way for the life of the handle" | |
| 21 | open | missing | D:222-224 "`completed` equals its `accepted` … `in_flight_at_signal`"; D:228-229 "`completed` counts every accepted connection, including ones refused on their own merits" | G:109-110; P:160-161 "as `accepted=<n> completed=<n>`" | |
| 22 | open | missing | D:229-231 "bounded by twice `read_timeout`"; R:117-118 "at most twice the 10-second read timeout" | G:84 `read_timeout_ms`; P:17-18 | |
| 23 | open | missing | D:232-235 "each chunk written to the client has its own … A stop waits for a relayed stream to end." | G:84, G:130-131 | |
| 24 | open | missing | D:235-236 "Dropping the handle without a shutdown still stops the listener, but reports nothing." | G:107-110 | |
| 25 | open | missing | D:252-253 defaults 8192 and 64; D:275 "It defaults to ten seconds" | G:82-84 | |
| 26 | open | missing | D:251 "`digest-characters` \| … \| 64", an exact length (D:268) | G:66 "`inventory:malformed-digest`" | |
| 27 | open | missing | R:98-99 "naming the line and the keys that are allowed" | P:47-48 `config:schema` | |
| 28 | open | missing | R:99-100 "A relative `owner_secret_file` is resolved from the working directory." | P:137-139, P:146 | |
| 29 | open | spec-only | D:139-140 (only CR/LF/NUL are stated) | G:56-57 "whitespace before its colon is `request-malformed`" | The comment also sits inside the `CompositionRefusal` block. |
| 30 | open | spec-only | D:57-65 state no inventory composition rules | G:64-66 `inventory:route-without-target`, `non-contiguous-positions`, `duplicate-target`, `duplicate-alias` | |
| 31 | open | spec-only | D:117 "`Gateway::bind_with_relay`…" says nothing about wire paths without a relay | G:121-122 "A gateway composed without a relay keeps its inspection-only answers." | |
| 32 | open | spec-only | D:131 "`unavailable` before `mark_ready`"; D:225-227 inspection served until the last accepted connection finishes | G:123 "Before `mark_ready` and once stopped, a relay is `unavailable`" | |
| 33 | open | spec-only | D:171-201 name no challenge header | G:102 "The challenge and method headers" | |
| 34 | open | spec-only | R:111-115, three events and no CLI rule | P:9-10, the closed command line; P:18 "A command-line error is clap's (exit status 2)." | |
| 35 | open | spec-only | R:102 "Both files go through a trusted-file reader." | P:31-32 "opened once without following a final symlink"; P:42-44 "a file that grows after the check is refused" | |
| 36 | open | spec-only | R:114 gives only the `<source>:<rule>` format | P:45 `not-utf8`; P:56-57 `listen:bind`, `signal:install` | |
| 37 | open | spec-only | R:100 delegates keys and ranges only | P:139 "at least one model is required" | Weak; R:100 may cover it. |
| 38 | open | unclear | R:108-109 "[docs/gateway.md](docs/gateway.md) has the full surface and every refusal code." | P:22 "Nothing in the binary provisions a pod or relays a model call yet" | Whether the binary serves the three wire paths. |

Counts: 38 rows. Without the plant: 2 contradicts (fixed), 25 missing, 9 spec-only, 1 unclear.
Outside the table, the reviewer found `G:14` pointing at "the credential scenarios below", but the
directory has no scenarios. That pointer now names `contracts/gateway/scenarios/`.
