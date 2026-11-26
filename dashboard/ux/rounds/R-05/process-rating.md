# Independent rating of the dashboard UI improvement loop (rounds R-01 to R-05)

Scope: `dashboard/ux/` (findings, rounds, baseline, tools), `dashboard/e2e/`, the history of the branch, pull requests 68 (merged) and 69 (open), the planning files, against the `ui-iteration-loop` skill.
Checked by running: `pnpm --dir dashboard e2e --workers=2` through the pool (221 passed, wall 60.1 s, slowest test 7.56 s) and `pnpm --dir dashboard test` (74 of 75 passed, the one failure is explained in C). Nothing live was contacted; nothing in the repository was edited.

## Scores

| | area | score | one line |
|---|---|---|---|
| A | Findings | 3 / 5 | 18 records, 17 real; closed ones mostly have strong red-first tests, but 6 records are mis-severed, not real or closed early |
| B | Missed findings | 3 / 5 | reviewers found most visible defects; two seen and not filed, one structural gap (what is above the fold) never examined |
| C | Process against the skill | 3 / 5 | R-02 and R-03 followed it well; R-04 and R-05 dropped reviews and triage, planning files are empty templates, triage arithmetic has errors |
| D | Measurement validity | 3 / 5 | deterministic, hermetic, fast, with break tests for most probes; 51 of 60 baseline values are 0, three checks cannot fire or are unproven |
| E | Outcome | 3 / 5 | the page is measurably better on every gated defect; reader tasks are unmeasured and R-05 added copy regressions |

## A. Findings

Evidence base: the record, its shot or cited code, the regression test in `dashboard/e2e/`, the red and fix commits (`git log`), and the metric before and after in the round reports.

| id | real? | severity right? | asset strong? | closure honest? | note |
|---|---|---|---|---|---|
| UXF-001 theme toggle absent | yes (R-01 shot, harness found none) | S2 is high: the OS scheme still drives the page; S3 | yes: `gates.e2e.ts` toggle test runs under the opposite scheme and reloads; three break tests in `tools.e2e.ts` | yes, but the toggle code (commit f7b0888) predates the finding by 9 minutes, so no red-before-fix exists; the known-failures entry served as red | closed R-02 |
| UXF-002 scroll regions not keyboard-focusable | yes (axe 12 nodes in R-01) | S2 follows axe `serious`; practical impact S3 because Chromium focuses scrollers itself, as the record says | yes: asserts tabindex, role, name, Tab with ring, and axe at two viewports; red commit 21f18c6f8 states the red values | yes: axe 2 to 0 | closed R-03 |
| UXF-003 cache tile overruns its neighbour | yes (R-01 shot, overflow.elements 1) | yes S2 | medium: `r03.e2e.ts` `boxedOverflow` has no assertion that any box was found (`.tile`, `dl.headers`, `.card h3` are class selectors; the renderer move renamed classes), so it can pass vacuously | yes: 1 to 0 | closed R-03 |
| UXF-004 key data under 14 px | yes (10.4 px badges) | yes S3 | yes as a gate on every replay (`gates.e2e.ts` key data) | partly: R-03 closed it with 1.75 px of target unmet; R-04 reached 0 by redefining key data to exclude badges, second lines and column heads and adding an invented 12 px floor for them. The page has 12.25 px text that the metric no longer counts | closed early, then closed by moving the definition |
| UXF-005 layout shift on arrival | yes by metric (token-flow 0.074) but already under the web "good" bound of 0.1 in R-01 | S3 fine | weak: the asset is the ratchet test, which holds a value and cannot fail before a fix | open; the value fell 0.074 to 0.039 with no change set aimed at it; no target, no stop rule after 5 rounds | needs a target or a decision to close |
| UXF-006 status word breaks inside "warning" | yes (R-01 shot) | yes S3 | strong: `r04.e2e.ts` measures word lines over every state and viewport and checks titles against raw values | yes | closed R-04 |
| UXF-007 Routes table cut, no cue | yes (R-02 shot) | yes S2 | yes: every column in view at both gating viewports, all scenarios and states | yes at 1366/1280; the same defect at 390 is UXF-017 | closed R-03 |
| UXF-008 flow layer strays in gutters | yes (R-02 shot) | yes S3 | strong: structure plus the 32 x 16 hit test, both themes | yes: 0.005-0.09 to 1; owner decision recorded | closed R-03 |
| UXF-009 stale heartbeat only red | yes (R-05 shot shows the new wording) | yes S3 | medium-strong: text match on the definition value, fresh case checked | yes | closed R-04 |
| UXF-010 hash-named limit blocks, "▸ 2" | yes (R-05 busy sheet: "Subscription limits 3fa9c1d2", raw header lines) | too low: a reader cannot tell which credential is limited, the main diagnostic question; S2 | none yet | open since R-02, skipped in every change set | 4 rounds open |
| UXF-011 three state vocabularies | yes (R-05: "streaming", "RUNNING", "five_hour") | S3 fine | none yet | open since R-02 | |
| UXF-012 flow caption | yes | S3 fine | medium: regexes on caption text, both motion modes | yes | closed R-04 |
| UXF-013 flow panel height change | real only at 390 (measured-only); the "empty box in idle" half is a design cost | frequency scored 1.00 in triage, the record lists 2 of 4 scenarios | strong: heights and shift at every viewport, session comes and goes | closed early: the idle half is not fixed, the idle box "keeps its height" (it still takes 250 px for one line, see B) | |
| UXF-014 flow labels and strings contrast | yes | S3 fine | medium: `r04.e2e.ts` computes contrast from the exported `INK` constants, not from painted pixels, so a paint path that ignores `INK` still passes | closed early: labels stay 12 px and out of the key-font measure, route names cut at 390 with no full value; both are named "not done" in the record and have no open record | |
| UXF-015 toggle clipped at 390 | no: a probe defect (a sample point outside a rounded corner); the page was never wrong | S2 for a defect that is not in the page | the break test of the probe is right | honest diagnosis, but the record still says "about 20 % of the toggle lies outside the viewport"; it should be marked invalid, and it entered the change set as the top item with priority 3.0 | closed R-04, class `layout` wrong (tool defect) |
| UXF-016 renderer drops spaces | yes (R-05 busy sheet "2.05k tok/slast 60 s") | S3 fine | none yet | open; uncommitted record fails the repo's own findings test (see C) | regression introduced by the R-05 move |
| UXF-017 narrow tables cut, no cue | yes (R-05 390 shot: In flight shows Route and Model only) | S3 fine for a measured-only viewport | none yet | open; same failing-test problem | |
| UXF-018 Provider labels | yes (R-05 shot "Name", "Other pods", "Read 21s ago") | S3 fine | none yet | open | |

Count judged not real, mis-severed or closed early: 6 (UXF-015 not real; UXF-001 and UXF-010 mis-severed; UXF-004, UXF-013, UXF-014 closed early or by redefinition). UXF-005 is a seventh case of a different kind: no closing rule.

Other facts: 8 of 18 records have `metric: none` (006 has an assertion; 009, 010, 011, 012, 014, 016, 018 none), so most late findings cannot move a ratchet.

## B. Missed findings (R-05 contact sheets, review shots, page source)

| # | defect | evidence | severity |
|---|---|---|---|
| M1 | The first screen at 1366x768 and 1280x720 is the token-flow panel, Overview and subscription limits. In flight, Routes, Sessions, Recent calls and the Provider/pods block, which holds the stale-heartbeat alarm and the cost-bearing pods, start below the fold in busy and errors. There is no alert summary at the top and no metric for what is above the fold | `contact-1366x768-*.png` busy and errors: the Provider frame appears only on the 4th screen region; heuristics 1 and 3 were "not observed" in the reviews | S2 (judgement) |
| M2 | Idle: the flow panel keeps ~250 px for one line, then three empty tables with headers. The R-05 reviewer wrote this in prose and did not file it | `contact-1280x720-dark.png` idle; `review-heuristics.md` R-05 "Empty states" | S3 |
| M3 | The Provider balance tile wraps alone onto a second row of Overview at 1280x720 in busy and errors. The R-05 reviewer saw it in a scratch shot, "not filed" | `contact-1280x720-dark.png` busy and errors | S3 |
| M4 | Flat type hierarchy: section headings are 14 px bold, the same size as table cells, and the page title is 15.4 px (`style.css:62`, `--type-heading-size`). The reviewer judged it "not a defect" | `contact-1366x768-light.png` | S3 |
| M5 | Connection loss and a stale snapshot are never replayed: `gates.e2e.ts` fails any state whose status is not `live`, so the page's polling and error states have no gate, no shot and no finding | `gates.e2e.ts` replay test; `live.ts` | unknown, test gap; a monitor that shows old numbers silently would be S2 |
| M6 | UXF-014's "not done" items (canvas labels at 12 px outside the font measure, route names cut at 390 with no full value) were closed with the record and have no open record | `UXF-014.md` last paragraph | S3 |
| M7 | Contact sheets crop the top 768 px of the last state only, so no sheet ever shows In flight, Recent calls, Sessions or the expanded attempts; worst-moment layout-shift entries have no shot (raised in the R-02 and R-03 report reviews, still true in R-05) | `R-05/shots/contact-*`, R-05 `report.md` | S3 (tool) |

## C. Process against the skill

| item | result | evidence |
|---|---|---|
| Rounds: trial, measure, triage, decision, build, gate | R-01 to R-03 complete except the trial (no sessions, no person). R-04 has report, round.json and shots only: no review, no triage. R-05 has one review and no triage, no report review. PR 68's text says "Rounds R-01 to R-04 with reviews and triage": untrue for R-04 | `ls dashboard/ux/rounds/R-0*` |
| Triage formula | applied mechanically and mostly right; errors found by recomputing: R-03 UXF-013 scored frequency 1.00 but its record lists idle and token-flow at 390 = 2 of 4, so priority 0.5 not 1.00; R-03 UXF-012 scored 0.75 but lists 2 scenarios = 0.5; R-02 UXF-005 scored 0.75 but the metric shows 2 scenarios = 0.5 (0.25 with confidence 1). Rechecked and correct: R-02 UXF-002 (3 x 1.00 x 2 / 1 = 6.00), R-02 UXF-007 (3 x 1.00 x 2 / 2 = 3.00, but the two agreeing sources are not named), R-03 UXF-006 (1 x 0.5 x 2 / 1 = 1.00). With the corrected numbers the R-04 set would have taken UXF-004 (1.0) before UXF-013 and UXF-012 (0.5 each) | `R-02/triage.md`, `R-03/triage.md` |
| Change set cap of 5 | held as 5 items per set, but R-04 landed 7 findings (UXF-012 and 014 bundled, UXF-004 closed through the harness fixes) and 5 tool fixes; R-05 changed the whole page (renderer migration, 8,262 added lines, PR 69) outside any change set, so no metric movement of R-05 is attributable | `R-03/triage.md`, PR 69 |
| Failing-first | strong: for UXF-002, 003, 007, 008, 004, 015, 013, 006, 009, 012/014 a `test: … (red)` commit precedes the `fix:` commit and states the red values; nothing stores the red output, and the red proof for UXF-001 is the known-failures entry | `git log` 21f18c6f8 to 8b22d1f5b, b18922c87 to 029659b50, e1718d8a7 to 7e5baf137 and others |
| Ratchet discipline | baseline never got worse (0 decision ids needed). Improvements were taken at the merge of each build branch, not per commit; 4 of 5 rounds entered `main` in one 20,134-line pull request, so per-change gating on `main` did not happen. Improvements inside a tolerance are not taken (layout shift 0.043 stays with tolerance 0.0043). The baseline `about` still cites a scratch round and a page hash older than R-05's | `baseline.json`, `ratchet.ts` `toleranceOf`, R-05 `notes.md` |
| Review agents | R-02 and R-03: both ran, wrote files, found real tool defects (T1-T12, N1-N5). Independence is weak: the report reviewer read the previous round's reviews and triage, and the heuristic reviewer re-judged open findings from the records. R-05: heuristic review only. Whether reviews preceded triage is shown only by commit order (one commit holds reviews, triage and decision) | `R-03/review-report.md` header |
| Findings to tests | enforced: `findings.test.ts` checks fields, evidence lines and tokens, and refuses a closed S1/S2 without an asset naming a declared e2e test, with break tests. In the worktree the three uncommitted R-05 records fail it: UXF-016 and UXF-017 cite tokens that are not on the cited lines (`CellValue.vue:23` has `:class` between the tokens; `style.css:80` has `;`). The review step did not run the findings test | vitest run: 1 failed, 74 passed |
| Three-strike rule | never invoked or recorded. Candidates: harness blind spots found in R-02, R-03 (and the UXF-015 probe fault in R-04), three rounds running; evidence line drift fixed by hand in at least 5 commits before a test existed | commits 7e5baf137, 8fbcca57b, 1368926d3, 9b0b0a0aa |
| Planning | `story:dashboard-ux-loop`, `task:dashboard-ux-r03` and `task:dashboard-ux-r04` are unfilled templates (italic guidance still in the body, status draft). The R-03 task lists UXF-002, 003, 006, 004 and "the token-flow finding", but the R-03 set was 002, 003, 007, 008, 004 (006 belongs to R-04). No R-05 task | `.engineering/planning/` |
| Design decision | one in five rounds (R-03, default taken, confirmed); the skill's template asks for `decision-yes.png` and `decision-no.png` shots: none exist | `R-02/decision.md`, `R-02/shots/` |
| Levels L0, L3, L4-L6, L8 | L0 and gate 6: the skill says drop them when there is no guidance cue, but no file says so. L3 and gate 4 (screen-only user): absent and not mentioned; a monitor has tasks ("which route is failing and why") that a bot could play. L4/L5 (keyed replay, vision probe) need recordings: none exist. L6/L8 (people): no trial, no questionnaire; stated only as a caveat in the R-02 and R-03 report reviews ("no person in the loop"), not in any report.md, triage or trend row. Five rounds ran on one day (about 3.5 hours between the first and last commit), against the skill's "at most 2 a week" with people | `R-03/review-report.md` section 4 item 1 |

## D. Measurement validity

Meets the stated budgets: 221 tests in 60.1 s wall (budget 120 s), slowest 7.56 s (limit 60 s), `globalTimeout` 120 s and a 60 s test timeout in `playwright.config.ts`, slowest ten printed by `slowest-reporter.ts`; vitest 5 s. Deterministic: fonts committed under `e2e/fonts` and installed before the first script; all other requests aborted and `blocked` asserted empty; one stream held open; my run on a different host passed the ratchet against the committed baseline, so values reproduce across hosts.

Remaining weak or vacuous checks:

| check | problem |
|---|---|
| Ratchet as a whole | 51 of 60 baseline values are 0 and 7 more are `1` (flow); only `layout_shift` is non-zero. It is a pass/fail floor; it cannot show progress and has no metric for any of the 8 findings with `metric: none` |
| `truncation.without_full_value` | the stylesheet has no `text-overflow` or clipping, so the check cannot fire on this page; no break test exists for it (the other probes have them) |
| `overflow.scrollers_unreachable` | Chromium focuses a scrolling box by itself, so it reads 0 whatever the markup; the report says so, axe carries the real check |
| `layout_shift.per_state_max` | idle and busy replay constant states (busy is the same fixture three times with the clock moved), so 0 there is true by construction; only token-flow and errors can move. UXF-005's title says "when calls arrive" |
| `key_font` / `secondary_font` | the probe selects by class (`.tile`, `.small`, `.badge`, `main tbody td`); an element outside the lists is neither key nor secondary, and a page with no match reads 0. UXF-004 closed by moving this definition |
| `flow.visible_ratio` | geometry only; reads 1 for an empty panel (documented); labels are canvas text, outside every text gate |
| `reduced_motion.violations` | the stylesheet has no animation, so only the canvas pixel comparison can fire |
| UXF-003 and UXF-014 tests | UXF-003 has no minimum count of boxes; UXF-014 checks the `INK` table, not the paint |
| Copy | no gate reads visible text; R-05 shipped run-together words and a relabelled Provider frame with every gate green |
| Tolerance | layout shift 10 % of the baseline (floor 0.002): a regression of 0.0043 on token-flow passes and an equal improvement is not taken |
| Comparability | R-01 to R-04 were measured with host fonts, R-05 with pinned fonts; the trend table shows 5 rows as if comparable (R-05 notes say so, trend.md does not) |

## E. Outcome

Measured, R-01 to R-05 (trend.md and the reports): gate 1 layout 2 of 8 to 8 of 8 replays per theme; gate 5 axe 0 of 8 to 8 of 8 (12 serious nodes to 0); smallest key-data font 10.4 px to 14 px; flow canvas in view 0.005-0.09 to 1; header gated at 5 viewports (20 of 20); layout shift worst 0.074 to 0.039; Routes shows all 13 columns at the gating viewports; the stale heartbeat, the status word and the caption are in words. These are real, visible improvements and each has a test.

Not shown: any reader task (time to find the failing route, the unhealthy credential, the expensive pod). Every metric is a defect count, so the answer to "better for its reader" is "fewer layout and accessibility defects at two desktop sizes", not "faster to the answer".

What still costs the reader most, in order: (1) the limit block is named by a hash and its values are raw header words (UXF-010, 011); (2) the operational content and the heartbeat alarm sit below a 250 px flow panel and Overview (M1, M2); (3) run-together words in tiles, sessions and in-flight state (UXF-016); (4) at 390 px State is out of view with no cue (UXF-017); (5) labels that mean nothing outside their frame (UXF-018).

## F. Top 5 recommendations

1. Run the whole loop in R-06: two fresh reviewers before the triage, a `triage.md` that recomputes frequency from the record, filled task artifacts for R-03 to R-06, and no page rewrite outside a change set. Moves: process findings in C; UXF-016 class regressions.
2. Fix the failing findings test now (UXF-016 and UXF-017 evidence tokens) and add a copy gate that asserts the visible text of tiles, sessions and in-flight state per scenario, then close UXF-016 red-first, then UXF-010, 011 and 018 with one table of display words. Moves: UXF-016, 010, 011, 018; metric `copy.defects`.
3. Add a screen-only user on two tasks (find the failing route and its cause; read the watchdog state) with actions, looks and position above the fold as ratcheted metrics, and an alert strip at the top. Moves: M1, UXF-010; gives the baseline its first non-zero values.
4. Cut the first-screen cost: collapse the idle flow panel to one row, keep the Provider balance tile on the Overview row at 1280x720, and decide or close UXF-005 with a stated target (0.1 is the web bound it already meets). Moves: UXF-013 second half, M2, M3, UXF-005.
5. Close the measurement gaps: break tests for truncation and the key-data selectors, a minimum box count in the UXF-003 test, a connection-loss replay, per-state shots below the fold, and an edge cue for scrolling tables at 390 px (UXF-017). Moves: M5, M7, UXF-017, `overflow.scrollers` at 390x844.
