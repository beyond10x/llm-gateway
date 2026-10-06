---
format: aep.planning-md/3
id: review-result:replan-acceptance-round-2
kind: review-result
status: active
title: Acceptance critic, spec-hardening epic and gateway-deployment split, round 2
relations:
- reviews: story:gateway-spec-declarations
- reviews: story:hosting-spec-declarations
- reviews: story:spec-diff-gate
- reviews: story:live-runpod-wiring
- reviews: story:public-model-listing
- reviews: story:runpod-production-transport
- reviews: epic:spec-hardening
- reviews: story:provider-key-files
- reviews: story:container-image
- reviews: story:gateway-deployment
- reviews: story:hosting-lifecycle-declared
- reviews: dependency-blocker:llm-usage-decoders
revision: 1
---
needs-revision

story:gateway-spec-declarations — acceptance 4 says row 38 "is answered from the code" but names nothing that states the answer, and the item-5 re-run only forbids `missing`/`contradicts`, so an `unclear` row reads the same before and after (name the sentence in the specification and README that carries the answer) — .engineering/planning/story/gateway-spec-declarations.md:51
story:hosting-spec-declarations — acceptance 3 says rows 40-42 are "answered from the code" with no named artifact, and the item-6 re-run only forbids `missing`/`contradicts`, so no check distinguishes answered from unanswered (name where each answer is written) — .engineering/planning/story/hosting-spec-declarations.md:53
story:spec-diff-gate — acceptance 4 and 5 name `contracts/diff-acknowledgements.yaml`, but ESS reads only a JSON `ess-diff-acknowledgements/1` file (`format`, `before`, `after`, `acknowledged`) and refuses YAML, so the check as written cannot pass; name a JSON file — .engineering/planning/story/spec-diff-gate.md:46 (shown by `ess verify diff --from spec --to spec --fail-on breaking-or-unknown --acknowledgements <(printf "schema: ess-diff-acknowledgements/1\nbefore: x\n")` → `refused: acknowledgements: expected value at line 1 column 1`)
story:live-runpod-wiring — the outcome's "Without the file, the binary refuses to start a pod rather than fall back to the emulator" has no acceptance item, so that behavior can ship untested — .engineering/planning/story/live-runpod-wiring.md:29
story:live-runpod-wiring — acceptance 1 runs the binary "against a loopback fixture of the Runpod API" but names no document key or setting that points the production transport at that fixture, so the test cannot be written from the story as drafted — .engineering/planning/story/live-runpod-wiring.md:42
story:public-model-listing — acceptance 6 requires docs/gateway.md to no longer say "every response is application/json", but that string is not in the document (`grep -n "every response is" docs/gateway.md` finds nothing; line 87 reads "every response the gateway writes itself is `application/json`"), so the check already passes before the work and does not pin the change — .engineering/planning/story/public-model-listing.md:63
story:public-model-listing — "ESS first" says the specification declares "the answers to other methods", but no acceptance item pins any method answer (for example `POST /v1/models`), so a declared behavior has no check — .engineering/planning/story/public-model-listing.md:43
story:runpod-production-transport — "This story updates that row to say so" (the AGENTS.md "Invariants" paid-call row, AGENTS.md:39) sits after the numbered acceptance as a statement, so no item states what the row says afterward — .engineering/planning/story/runpod-production-transport.md:62

What I read: all 12 ids given, whole bodies, via `aep plan artifact show` plus the lifecycle commands, and the two review tables (`docs/verification/spec-hardening-{gateway,hosting}-review.md`). I did not open any review-result artifact. The row ranges match the tables (gateway 4-28 missing, 29-37 spec-only, 38 unclear; hosting 5-39 missing, 40-42 unclear, 43-46 spec-only). Also confirmed: 179 scenarios in `contracts/suite.json`, `LeaseExpired` in `hosting.yaml:17`, `Requested -> Disowned` in `machine.rs:60`, and the transport.rs:5-7 and :41 citations.

What I could not establish:
- The epic's second design review and the stories' item-5/6 re-runs depend on an agent review returning no findings, which may not give the same answer twice (unease, not a finding; each story's row-by-row item 1 offsets it).
- Whether ESS 0.53.0 gives `unclassified-changed` for an added domain (acceptance 3 of `story:spec-diff-gate`, ess#469); I did not run it.
- Whether `story:hosting-lifecycle-declared`'s `mutate` run exits 0 here; it needs an `--emit`/`--collect` runner, since the built-in targets are billing, oracle-fixture and interpreted.
- Out of my lane, not counted in the verdict: `story:live-runpod-wiring` uses the Runpod key that `story:provider-key-files` reads but lists no `depends_on` on it (design); `story:runpod-production-transport` does not list AGENTS.md in its scope (scope).

```findings
- file: .engineering/planning/story/gateway-spec-declarations.md
  line: 51
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 4 says row 38 is 'answered from the code' but names no sentence in the specification or README that carries the answer, and the item-5 re-run only forbids missing or contradicts findings, so an unclear row reads the same before and after"
- file: .engineering/planning/story/hosting-spec-declarations.md
  line: 53
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 3 says rows 40-42 are 'answered from the code' with no named artifact, and the item-6 re-run only forbids missing or contradicts findings, so no check distinguishes answered from unanswered"
- file: .engineering/planning/story/spec-diff-gate.md
  line: 46
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 4 and 5 name contracts/diff-acknowledgements.yaml, but ess verify diff --acknowledgements reads only a JSON ess-diff-acknowledgements/1 file and refuses YAML, so the check as written cannot pass"
- file: .engineering/planning/story/live-runpod-wiring.md
  line: 29
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the outcome's 'Without the file, the binary refuses to start a pod rather than fall back to the emulator' has no acceptance item, so that behavior can ship untested"
- file: .engineering/planning/story/live-runpod-wiring.md
  line: 42
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "acceptance 1 runs the binary against a loopback fixture of the Runpod API but names no document key or setting that points the production transport at the fixture, so the test cannot be written from the story as drafted"
- file: .engineering/planning/story/public-model-listing.md
  line: 63
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 6 requires docs/gateway.md to no longer say 'every response is application/json', but that string is not in the document (docs/gateway.md:87 reads 'every response the gateway writes itself is application/json'), so the check already passes before the work"
- file: .engineering/planning/story/public-model-listing.md
  line: 43
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "'ESS first' says the specification declares 'the answers to other methods', but no acceptance item pins any method answer such as POST /v1/models, so a declared behavior has no check"
- file: .engineering/planning/story/runpod-production-transport.md
  line: 62
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "'This story updates that row to say so' about the AGENTS.md Invariants paid-call row sits after the numbered acceptance as a statement, so no item states what the row says afterward"
```
