---
format: aep.planning-md/3
id: review-result:adversary-w12-kubernetes-deployment-guide-pass-1
kind: review-result
status: active
title: Wave 2026-10-16-w03 adversary, story:kubernetes-deployment-guide, pass 1
relations:
- reviews: story:kubernetes-deployment-guide
revision: 1
---
unit: story:kubernetes-deployment-guide, adversary pass 1, covering adv/kubernetes-deployment-guide at aa579d7 (impl 463d4dd plus one test commit)
verdict: red (three findings, all introduced by the page)
cases: executed 246→250, red 3
origin: n/a (adversary: introduced 3, pre-existing 0, undecided 0)
wrote-outside-worktree: one scratch directory of the session

1. Diff since 463d4dd: crates/llm-gateway-cli/tests/adversary_k8s_guide.rs (+235), test only; each case reads its input from the page.
2. Attacked and not broken: the page's gateway.toml starts and its pasted lines match byte for byte; chmod then chown succeed with only CAP_CHOWN; the trusted reader accepts a 0400 copy owned by the runtime user under a 2777 emptyDir; the 0440 test fails on a drifted refusal line; probe paths match the server; no story id or home path on the page.
3. Judgement, no test: the restricted Pod Security level refuses runAsUser 0 and an added CHOWN, so the page must name the level its init container needs; "allows an answer of 300" is set nowhere.
4. Decisions (coordinator): the init script removes its earlier copy before copying; the pasted command uses the Deployment's config path; the page states the drain as the server does it (readiness turns 503 so the Service stops routing, the listener stays open until accepted connections are answered, a connection made meanwhile waits); the page names the Pod Security level (baseline) the init container needs; the 300 claim goes. The server's listener behaviour is unchanged in this wave.

```findings
[{"file":"website/docs/guides/run-on-kubernetes.md","line":83,"category":"acceptance","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"the init script's plain cp fails with EACCES when the init container re-runs over its own 0400 copy in the surviving emptyDir"},
 {"file":"website/docs/guides/run-on-kubernetes.md","line":172,"category":"contract-drift","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"the pasted command names /run/gateway.toml while the Deployment passes /etc/llm-gateway/gateway.toml"},
 {"file":"website/docs/guides/run-on-kubernetes.md","line":194,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"the listener stays open through the drain, so a post-SIGTERM connection completes its handshake and waits instead of being refused"}]
```
