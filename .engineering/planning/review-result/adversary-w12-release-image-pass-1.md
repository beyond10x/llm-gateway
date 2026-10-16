---
format: aep.planning-md/3
id: review-result:adversary-w12-release-image-pass-1
kind: review-result
status: active
title: Wave 2026-10-16-w03 adversary, story:release-image, pass 1
relations:
- reviews: story:release-image
revision: 1
---
unit: story:release-image, adversary pass 1, covering adv/release-image at f95765b (impl/release-image a057547 plus one test commit)
verdict: red (two findings introduced, one undecided, one note)
cases: executed 251→258, red 2
origin: n/a (adversary: introduced 3, pre-existing 0, undecided 1)
wrote-outside-worktree: mutant copies and run logs in the session scratch directory only
needs-coordinator: yes (finding 3, the first-ever push to a package that does not exist yet)

1. Diff since a057547: `crates/llm-gateway-cli/tests/adversary_w12_release_image.rs` (+340), test only; release.yml was mutated in scratch copies only.
2. Runs: the new file alone `5 passed; 2 failed`; crate suite 256 passed, 2 failed (exit 101); clippy exit 0. No conformance run: no spec change (the unit is CI only).
3. Three new cases are green and red on scratch mutants the shipped text checks would miss: a fail-open `*)` branch, `if: always()` on the build job, and an expression in a `run:` line.
4. Attacked and not broken: all six action SHA pins resolve to their named tags; each deviation acceptance 3 lists is caught; the tag glob matches neither `v1.2.3` nor `1.2.3-rc1`; only a tag push triggers; `run:` lines read `$GITHUB_REF_NAME`, no expression.
5. Decision on finding 3 (coordinator): a DENIED or UNAUTHORIZED registry answer asks the packages API with the job token; a 404 there means the package was never created, so the version is absent; any other answer fails closed. The README says the package is private when first created.

```findings
[{"file":".github/workflows/release.yml","line":38,"category":"mutant","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"any 'not found' text, including bash's 'docker: command not found', passes the first existence check; test adv_w12_first_check_fails_closed_when_docker_cannot_run"},
 {"file":".github/workflows/release.yml","line":115,"category":"boundary","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"the manifest re-check reads a registry error as absent and proceeds to imagetools create --tag; test adv_w12_manifest_recheck_never_tags_when_the_registry_errors"},
 {"file":".github/workflows/release.yml","line":37,"category":"judgement","severity":"warning","verdict":"INFEASIBLE","origin":"undecided","message":"ghcr's answer for the not-yet-created package may be DENIED, which the check treats as unknown, blocking the first release"},
 {"file":"CHANGELOG.md","line":3,"category":"contract-drift","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"the header still calls every release a source release while releases now publish an image"}]
```
