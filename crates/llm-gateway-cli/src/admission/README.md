# Recorded first turns the binary admits clients by

`LlmTranslator::admits` (`../translate.rs`) embeds these two requests with `include_str!` and
prepares each for every model a client's wire does not relay, once, when the relay is composed.
`GET /` profiles Codex or Claude Code for such a model only when llm carries the client's turn
to it (`docs/gateway.md`, "The public routes"). The tests that feed the same turns to the
translator read them from here.

| file | client | provenance |
| --- | --- | --- |
| `codex/first-turn.json` | Codex 0.160.0, responses wire | captured and sanitized in llm `0.7.0`, git blob `88ba7fc46331464b0372ac9e7bae20d97d0da063`; documented with its second turn in `../../tests/fixtures/codex/README.md` |
| `claude-code/1-tool-turn.request.txt` | Claude Code 2.1.293, messages wire | authored from published sources, not captured; its `#` lines are its provenance; documented with the other Claude Code requests in `../../../llm-gateway/tests/fixtures/claude-code/README.md` |

Neither is edited by hand. A newer client's turn replaces its file here, and the tests that read
it move with it.
