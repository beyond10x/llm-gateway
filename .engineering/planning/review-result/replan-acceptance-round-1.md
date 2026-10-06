---
format: aep.planning-md/3
id: review-result:replan-acceptance-round-1
kind: review-result
status: active
title: Acceptance critic, spec-hardening epic and gateway-deployment split, round 1
relations:
- reviews: epic:spec-hardening
- reviews: story:gateway-spec-declarations
- reviews: story:hosting-spec-declarations
- reviews: story:spec-diff-gate
- reviews: story:hosting-lifecycle-declared
- reviews: story:public-model-listing
- reviews: story:container-image
- reviews: story:provider-key-files
- reviews: story:gateway-deployment
- reviews: story:runpod-production-transport
- reviews: dependency-blocker:llm-usage-decoders
revision: 1
---
needs-revision

story:gateway-spec-declarations — acceptance 1 says "For each of the review's 25 `missing` findings", but no stored artifact lists those 25, so nobody can tell which rules are owed or whether all were declared; commit the enumerated findings and cite them — .engineering/planning/story/gateway-spec-declarations.md:40; docs/verification/spec-hardening.md:26-30 (counts and "Among them" examples only)
story:hosting-spec-declarations — acceptance 1 says "For each of the review's 35 `missing` findings", but the 35 are enumerated nowhere (and neither are the 4 spec-only items acceptance 4 counts), so the check is a vote — .engineering/planning/story/hosting-spec-declarations.md:43; docs/verification/spec-hardening.md:26-35
story:spec-diff-gate — acceptance 3 names the pass with an acknowledgement and the refusal of a stale one but never says the same unclassified change fails without one, so it reads the same before the gate exists as after (and it joins two outcomes) — .engineering/planning/story/spec-diff-gate.md:42-44
story:public-model-listing — acceptance 1 checks for owner-only facts only in the `GET /v1/models` body, while the Outcome says neither route renders provenance, `config_digest`, endpoint or credential, so `GET /` can leak and pass — .engineering/planning/story/public-model-listing.md:46
story:public-model-listing — acceptance 2 says the variant is "chosen by `User-Agent` as llmgw does" without naming one `User-Agent` value or its variant, so the check needs llmgw's source at `048ebd8` — .engineering/planning/story/public-model-listing.md:49; docs/llmgw-capability-matrix.md:47
story:public-model-listing — the dropping of "a closed set of two literal paths" and "every response is application/json" from `docs/gateway.md` appears only under "ESS first", so the acceptance is met with the document still contradicting the new routes — .engineering/planning/story/public-model-listing.md:40-42; docs/gateway.md:87,100
story:container-image — acceptance 1 claims the `Dockerfile` "builds" and "listens on 8080", but its only check parses base image, user and entrypoint (no port, no source-revision label that row D1 names), so a Dockerfile that fails to build or exposes another port passes — .engineering/planning/story/container-image.md:32; docs/llmgw-capability-matrix.md:169
story:container-image — acceptance 2 requires only that both blocks parse, and names neither profile nor the weight-cache block that row D3 includes, so any parseable block closes D3 — .engineering/planning/story/container-image.md:35; docs/llmgw-capability-matrix.md:171
story:provider-key-files — acceptance 3 names no source for the vLLM key ("the source the document names") and no outcome for the derivation decision ("is recorded ... and a test pins the result"), and it joins the two, so a closer cannot say what B9 requires — .engineering/planning/story/provider-key-files.md:49-51; docs/llmgw-capability-matrix.md:133
story:gateway-deployment — "ESS first" has the specification declare the bearer and the `Relay` grammar gain an observation, but no acceptance line requires either declaration, so the story closes with the bearer only in a process test — .engineering/planning/story/gateway-deployment.md:44-45,49-55
story:runpod-production-transport — acceptance 3 ("A listing that does not prove it is complete is not reported as complete") names no test or fixture and does not say what proves completeness, unlike acceptance 2 — .engineering/planning/story/runpod-production-transport.md:48; crates/llm-runpod/src/transport.rs:41

What I read: 11 of 11 ids with `aep plan artifact show`, plus `aep plan artifact kinds` and `lifecycle story|epic|dependency-blocker`. I checked `docs/verification/spec-hardening.md`, the capability matrix rows, `machine.rs` `Phase`/`TRANSITIONS` and `transport.rs:5-7`. I also checked the `docs/gateway.md` refusal table (19 codes), the `AGENTS.md` Gate section and `ess verify diff|mutate --help` (0.53.0).

Passed without finding: epic:spec-hardening (its arithmetic of 82, 37 and 45 matches the record), story:hosting-lifecycle-declared and dependency-blocker:llm-usage-decoders.

Could not establish:
- the 0.53.0 `--fail-on` and `ess-diff-acknowledgements/1` flags exist, but I did not run them against this tree;
- llm 0.2.0's private and `pub(crate)` readers in the blocker, because I did not open the llm tree;
- whether the Runpod transport's `PodListing` can express "proves complete" beyond a bool.

Out of lane, not counted:
- story:runpod-production-transport's Scope still says K7, B8 and B9 "belong to story:gateway-deployment", which is false after the split (scope or design);
- `AGENTS.md:39` says the transport's tests "stay on the emulator" while the acceptance says loopback fixtures (design);
- the `Requested -> Disowned` scenario in story:hosting-spec-declarations acceptance 5 and the lifecycle in story:hosting-lifecycle-declared acceptance 2 may overlap (design).

```findings
- file: .engineering/planning/story/gateway-spec-declarations.md
  line: 40
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 1 says 'For each of the review's 25 missing findings' but no stored artifact lists those 25, so nobody can tell which rules are owed; docs/verification/spec-hardening.md:26-30 holds counts and examples only"
- file: .engineering/planning/story/hosting-spec-declarations.md
  line: 43
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 1 says 'For each of the review's 35 missing findings' but the 35 (and the 4 spec-only items of acceptance 4) are enumerated nowhere; docs/verification/spec-hardening.md:26-35 holds counts and examples only"
- file: .engineering/planning/story/spec-diff-gate.md
  line: 42
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "acceptance 3 names the pass with an acknowledgement and the refusal of a stale one but never says the same unclassified change fails without one, so it reads the same before the gate exists as after, and it joins two outcomes"
- file: .engineering/planning/story/public-model-listing.md
  line: 46
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "acceptance 1 checks for owner-only facts only in the GET /v1/models body, while the Outcome says neither route renders provenance, config_digest, endpoint or credential, so GET / can leak and pass"
- file: .engineering/planning/story/public-model-listing.md
  line: 49
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 2 says the variant is 'chosen by User-Agent as llmgw does' without naming one User-Agent value or its variant, so the check needs llmgw's source at 048ebd8"
- file: .engineering/planning/story/public-model-listing.md
  line: 40
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "dropping 'a closed set of two literal paths' and 'every response is application/json' from docs/gateway.md is stated only under ESS first, so the acceptance is met with docs/gateway.md:87,100 still contradicting the new routes"
- file: .engineering/planning/story/container-image.md
  line: 32
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "acceptance 1 claims the Dockerfile builds and listens on 8080, but its only check parses base image, user and entrypoint (no port, no source-revision label that row D1 names), so a Dockerfile that fails to build or exposes another port passes"
- file: .engineering/planning/story/container-image.md
  line: 35
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "acceptance 2 requires only that both blocks parse and names neither profile nor the weight-cache block that row D3 includes, so any parseable block closes D3"
- file: .engineering/planning/story/provider-key-files.md
  line: 49
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 3 names no source for the vLLM key ('the source the document names') and no outcome for the derivation decision ('is recorded ... and a test pins the result'), and it joins the two, so a closer cannot say what B9 requires"
- file: .engineering/planning/story/gateway-deployment.md
  line: 44
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "ESS first has the specification declare the bearer and the Relay grammar gain an observation, but no acceptance line requires either declaration, so the story closes with the bearer only in a process test"
- file: .engineering/planning/story/runpod-production-transport.md
  line: 48
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "acceptance 3 ('A listing that does not prove it is complete is not reported as complete') names no test or fixture and does not say what proves completeness, unlike acceptance 2"
```
