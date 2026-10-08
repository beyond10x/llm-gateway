# Model profiles

A `[models.<alias>]` table in the deployment document is the whole serving decision: which card a
pod rents, how much context it serves, how many requests it batches, and which vLLM flags it
starts with. These are llmgw's two proven profiles and its weight-cache block (llmgw
`docs/model-profiles.md:8`, `:44` and `:88` at `048ebd8`), written as blocks of this repository's
deployment document. `spec/domains/deployment.yaml` holds every key, default and range; the
[README](../README.md#the-deployment-document) shows the rest of the document.

Switching profile is replacing one table with another; no image build. Each table names the
provider `runpod`, so the document declares it:

```toml
[providers.runpod]
kind = "runpod-vllm"
```

`crates/llm-gateway-cli/tests/profiles.rs` loads every block below through the binary's own
loader, so this file cannot hold a block the binary refuses.

## Default: broad availability, 32k context

Rents whichever of three widely available 48 GB cards has capacity, which keeps cold starts from
failing when one card type is momentarily unobtainable. Non-thinking sampling from the model card;
no speculative decoding. llmgw `docs/model-profiles.md:8-39`.

```toml
[models."qwen3.8-27b"]
provider = "runpod"
wires = ["chat", "responses", "messages"]
context_window = 32768
hf_model = "Qwen/Qwen3.8-27B-FP8"
image = "vllm/vllm-openai:v0.27.1"
gpu_types = ["NVIDIA L40S", "NVIDIA RTX A6000", "NVIDIA A40"]
max_model_len = 32768
max_num_seqs = 8
gpu_util = 0.90
disk_gb = 80
thinking = "off"
extra_vllm_args = [
  "--language-model-only", "--max-num-batched-tokens", "1536",
  "--kv-cache-dtype", "fp8", "--mamba-ssm-cache-dtype", "float16",
  "--mamba-cache-dtype", "float16", "--enable-prefix-caching",
  "--mamba-cache-mode", "align", "--reasoning-parser", "qwen3",
  "--enable-auto-tool-choice", "--tool-call-parser", "qwen3_coder",
]
idle_timeout_minutes = 30
start_wait_seconds = 1800
```

## High throughput: H100 NVL with MTP speculative decoding, 64k context

The profile llmgw measured on 2026-08-18: two pods, identical flags apart from
`--speculative-config`, 70.3 tok/s without and 109 tok/s with MTP at three draft tokens. Four
draft tokens killed the engine; do not raise it without re-measuring the acceptance rate. 64k
context at 64 concurrent sequences does not fit a 48 GB card, so this profile rents only cards of
80 GB or more. llmgw `docs/model-profiles.md:44-86`.

```toml
[models."qwen3.8-27b"]
provider = "runpod"
wires = ["chat", "responses", "messages"]
context_window = 65536
hf_model = "Qwen/Qwen3.8-27B-FP8"
image = "vllm/vllm-openai:v0.27.1"
gpu_types = ["NVIDIA H100 NVL", "NVIDIA H100 80GB HBM3", "NVIDIA B200"]
max_model_len = 65536
max_num_seqs = 64
gpu_util = 0.90
disk_gb = 80
thinking = "on"
extra_vllm_args = [
  "--language-model-only", "--max-num-batched-tokens", "1536",
  "--kv-cache-dtype", "fp8", "--mamba-ssm-cache-dtype", "float16",
  "--mamba-cache-dtype", "float16", "--enable-prefix-caching",
  "--mamba-cache-mode", "align", "--reasoning-parser", "qwen3",
  "--enable-auto-tool-choice", "--tool-call-parser", "qwen3_coder",
  "--speculative-config", '{"method":"mtp","num_speculative_tokens":3}',
]
idle_timeout_minutes = 30
start_wait_seconds = 1800
```

Availability is the cost. On 2026-08-19 Runpod refused creation on `NVIDIA H100 NVL` for several
minutes while capacity churned, and a single-entry `gpu_types` list has nothing to fall back to.
The list above keeps two alternatives for that reason.

## Weight cache (optional, either profile)

A cold start downloads the checkpoint every time. Attaching a Runpod network volume and pointing
the model cache at it removes that download at the price of standing storage. Add these keys to
either profile's table. llmgw `docs/model-profiles.md:88-103`.

`YOURVOLUMEID` is a placeholder: put the id of your own Runpod network volume there, and set
`data_center_ids` to the one data center that volume lives in. The document accepts 1 to 64 ASCII
letters and digits as a volume id.

```toml
network_volume_id = "YOURVOLUMEID"
volume_mount_path = "/workspace"
data_center_ids = ["US-CA-2"]
```

A network volume exists in exactly one data center, so the pod is pinned there and rents only what
that data center has: llmgw records `US-CA-2` as listing H100 80GB HBM3 and B200, not L40S, so
with the default profile only the cards that data center has can be rented. The document refuses
`network_volume_id` without `data_center_ids`. Stopping pods instead of terminating them is not an
alternative: Runpod clears the container disk on stop, keeps billing the storage, and may return no
GPU on resume.

## What did not carry over

llmgw's `reasoningEffort` and `sampling` are empty strings in both profiles; here an unset key is
the same thing, so the blocks omit them. llmgw's `deployment.zwirnVllmContextWindow` belongs to
llmgw's own deployment configuration and has no counterpart in this document.
