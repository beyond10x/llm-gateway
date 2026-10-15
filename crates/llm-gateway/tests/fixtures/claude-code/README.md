# Claude Code messages-wire fixtures

Authored from published sources, not captured: no Claude Code session and no vLLM server
produced these bytes. Each file is headed by the sources it was derived from. A live record
against a pod checks them later; until then they are the gateway's best reading of what
Claude Code 2.1.293 sends and what `vllm/vllm-openai:v0.27.1` answers.

Format: every leading line that starts with `#` is the provenance header and is not part of the
fixture. The fixture starts at the first line that does not start with `#` and runs to the end
of the file, every byte of it.

- A `*.request.txt` file is an HTTP/1.1 request as Claude Code sends it, with LF line ends: the
  request line, the header lines, an empty line, then the body. The test writes the head with
  CRLF line ends and adds `host`, the owner's `authorization` and a `content-length` of its own;
  the body is sent byte for byte, its final newline included.
- A `*.response.txt` file is the body a vLLM v0.27.1 pod answers that request with: an event
  stream (`text/event-stream`) when the request has `"stream": true`, else one JSON document
  (`application/json`).

| Request | Answer | What it carries |
| --- | --- | --- |
| `1-tool-turn.request.txt` | `1-tool-turn.response.txt` | `?beta=true`, `anthropic-version`, `anthropic-beta`, adaptive `thinking`, `output_config.effort` `max`, a tool and `cache_control`; answered by a thinking block, a `signature_delta` and a `tool_use` block ending in `stop_reason` `tool_use` |
| `2-tool-result.request.txt` | `2-tool-result.response.txt` | the follow-up carrying the `tool_result`; answered by a thinking block and a text block ending in `end_turn` |
| `3-structured-output.request.txt` | `3-structured-output.response.txt` | `output_config.format` with a JSON schema beside effort `medium`, not streamed; answered by one JSON message whose text is the schema's JSON |
| `2-tool-result.request.txt`, as the gateway counts it at `POST /v1/messages/count_tokens` | `4-count-tokens.response.txt` | the count vLLM answers the gateway's output clamp with: `{"input_tokens":n}` |

Upstream sources, all read on 2026-10-15:

- Claude Code's gateway protocol: <https://code.claude.com/docs/en/llm-gateway-protocol>.
- vLLM v0.27.1 (tag `v0.27.1`, commit `6e448d0ea9bf3d88d898b65449ca6dc2aec170ac`), under
  <https://github.com/vllm-project/vllm/blob/v0.27.1/>: `vllm/entrypoints/anthropic/protocol.py`,
  `vllm/entrypoints/anthropic/serving.py` and `vllm/entrypoints/anthropic/api_router.py`.
