# llm-gateway

The LLM gateway and hosted-inference provisioning: an authenticated, single-owner gateway that
translates the supported protocol subset, and the hosting port with its Runpod and Modal adapters.

These crates are moving here from [beyond10x/llm](https://github.com/beyond10x/llm), which keeps the
client side (the protocol crates, credentials, routing and cost). The move is llm
`story:serving-extraction`; until it lands, the code lives in llm.

[beyond10x/llmgw](https://github.com/beyond10x/llmgw) stays in service until a qualified,
reversible cutover (llm `story:llmgw-retirement`).

Licensed under Apache-2.0 ([LICENSE](LICENSE)).
