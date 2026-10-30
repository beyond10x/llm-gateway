---
format: aep.planning-md/3
id: story:trusted-refusal-names-no-path
kind: story
status: implemented
title: A trusted-file refusal names its setting, never the path
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T06:14:07Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-30T06:14:07Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-30T06:59:25Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
## Outcome

No trusted-file refusal quotes the path it was given or the file a link resolves to. The refusal
code names the source (`config:unreadable`, `owner-secret:unsafe-mode`, `container-token:symlink`,
...), a caller that knows the setting names it (`routes`, the key's field, the environment
variable), and the message carries only the rule's detail (`No such file or directory`, `is a
symbolic link`, a mode or a size).

## Why

`review-result:security-w01-cli-server-pass-1` finding 2 (`crates/llm-gateway-cli/src/trusted.rs`
`refuse`): every trusted-file refusal starts with the path verbatim, so a key passed by mistake as
`--config=<key>` or as a `*_file` value is printed in the refusal and in the container token's
warning log. Wave 2026-10-30-w01 kept it because the path helps an operator find a wrong setting;
the setting's name gives that help without echoing a value, as `credential:unknown-target` already
does for a name (w01).

## Acceptance

- A test per trusted source gives a path holding a marker (missing, a directory, a symbolic link,
  unsafe mode, over the size bound) and finds the marker in no refusal and no log line; each
  refusal still names its code and the rule.
- `docs/gateway.md` no longer says the symlink refusal names the file it resolves to.
- `credential set`, which writes the same `*_file` settings, names the setting and never the
  path or its directory, in its `credential:directory-*` and `credential:write` refusals and in
  its success line (`review-result:adversary-w03-refusal-pass-1`).
