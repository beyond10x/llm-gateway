# Two Runpod routes: fast and strong

One deployment document block with two Runpod models beside a local model: `inkling-pod`, a
small fast model for bulk work, and `qwen`, a stronger model for harder tasks. Neither starts on a
request (`start_on_request` is not set): a call to a stopped pod is refused `model-stopped`, and
the owner starts a pod with `POST /v1/models/{alias}/wake`. Nothing here has been run on a pod;
every speed below is a figure published by someone else, cited where it is given.

`crates/llm-gateway-cli/tests/profiles.rs` loads the block through the binary's own loader, so
this file cannot hold a block the binary refuses.

## The block

```toml
[providers.runpod]
kind = "runpod-vllm"
daily_cap_usd = 3.33

[models.inkling-pod]
provider = "runpod"
wires = ["chat", "responses", "messages"]
tool_calling = "parsed"
context_window = 65536
hf_model = "thinkingmachines/Inkling-2-Mini"
image = "vllm/vllm-openai:inkling-2-mini"
gpu_types = ["NVIDIA RTX PRO 6000 Blackwell Server Edition", "NVIDIA H100 PCIe"]
max_model_len = 65536
max_num_seqs = 8
gpu_util = 0.85
disk_gb = 60
thinking = "on"
extra_vllm_args = [
  "--tool-call-parser", "inkling", "--enable-auto-tool-choice",
  "--reasoning-parser", "inkling",
  "--speculative-config", '{"method":"dspark","num_speculative_tokens":7,"draft_sample_method":"probabilistic"}',
]
idle_timeout_minutes = 10
start_wait_seconds = 900
price_per_hour_usd = 2.89

[models.qwen]
provider = "runpod"
wires = ["chat", "responses", "messages"]
tool_calling = "parsed"
context_window = 65536
hf_model = "Qwen/Qwen3.8-27B-FP8"
image = "vllm/vllm-openai:v0.27.1"
disk_gb = 80
thinking = "on"
idle_timeout_minutes = 10
start_wait_seconds = 900
price_per_hour_usd = 3.19

[[models.qwen.profiles]]
name = "h100-64k"
gpu_types = ["NVIDIA H100 NVL", "NVIDIA H100 PCIe"]
max_model_len = 65536
max_num_seqs = 64
extra_vllm_args = [
  "--language-model-only", "--max-num-batched-tokens", "1536",
  "--kv-cache-dtype", "fp8", "--mamba-ssm-cache-dtype", "float16",
  "--mamba-cache-dtype", "float16", "--enable-prefix-caching",
  "--mamba-cache-mode", "align", "--reasoning-parser", "qwen3",
  "--enable-auto-tool-choice", "--tool-call-parser", "qwen3_coder",
  "--speculative-config", '{"method":"mtp","num_speculative_tokens":3}',
]

[[models.qwen.profiles]]
name = "l40s-32k"
gpu_types = ["NVIDIA L40S", "NVIDIA RTX A6000", "NVIDIA A40"]
max_model_len = 32768
max_num_seqs = 8
extra_vllm_args = [
  "--language-model-only", "--max-num-batched-tokens", "1536",
  "--kv-cache-dtype", "fp8", "--mamba-ssm-cache-dtype", "float16",
  "--mamba-cache-dtype", "float16", "--enable-prefix-caching",
  "--mamba-cache-mode", "align", "--reasoning-parser", "qwen3",
  "--enable-auto-tool-choice", "--tool-call-parser", "qwen3_coder",
]
```

Two lines are left out of the block because the test cannot hold real credentials; the running
document adds them:

- `api_key` or `api_key_file` in `[providers.runpod]`: the account's Runpod API key (see "The
  account key").
- `vllm_api_key_file` in each model: the file holding the key the model's pod expects. Its value
  is also stored in the Runpod account as the secret `vllm_<alias>` with every `-` and `.` written
  `_`: `vllm_inkling_pod` and `vllm_qwen`.

## Prices

Runpod's public price list, https://www.runpod.io/pricing, read on 2026-11-26, per GPU per hour.
The document's `cloud_type` has one value, `Secure`, so the Secure Cloud column is the one billed.

| GPU type | VRAM | Secure Cloud | Community Cloud | Used by |
| --- | --- | --- | --- | --- |
| `NVIDIA RTX PRO 6000 Blackwell Server Edition` | 96 GB | $2.69 | $1.69 | `inkling-pod` |
| `NVIDIA H100 PCIe` | 80 GB | $2.89 | $1.99 | `inkling-pod`, `qwen` h100-64k |
| `NVIDIA H100 NVL` | 94 GB | $3.19 | $2.59 | `qwen` h100-64k |
| `NVIDIA L40S` | 48 GB | $1.09 | $0.79 | `qwen` l40s-32k |
| `NVIDIA RTX A6000` | 48 GB | $0.69 | $0.33 | `qwen` l40s-32k |
| `NVIDIA A40` | 48 GB | $0.59 | $0.35 | `qwen` l40s-32k |

`price_per_hour_usd` is one value per model, the most its pod costs on any of its GPU types
(docs/hosting.md, "Spend ceilings"): 2.89 for `inkling-pod`, 3.19 for `qwen`. The spend check
prices a pod at that value whatever card it got, so a `qwen` pod on an A40 is counted at 3.19.
The GPU type ids are those of https://docs.runpod.io/references/gpu-types.

`NVIDIA H100 80GB HBM3` ($4.39) and `NVIDIA B200` ($7.99), which docs/model-profiles.md's
high-throughput profile also lists, are left out: with them `qwen`'s price would be 7.99, and a
start is admitted only while today's spend plus the new pod's price times its
`start_wait_seconds` stays within `daily_cap_usd`. At 7.99 and 900 s that is 2.00 of the 3.33.

## The spend ceiling

`daily_cap_usd = 3.33` is the owner's monthly budget of 100 USD over 30 days, one ceiling for
every Runpod route of the account. The provider declares it, so every model declares
`price_per_hour_usd`. At the declared prices it pays for about 1.15 pod-hours of `inkling-pod`
or 1.04 pod-hours of `qwen` a day, less while both run. `idle_timeout_minutes = 10` stops an idle
pod after 10 minutes, and `start_wait_seconds = 900` keeps each start's admission estimate at
0.72 (`inkling-pod`) and 0.80 (`qwen`).

## The fast route: Inkling 2 Mini on vLLM

vLLM serves Inkling 2 Mini: https://vllm.ai/blog/2026-11-20-inkling2-mini (2026-11-20) names
day-0 support on Hopper and Blackwell and the image `vllm/vllm-openai:inkling-2-mini`. The
weights are https://huggingface.co/thinkingmachines/Inkling-2-Mini, one checkpoint of about
20 GB in NVFP4 and MXFP8 (its model card). vLLM's Inkling 2 kernels need SM90 or newer, so no
Ampere or Ada card (L40S, A6000, A40) is listed.

Measured by Hugging Face on one RTX PRO 6000 96 GB,
https://huggingface.co/blog/inkling-2-mini-inference-hillclimb: 190.6 tok/s single-user decode
and 716 tok/s for 8 users, with vLLM nightly and DSpark speculative decoding at 7 draft tokens
(the flags of the block); 157.0 tok/s without speculation. Those runs used the image
`vllm/vllm-openai:nightly`, not `vllm/vllm-openai:inkling-2-mini`, and a context of 262144. The
block's 65536 and 8 sequences are not measured. No figure for an H100 PCIe was found.

`thinking = "on"` sends no chat-template argument; `"off"` would send Qwen's
`enable_thinking: false` and Qwen's sampling, which are not Inkling's.

## The strong route: Qwen3.8-27B

docs/model-profiles.md's fallback block with two changes: the 80 GB profile lists `H100 NVL` and
`H100 PCIe` (see "Prices"), and thinking is on. Measured by llmgw on an H100 NVL: 70.3 tok/s
without and 109 tok/s with MTP at three draft tokens (docs/model-profiles.md, "High throughput").
When no 80 GB card is free, the start falls back to the 32k profile on the 48 GB cards.

## The account key

The gateway reads the Runpod API key once at start, from one of two places in
`[providers.runpod]`: `api_key`, a `keychain:<service>/<account>` reference to an OS keychain
entry, or `api_key_file`, the path of a file the owner fills with the key. The file is read
under the rules of `vllm_api_key_file`: not a symbolic link, a regular file owned by the owner or
root, no group or world permission (mode `0600` or `0400`), at most 4096 bytes of one printable
token once a trailing newline is trimmed. A file that breaks a rule refuses the start before any
request reaches Runpod, as `endpoint-credential:<rule>` naming
`providers.runpod.api_key_file` and neither the path nor the key. The two keys are refused
together. Until the key is there and the owner says so, no call reaches the Runpod API from this
document: with neither key both models answer `target-unavailable`.

## Choosing a route

Three routes for a client that picks a model per task:

1. `inkling-2-mini` (the local Ollama model, `[hosted.inkling-2-mini] kind = "ollama"`): the
   default. No spend, always callable while the host's Ollama runs.
2. `inkling-pod`: the same model family on a rented card, for a batch the local model is too slow
   for. Woken first, then called; it stops itself after 10 idle minutes.
3. `qwen`: harder tasks where the local model's answer was not good enough. Woken first, then
   called.

A client reads `GET /v1/status` before a call: `callable: false` with phase `stopped` means the
pod needs a wake, and a call would be refused `model-stopped`. A wake refused `spend-ceiling`
means the day's budget is used; fall back to `inkling-2-mini`.
