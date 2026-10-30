# Captured Codex requests

Copied byte for byte from llm's `crates/llm-responses/tests/fixtures/codex/` at tag `0.7.0`
(https://github.com/beyond10x/llm/tree/0.7.0/crates/llm-responses/tests/fixtures/codex), where
they are documented. `codex-cli` 0.160.0 sent them on 2026-10-16 to a loopback recorder standing
in for a Responses endpoint; no provider was reached, no paid call was made and no credential was
involved. llm sanitized them (every identifier replaced by `00000000-0000-7000-8000-00000000000n`,
long text shortened and marked `[shortened]`, the working directory replaced by `/workspace`) and
does not edit them by hand; neither does this repository.

| file | git blob at llm `0.7.0` | what it is |
| --- | --- | --- |
| `../../../src/admission/codex/first-turn.json` | `88ba7fc46331464b0372ac9e7bae20d97d0da063` | the opening request of a session: instructions, a developer message of two parts, the environment and the question |
| `second-turn.json` | `05b812027708af1f2a4e98f3639a247ce5678169` | the request after a scripted tool call: the same opening, the replayed reasoning item, the call and its plain-string output |

Codex was configured with the model alias `code`, `web_search = "disabled"`, reasoning effort
`high` and reasoning summary `auto`. The first turn lives with the binary, which embeds it to
decide `GET /`'s profiles (`src/admission/`). `crates/llm-gateway-cli/tests/translation.rs` feeds both to
the gateway's translator.
