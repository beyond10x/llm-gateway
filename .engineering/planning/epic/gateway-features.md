---
format: aep.planning-md/3
id: epic:gateway-features
kind: epic
status: draft
title: 'Gateway features beyond llmgw: hosted endpoints, target fallback, usage records'
relations:
- serves: vision:portable-model-inference
revision: 3
---
## Outcome

Behind its one owner credential, llm-gateway does three things that other LLM gateways commonly do
and llmgw never did: it serves a model from a hosted endpoint as well as from a Runpod pod, it
falls back between a route's ordered targets within one request, and it records what each model
call cost in tokens.

## Why

Operator request, 2026-10-06: research the main features of LLM proxies and gateways, and plan
what llm-gateway should do next.

## Order

This epic comes after llmgw parity, and none of its stories is on the cutover path. The cutover
(llm `story:llmgw-retirement`) waits on the 21 open rows of `docs/llmgw-capability-matrix.md`
(12 partial, 9 gap). The draft stories of `epic:gateway` and `epic:hosting` cover all of them:
`story:runpod-production-transport` (B rows), `story:gateway-deployment`, `story:cold-start-hold`
and `story:gateway-observability`.

## Research, 2026-10-06

The research compared seven gateways: LiteLLM, Portkey, Kong AI Gateway, Envoy AI Gateway,
Cloudflare AI Gateway, Bifrost and TensorZero. The table shows each feature they share, which of
them name it, and where it stands here.

| Feature | Named by | llm-gateway today | Here |
| --- | --- | --- | --- |
| One endpoint for several API shapes | all | relays chat, responses and messages untranslated (rows R6-R8) | `story:gateway-translation` (draft) |
| Model aliases, rewritten to the upstream name | Envoy (model virtualization), LiteLLM | covered (row W2) | none needed |
| Provider fallback, retries | all | a failed target is dropped and the next request gets a replacement (row W7); nothing tries a second target within one request | `story:target-fallback` |
| Hosted provider endpoints, upstream credentials | Envoy (upstream authentication), Cloudflare (BYOK), LiteLLM (secret managers) | `runpod-vllm` is the only provider kind | `story:hosted-endpoints` |
| Token usage and cost per request | Envoy, Kong (OpenTelemetry GenAI metrics), LiteLLM (spend tracking), Cloudflare (custom costs) | nothing reads the answer | `story:usage-records`, blocked on `decision-blocker:usage-from-responses` |
| Metrics, logs | all | none (rows R4, O1-O3) | `story:gateway-observability` (draft) |
| Self-hosted endpoint lifecycle, scale to zero | Envoy (InferencePool), llm-d | hosting contract and Runpod adapter on an emulator | `epic:hosting` drafts |

## Not in scope

| Feature | Named by | Why not |
| --- | --- | --- |
| Virtual keys, teams, per-key budgets and rate limits | LiteLLM, Portkey, Bifrost, Cloudflare | Decided 2026-09-27 (`spec/domains/gateway.yaml` header): no multi-tenant accounts or quotas; the embedding platform runs one gateway per owner |
| Guardrails, PII redaction, prompt templates, prompt compression | Portkey, Kong, Cloudflare | Each rewrites or blocks prompt content; the relay changes only `model` and the messages-wire effort (rows W2, W4) |
| Exact or semantic response caching | LiteLLM, Portkey, Kong, Cloudflare, Bifrost | No source here shows repeated identical requests; provider prompt caching already passes through the relay |
| MCP gateway | Envoy, LiteLLM | llm owns model inference, not tool execution (`vision:portable-model-inference`; llm `AGENTS.md` "Boundaries") |
| A/B tests, traffic mirroring, canaries | LiteLLM, Portkey, TensorZero | No source here asks for it |
| Load balancing across replicas of one model | LiteLLM, Portkey, Bifrost, Envoy | One pod serves one model (llmgw `AGENTS.md` invariant 4); a second pod for one model is a hosting decision |

## Specification

The nouns of the three stories are declared, marked `PLANNED`, in `spec/domains/upstream.yaml`
(`HostedEndpoint`, `TargetAttempt`) and `spec/domains/telemetry.yaml` (`UsageRecord`,
`MetricSeries`). `ess specify validate --path spec` reports them valid on ESS 0.52.0 and 0.53.0.

## Sources

- https://docs.litellm.ai/docs/simple_proxy
- https://portkey.ai/docs/introduction/feature-overview
- https://developer.konghq.com/ai-gateway/
- https://developer.konghq.com/ai-gateway/ai-otel-metrics/
- https://theagentrouter.ai/docs/capabilities/ (Envoy AI Gateway; `aigateway.envoyproxy.io/docs/capabilities/` redirects there)
- https://developers.cloudflare.com/ai-gateway/features/
- https://www.tensorzero.com/docs/gateway
- https://llm-d.ai/docs/getting-started

## Done when

Each of `story:hosted-endpoints` and `story:target-fallback` is either `implemented` or
`archived`. An implemented story's acceptance cases pass in `task check`, as the conformance
scenarios and crate tests its body names. An archived story gives the reason in its body.
`decision-blocker:usage-from-responses` is `cleared`. After that, `story:usage-records` is either
`implemented` (option A) or `archived` (option B or C).
