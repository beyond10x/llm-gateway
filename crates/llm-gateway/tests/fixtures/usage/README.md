# Usage fixtures

Authored from published sources, not captured: no vLLM server produced these bytes. Each file is
headed by the sources it was derived from. Until a live record against a server checks them,
they are the gateway's best reading of what `vllm/vllm-openai:v0.27.1`, started with
`--enable-prompt-tokens-details`, answers on the chat and messages wires, usage block included.

Format: every leading line that starts with `#` is the provenance header and is not part of the
fixture. The fixture starts at the first line that does not start with `#` and runs to the end
of the file, every byte of it. Each file is the body of an answer: an event stream
(`text/event-stream`) for a `*-stream.response.txt` file, else one JSON document
(`application/json`).

Every usage block carries `future_usage_field`, which is not vLLM's: no server defines it. It
stands for a usage field the gateway does not know, so a relay that parsed and re-wrote usage
would drop it.

| Answer | Wire | What its usage carries |
| --- | --- | --- |
| `chat.response.txt` | chat, not streamed | `prompt_tokens_details.cached_tokens`, `created_cache_tokens`, a null `multimodal_tokens` |
| `chat-stream.response.txt` | chat, streamed with `stream_options.include_usage` | the final chunk, with no choice, carries `usage` with `prompt_tokens_details.cached_tokens`; then `[DONE]` |
| `messages.response.txt` | messages, not streamed | `cache_read_input_tokens`, `cache_creation_input_tokens` |
| `messages-stream.response.txt` | messages, streamed | `message_start` usage without cache fields, as vLLM writes it from its first chunk; `message_delta` usage with `cache_read_input_tokens` and `cache_creation_input_tokens` |

Upstream sources, all read on 2026-10-15: vLLM v0.27.1 (tag `v0.27.1`, commit
`6e448d0ea9bf3d88d898b65449ca6dc2aec170ac`), under
<https://github.com/vllm-project/vllm/blob/v0.27.1/>: `vllm/entrypoints/openai/engine/protocol.py`,
`vllm/entrypoints/openai/chat_completion/protocol.py`, `vllm/entrypoints/openai/chat_completion/serving.py`,
`vllm/entrypoints/openai/chat_completion/api_router.py`, `vllm/entrypoints/anthropic/protocol.py`,
`vllm/entrypoints/anthropic/serving.py` and `vllm/entrypoints/anthropic/api_router.py`.
