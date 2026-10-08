# Gateway specification declarations, 2026-10-08

The re-run of the `ess:hardening` design review (technique 8) for `story:gateway-spec-declarations`,
on `impl/gateway-spec-declarations` (base `c63f07e`), after the story declared rows 4-38 of
[spec-hardening-gateway-review.md](spec-hardening-gateway-review.md). Line numbers refer to the
commit that adds this record. `ess` is 0.56.0; the system is on `format: ess/23`, the format whose
enum attributes let `llm-gateway.gateway.Refusal` carry each code's status and message.

## Inputs

| Side | Files |
| --- | --- |
| design | `docs/gateway.md` (`D`), README "The deployment document" under "Run the gateway" (`R`) |
| mapping | none |
| specification | `spec/domains/gateway.yaml` (`G`), `spec/domains/deployment.yaml` (`P`), validated first |

The review was run by the implementing agent, not by a separate one, so it knew the plant. The
plant shows the procedure reaches the declaration; it does not show an independent reader would.

## The plant

On a copy of `spec/` under the unit's scratch directory, the challenge-header rule was deleted
from `G:201`: `every \`401\` carries \`www-authenticate: Bearer\`, and every \`405\` an \`allow\``
became `every \`405\` carries an \`allow\``. `ess specify validate --path <copy>/spec` printed
`llm-gateway v1 — 8 file(s), valid`. The review of the copy returned it as finding 1.

## Findings

| # | classification | design (file:line, quoted) | spec (file:line, quoted) | note |
|---|---|---|---|---|
| 1 | missing | D:127-128 "Every `401` carries `www-authenticate: Bearer`" | G:200-201 "Per exchange, `www-authenticate=<value> allow=<value>`" | The plant, caught. The real spec declares it at G:201. |
| 2 | contradicts | R:146-147 "takes at most twice the 10-second read timeout however slowly a client sends or reads" | P:21-23 "at most twice the read timeout for inspection and the probes, and not bounded that way for a relay"; G:139-150 | The spec follows D:309-317 and the code. R is wrong for the wire paths: see the measurement below. Open: R or the code, for the owner to decide. |

Both directions were read: every behaviour statement in `D` and `R` was matched to a declaration,
then every declaration in `G` and `P` to a design statement. The second direction found two
statements `D` did not make: the code of a head that never completes (`request-malformed`, G
`Exchange` step 1) and the case-insensitive bearer scheme (G `Exchange` step 4). Both are true,
pinned by scenarios `a-head-that-never-completes-is-refused-when-the-read-times-out` and
`the-bearer-scheme-is-matched-without-regard-to-case`, and `D:124-127` now states them; they
are not in the table because the review was run after that change.

Counts on the copy: 1 missing (the plant), 1 contradicts, 0 stale mapping, 0 spec-only, 0
unclear. Without the plant, the one `contradicts` finding is the only one, and its spec side is
declared with an `ESS-LIMIT:` note.

Not reached: `D` "Counters and usage records", which `spec/domains/telemetry.yaml` declares, a
third domain outside this review; `D` "Verification"; `R` above "The deployment document" (the
quick start, for example its target identifiers `small.chat` and `small.responses`).

## The stop bound for a wire path

`R:146-147` says a stop "takes at most twice the 10-second read timeout however slowly a client
sends or reads". A throwaway test, kept in the scratch directory and never committed, composed
the gateway with a relay whose target source answers every request `Unavailable`, as the binary
does for a model whose provider declares no `connectors`, with `read_timeout` set to one second.
The client sent its head in two halves 800 ms apart, its body in two halves 800 ms apart, then
kept sending without reading; the stop was called 100 ms after the connection opened.

```text
read_timeout=1s stop took 2.497881068s (2.50 x read_timeout), since connect 2.610889218s, report ShutdownReport { accepted: 1, completed: 1, in_flight_at_signal: 1 }, answer Some("HTTP/1.1 503 Service Unavailable")
```

The head, the body, the answer and the read-out of a refused relay each have their own deadline
of one `read_timeout` (D:309-317, `src/server.rs` `serve`, `src/relay.rs` `serve`), so one
request can hold a stop for more than twice it. With the binary's ten-second default that is
over 20 seconds. The code agrees with `D`; this story changes no behaviour, so `R` is left as
written.

## The tests that pin the declarations

`crates/llm-gateway/tests/spec_declarations.rs` holds the rules no scenario can observe, one case
per review row. `row_4_*` and `row_25_*` read `G` and were red before it declared them:

```text
test row_25_every_gateway_config_default_is_declared_with_its_value ... FAILED
test row_4_every_refusal_the_crate_emits_is_declared_with_its_code_status_and_message ... FAILED
test result: FAILED. 8 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.90s
```

The others drive the gateway and were green on arrival, because the behaviour already existed.
Each was shown to fail on a planted defect, restored afterwards:

| Case | Planted defect | Result |
| --- | --- | --- |
| `row_14_*` | the body deadline set to 100 times `read_timeout` | failed |
| `row_15_*` | the interim `100 Continue` never written | failed |
| `row_19_*` | a refused relay closed without reading out the body | failed |
| `row_23_a_client_that_stops_reading_*` | each chunk's write deadline set to 40 times `read_timeout` | failed |
| `row_24_*` | the handle's `Drop` returning before it stops the listener | failed |
| `row_4_*` | `overloaded` declared with status 429 | failed |
| `row_25_*` | `max_concurrent_requests` declared as defaulting to 65 | failed |
| `k29_a_schema_refusal_names_its_line_*` | the `line <n>:` prefix moved to the end | failed |
| scenario `inspection-every-answer-is-json-no-store-and-closes-after-one-request` | `connection: close` dropped from the gateway's own answers | 194 of 195 passed |

`row_17_*`, `row_23_a_stop_waits_*`, `row_12_*` and `k30_a_relative_owner_secret_file_*` were
not planted. The first version of `row_19_*` survived its plant, because its client read while it
was still sending; it was rewritten to read only after sending, and the rewritten case was
killed.
