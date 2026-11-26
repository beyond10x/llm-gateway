# Report review: R-03

Reviewed: `report.md`, `round.json`, `shots/contact-*`, `../R-02` (its `review-report.md`, `triage.md`, `decision.md`), `../trend.md`, `ux/baseline.json`, `ux/round.ts`, `ux/round-guard.ts`, `ux/ratchet.ts`, `ux/findings.test.ts`, `e2e/*`, the page source. No recordings, no questionnaire. The headline numbers were recomputed by rerunning the hermetic suite and by probes of my own (a loopback server holding one event stream open, Playwright scripts, a copy of the tree with the R-02 page; nothing live, nothing paid, nothing committed). Chromium 158.0.8127.18, axe-core 4.14.1, as in the report.

## Verdict

**Fit to send, with the § 4 caveats.** The numbers are right, the R-02 tool defects are fixed (T1 to T11 fully, T12 partly), and the two new flow metrics work and are discriminating, but they are a guard, not a quality measure. Default action: send R-03 with the § 4 caveats; fix N1 to N5 (§ 3) before R-04; refresh the cited lines of UXF-005, 006 and 009 now.

## 1. Are the numbers right and meaningful?

### Recomputed

| what | result |
|---|---|
| `pnpm e2e --workers=2` (project `gate`) | 152 passed, 82.9 s wall (budget 120 s), slowest test 11.4 s (budget 60 s); includes the ratchet test, so every baseline value is met and no improvement is left untaken |
| `vitest run` | 6 files, 44 tests passed |
| round.json vs its own per-replay data | the gating worst values recompute identically (0 differences over 4 scenarios by 11 metrics); 40 replays, 0 requests aborted, `theme_applied` true in all 40 |
| round.json gating values vs `ux/baseline.json` | identical in every cell (0 differences) |
| per-state layout shift, replay vs a held real stream | token-flow 1280x720 light, token-flow 1366x768 dark, token-flow 390x844 light, busy 1366x768 light: equal state by state to three decimals, and equal to round.json. errors 1366x768 light: replay 0 0.007 0.001 (round 0 0.007 0), held 0 0.008 0; errors 1280x720 dark equal. Differences are at most 0.001 |
| held stream status | `live` at every state, as the replay reads |
| flow probe on the held stream | visible ratio 1 and drawn, in all six cases |
| hard gates | all 8 of 8 in both themes, as the report; flow 2 of 2 per theme |

### Did each change move its target metric

No R-03 triage table exists in the round directory, so I measured it: the R-02 page (the bytes of the R-02 round, sha256 `82ceb6386f06`) under the R-03 harness, all 16 gating replays, against R-03.

| change | target | R-02 page, R-03 harness | R-03 | moved |
|---|---|---|---|---|
| UXF-002 scroll regions named and focusable | `axe.serious_critical` 2 to 0 | idle 1, busy 2, token-flow 2, errors 2 (all `scrollable-region-focusable`) | 0 everywhere | yes |
| UXF-003 tile tables inside their tiles | `overflow.elements` 1 to 0 | 1 in busy, token-flow, errors | 0 | yes |
| UXF-007 Routes table has the full row | new assertion, no metric | not a round metric; the Routes table no longer scrolls at the two gating viewports | `r03.e2e.ts` asserts it | yes, by a test only |
| UXF-008 token flow in its own panel | `flow.visible_ratio` at least 0.95 | 0 to 0.125 in busy, token-flow, errors; 0.31 to 0.375 in idle | 1 in all | yes |
| UXF-004 key data at 14 px | `key_font.deficit_px` 3.6 to 0 | 3.6 (idle 1) | 1.75 (idle 0) | partly: the target 0 is not met, and the finding is closed on a test that defines key data differently (see N4) |
| none targeted | `layout_shift.per_state_max` | token-flow 0.074, errors 0.059 | 0.059, 0.007 | moved by the page with the harness held fixed; the baseline took it; nobody names which change did it |

The R-02 page under the R-03 harness reproduces R-02's headline layout shift (0.074) and its axe count (2 per replay at worst), so the trend rows R-01 to R-03 are comparable on those columns. The axe node total per theme was undercounted in R-02 (token-flow 1 node, now 2 on every state scan), which is the T8 effect and does not change the worst value.

### Does each metric mean what its name says

| metric | verdict |
|---|---|
| `flow.visible_ratio` | The share of a 32 x 16 grid over the canvas box that is in the viewport with no painting element above it (background alpha over 0.5, background image, image, svg, video, canvas, input, button, select, textarea, iframe), measured at the last state, scrolled to the top. It is geometry, not legibility: it does not look at what is drawn, at contrast, or at an overlay with alpha 0.5 or less. A canvas in its own panel at the top reads 1 whether or not anything is drawn (idle: 1 with nothing drawn). The R-02 page gave 0 to 0.375 and R-03 gives 1, so it separates the two states it was built for. |
| `flow.drawn` | True when more than 0.05 % of the canvas pixels differ from the most common colour of a coarse sample. 4 of 4 gating replays in busy, token-flow and errors, 0 of 4 in idle, which is right (the panel says "No active sessions."). It is measured at the last state only; the model is seeded, so it is stable (equal on the held stream: 3.7 % to 9 % of pixels). Under reduced motion the canvas is drawn and static (I checked token-flow, busy and errors at 1366x768): the reduced-motion check itself still does not assert it is drawn. |
| the pair | Read together they answer "is the flow seen". Only token-flow is gated on them; the report row "flow.visible_ratio, smallest 1" for idle and "flow.drawn 0/4" for idle are expected values that no test names as expected. |
| `key_font.deficit_px` | 1.75 is the 12.25 px badge ("vLLM", the second line under a name): the metric counts badges and second lines inside table cells as key data. The idle 14 px is the connection status line. |
| `layout_shift.per_state_max` | Right (above). The tolerance of 0.01 is larger than the errors baseline (0.007) and ten times the run-to-run noise I saw (0.001). |
| `axe.serious_critical` | Now the union of nodes over every state and every action (scans: idle 2, busy 4, errors 5, token-flow 8, equal to states plus actions). Right. |
| `keyboard.focus_invisible` | Now a contrast test of the ring against its backdrop, with opacity. |
| `header.hidden_ratio` | 0 at the gating viewports. At 390x844 it is 0.2 in every scenario: the theme toggle is 20 % outside the viewport. Measured-only, so no shot and no ratchet. |
| `truncation.without_full_value`, `reduced_motion.violations` | 0, as before; the page still has no ellipsis and no CSS animation, so the first cannot fire and the second is the canvas pixel comparison only. |
| ratchet | Worst over the gating viewports and both themes, per scenario and per axe rule. Every improvement is taken: the committed baseline equals the round's gating values in all cells, and the suite's ratchet test fails on an untaken improvement. The two flow metrics are not in the baseline (higher is better; only the 0.95 floor and `drawn` on token-flow are gated). |

## 2. Findings the report misses

1. The theme toggle is clipped at 390x844 in all four scenarios (`header.hidden_ratio` 0.2, "theme toggle" visible 0.8). The report prints it as a measured-only worst moment with no detail ("–"), and the ratchet does not see it.
2. At 1366x768 and 1280x720 the Recent calls table still scrolls sideways in busy (`div.scroll` "Time Route Wire Status TTFT Duration Queue Toke…"); at 390x844 three to five tables scroll, at 960x1080 up to three. `details.scrollers` is in round.json and still not in the report (R-02 § 2 item 3). The fix of UXF-007 covers the Routes table only.
3. The contact sheets still show only the top 768 px at the last state, so no session list, calls table or expanded attempts is on any shot (R-02 § 2 item 5); the flow panel is the one new thing the shots do show.
4. The report has no table of what the change set moved. The one above is what a reader needs; it is not in `report.md` or `triage.md`.
5. `allowed_warning` still breaks mid-word in the status tile ("allowed_warnin" / "g", busy contact sheet, UXF-006, open).
6. Three open findings cite lines that no longer say what the record says: UXF-005 (`App.vue:261` is a table header now), UXF-006 (`style.css:93` is `.theme span`; `.tile .value` is `style.css:164`), UXF-009 (`style.css:122` is `.waiting`; `.stats .alarm` is `style.css:130`). The four closed records and UXF-008 were refreshed and hold.

## 3. Tool defects

R-02 defects, each rechecked:

- **T1** fixed. `e2e/harness.ts` `themeToggleProblems` refuses unless the page shows the other scheme before the choice; three break tests (`e2e/tools.e2e.ts`: same scheme refused, a no-op toggle fails, an unstored choice fails after reload) pass; `gates.e2e.ts` opens the replay under the opposite scheme; `theme_applied` is in round.json and asserted for all 40 replays.
- **T2** fixed for visibility, open for the reduced-motion half. `flowProbe` exists with four break tests (behind panels: drawn and not visible; blank in a panel: visible and not drawn; below the fold: not visible; own panel at the top: passes), and the R-02 page reads 0 to 0.375 on it. `measure.ts` `reducedMotion` still passes a blank canvas (see N1).
- **T3** fixed. Token-flow 1280x720 light with no wait, with a 2.5 s wait before the first state, and on a held real stream: identical per-state shifts (0.022 0.043 0.027 0.008 0.012 0.003 0.004), and the same as round.json. Four more cases equal within 0.001.
- **T4** fixed. The stand-in event stream opens once; status is `live` at every state in all 40 replays and on the held stream; one snapshot fetch; the poll never starts; a failed stream reads otherwise (break test).
- **T5** fixed. Metrics go to `ux/.metrics/<run id>/<set>/`, outside Playwright's output directory, one directory per run (two run directories sat side by side in the tree while I ran the gate; I did not run a round beside it); `readMeasurements` refuses another run's file (unit test).
- **T6** fixed for overwrite and for `--baseline`. `pnpm ux:round R-02` and `R-03` both exit 2 ("already holds report.md, round.json, shots"); `--baseline` is refused for a missing gate replay or a worse value without a decision (unit tests). Remaining: see N5.
- **T7** fixed. `known-failures.ts` holds items (axe rule id plus node, selector), fails on a new item and on a stale one, and the list is empty now; the ratchet counts nodes per axe rule (`axe.rule.<id>`). Unit tests cover a swap of one violation for another.
- **T8** fixed. Axe scans every state and every action; nodes are the union per rule (scan counts above).
- **T9** fixed. `focusStep` tests ring contrast and opacity; the break test covers no ring, a resting transparent shadow, a faint ring, a clear ring, an invisible control (all invisible) and a contrasting ring and the radio's inner ring (visible).
- **T10** fixed. A page without key data reads n/a and a deficit of 0, not 14; the trend column says it is gating-only. Idle now reads 14 px because the status line counts.
- **T11** fixed. The report names Chromium, axe-core and its Playwright package, Playwright and a harness commit (marked `+changes` when `dashboard/e2e`, `dashboard/ux`, the config or the lock differ). Not recorded: operating system and fonts (the page uses `system-ui`). Round.json equals a local rerun and a rerun on the build host at the gate, so this has not bitten.
- **T12** partly fixed. `ux/findings.test.ts` now checks that every cited file exists and every cited line is within the file (break test), and that the regression asset names a test declared in `e2e`. It cannot see a line that moved but is still in range, which is what happened to UXF-005, 006 and 009 (§ 2 item 6).

New defects:

- **N1.** `e2e/gates.e2e.ts`, `ux/baseline.json`: the flow metrics are not ratcheted and are gated on the token-flow scenario only. A busy or errors page with a blank canvas passes every gate, and so does a reduced-motion replay whose canvas is blank (`measure.ts` `reducedMotion` compares pixels only). Input: blank the canvas in busy.
- **N2.** `e2e/round-report.e2e.ts` and `e2e/measure.ts`: `details.scrollers` (tables that scroll sideways) is collected and never reported or ratcheted; the toggle clipped at 390x844 is in the measured-only list without detail. Input: `round.json` busy 1366x768, `details.scrollers` has one entry; 390x844 `header` has the toggle at 0.8.
- **N3.** `ux/ratchet.ts` `TOLERANCES`: `layout_shift.per_state_max` tolerance 0.01 against a baseline of 0.007 (errors) lets a 140 % worse shift pass, with noise at 0.001. Input: errors baseline 0.007, replay noise 0 to 0.001.
- **N4.** `ux/findings/UXF-004.md`, `e2e/r03.e2e.ts` `fontDeficits`: the regression test defines key data as 14 px and secondary text as 12 px and passes, while the metric defines key data to include badges and second lines and stays at 1.75; the finding is closed with its metric target (0) unmet, stated in the record but not in `report.md`. `ux/findings/UXF-008.md` still names a 5 x 5 grid; the probe uses 32 x 16 and `r03.e2e.ts` has a third hit test of its own.
- **N5.** `ux/baseline.json` `about`: the baseline says it comes from "a scratch round ... its round directory is not kept", so it cannot be traced to the committed R-03 round, although the values equal it. `trend.md` stamps R-03 with 12 characters of the commit, R-01 and R-02 with 8.

## 4. Caveats to send with the round

1. Scenarios are synthetic and a person was not in the loop: nothing speaks to task time or comprehension. Three of four scenarios are anchors.
2. What R-03 moved, measured on the R-02 page with the same harness: axe 2 to 0, overflow 1 to 0, flow visible 0 to 0.375 up to 1, key-font deficit 3.6 to 1.75 (target 0 not met), layout shift down in token-flow (0.074 to 0.059) and errors (0.059 to 0.007), which no change set targeted.
3. `flow.visible_ratio` is geometry in a box; it reads 1 for an empty panel. Read it with `flow.drawn`. It says nothing about whether the labels and strings can be read (UXF-014 is open).
4. Layout shift and axe numbers equal a held real stream at the gating viewports (light and dark spot checks); measured-only viewports have no screenshot.
5. At 390x844 the theme toggle is 20 % outside the viewport in every scenario, and tables scroll sideways at 390x844, 960x1080 and, in busy, at both gating viewports (the Recent calls table). Neither is a gate or a ratchet value today.
6. The suite step takes 83 s on two workers, within its budget.
