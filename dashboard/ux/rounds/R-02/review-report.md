# Report review: R-02

Reviewed: `report.md`, `round.json`, `shots/`, `../R-01`, `../trend.md`, `baseline.json`, `ux/round.ts`, `ux/ratchet.ts`, `e2e/*`, the page source at the round's commit. No rrweb recordings and no questionnaire exist for this round; the numbers were recomputed by rerunning the hermetic suite (an unchanged copy of the tree, page sha256 `82ceb6386f06…` as in the report) and by small probes of my own (a local loopback server, a Playwright script; nothing live, nothing paid, none committed). Chromium 158.0.8127.18, axe-core through `@axe-core/playwright` 4.14.1.

## Verdict

**Not fit to send as is.** The numbers are right; two claims built on them are not sound (the theme-toggle assertion cannot fail, and nothing measures whether the token-flow canvas is seen), and the report does not say that no metric moved between R-01 and R-02. Default action: send R-02 with the § 4 caveats at the top of `report.md`, keep UXF-001 closed only after its regression test is made discriminating (T1), and fix T1 to T4 before R-03.

## 1. Are the numbers right and meaningful?

### Recomputed

| what | result |
|---|---|
| `pnpm e2e` (project `gate`, 2 workers) | 118 passed, 46.2 s wall, slowest test 7.1 s (within 60 s and 120 s); 28 of them are expected failures held by UXF-002 and UXF-003 |
| `pnpm ux:round R-02` | all 40 replays measured, 114 to 122 s wall (a first attempt reported "round measured 2 of 40 replays", see T5) |
| round.json vs my rerun | 39 of 40 replays identical in every metric and every detail list. One differs: errors 1280x720 dark `layout_shift.per_state_max` 0.059 in the report, 0.060 in the rerun (noise; the ratchet tolerance is 0.01). The report differs by that cell (and the row built from it) and by the commit stamp (my copy has no git history) |
| hard gates from the per-replay data, by my own count | gate 1 layout 2 of 8 per theme (the two idle replays), gate 5 axe 0 of 8 and 12 nodes per theme (idle 1+1, busy 2+2, token-flow 1+1, errors 2+2, all `scrollable-region-focusable`), keyboard 8 of 8: all equal the report |
| R-01 vs R-02 | every metric at the 20 gating replays is equal. Only the 390x844 layout shift moved (errors 0.029 to 0.027, token-flow 0.102 to 0.098, measured-only, from the added toggle wrapping the header) |
| real stream vs replay | see below: the numbers hold |

### Does each metric mean what its name says?

| metric | verdict |
|---|---|
| `axe.serious_critical` | Right for the two states it scans. axe runs on the first state and the last state (and after each action), not on the states between. The count is the largest node count per rule over the scans, summed over rules: two different elements failing in two scans count as one. One rule only is present today. |
| `overflow.elements` | Counts distinct elements, once each over all states, not how many pixels or how much text. "1" in busy is the Cache hit rate mini table printing over the Throughput tile's text (`contact-1366x768-dark.png`, token-flow: "claude-haiku 62%" lands on "in 109 tok/s"). The number does not say that two pieces of text are unreadable. |
| `overflow.scrollers_unreachable` and `keyboard.unreachable` | 0, but only because Chromium makes a scrolling container focusable on its own. axe says the same containers have no keyboard access (UXF-002, 12 nodes). The report prints "0 scrollers without keyboard access" in gate 1 and "12 serious/critical nodes (scrollable-region-focusable)" in gate 5 two lines apart: a reader sees a contradiction. The walk measures the browser, not the markup. |
| `keyboard.focus_invisible` | "Visible" means computed `outline-style` is not `none` or any `box-shadow` is set on the focused element. It does not compare with the unfocused element, does not check opacity or colour contrast. The theme toggle's radio inputs are `opacity: 0`; they pass because a global `:focus-visible` rule styles the invisible input, not because the ring on the label is seen (the ring exists, so the outcome is right for the wrong reason). |
| `key_font.deficit_px` | The smallest font among key data, so one badge sets the value: 10.4 px is the vLLM badge and the grey second line, 13 px in idle is the connection status text. It says nothing about the share of key data under 14 px. Idle has no key data and reports the status line as its key data. If a page had none, `key_font_min_px` would be 0 and the deficit 14 (`measure.ts`). |
| `layout_shift.per_state_max` | Matches the real stream (below), but only by accident of order, see T3. |
| `reduced_motion.violations` | The page has no CSS animation and no transition (`src/style.css`; only the reduced-motion override block), so the CSS half of the check cannot fail on this page. The canvas half compares `toDataURL` before and after 500 ms: a blank canvas passes (idle's canvas has no lit pixel in any state), and a canvas nobody can see passes (below). The only break test covers a moving canvas in light theme. |
| `truncation.without_full_value` | The page uses no `text-overflow: ellipsis` and no `overflow: hidden` on text, so 0 is true by construction and the check never fired. What the page does instead is wrap ("allowed_warni" / "ng", UXF-006) and scroll, and neither is measured. |
| `header.hidden_ratio` | The header is `position: sticky`; the check cannot fail unless something covers it. Fine as a regression guard, not as evidence. |
| gate "2 of 8" and "8 of 8" | The 8 is 4 scenarios by 2 gating viewports in one theme. "2 of 8 pass gate 1" is the two idle replays. |
| ratchet | Compares the worst value over gating viewports and themes; zero tolerance on counts. A replaced violation of the same count passes; the two `test.fail` markers (UXF-002 on every scenario, UXF-003 on three) pass whatever fails inside the gate, not only the named finding. |

### The polling fallback after each fulfilled response

The report states it ("polling every 5 s in every shot"). I checked whether it distorts the numbers: a local server held one `dashboard/events` response open and pushed each state into it, as the gateway does; the same page, scenario, viewport and theme (light) were measured next to the replay.

| scenario, viewport | per-state shift, replay in the round (with the harness's own start-up) | per-state shift, held stream | axe rules and nodes |
|---|---|---|---|
| token-flow 1280 | 0.074 0.051 0.059 0.016 0.016 0.007 0.016 | 0.074 0.051 0.059 0.016 0.016 0.007 0.016 | 1 node, equal |
| token-flow 390 | 0.098 0.079 0.067 0.023 0.034 0.010 0.017 | 0.098 0.079 0.067 0.023 0.034 0.010 0.017 | 4 nodes, equal |
| errors 1280 | 0.001 0.059 0.004 | 0.002 0.060 0.002 | 2 nodes, equal |
| errors 390 | 0 0.027 0.001 | 0 0.027 0.001 | 4 nodes, equal |
| busy 1280 and 390 | 0 0 | 0 0 | 2 and 4 nodes, equal |

So the replay's numbers equal the live stream's at the gating viewports. What differs and the report only half says:

- The status text. Replay: "polling every 5 s" (warning colour) in every state; real stream: "live" (ok colour). Every screenshot a person looks at shows a warning in the header that a real user does not see. The 13 px key-font reading in idle is the same text size either way. axe was compared in light theme only, so the contrast of the `live` colour in dark theme was not scanned.
- The status line flips live, polling, live per state in the replay and stays `live` in the real stream: a polite live region announces on every state in the replay and never in reality.
- The page polls `dashboard/snapshot` every 5 s while a replay runs (busy takes 7 to 8 s), re-applying the current state; harmless here, and a source of the one-cell noise above.
- States arrive about a second apart in the replay and 4 s apart on the scenario's clock; countdowns jump.

### Does a canvas fixed behind opaque panels count as "visible"?

No metric counts the canvas as visible; none looks at it. The layout probe skips fixed canvases (overflow); the reduced-motion check counts any canvas whether or not one pixel of it can be seen; the break test proves the canvas moves, not that it is seen. I measured how much of it can be seen: the share of non-transparent canvas pixels that no element with an opaque background covers (`elementFromPoint` plus computed background), 1366x768, light, last state, sampled every other pixel.

| scenario | canvas drawn (share of viewport) | of that, not covered by a panel | uncovered, share of viewport |
|---|---|---|---|
| idle | 0 | none | 0 |
| busy | 2.2 % | 0.5 % | about 0.01 % |
| token-flow | 1.7 % | 9.1 % | about 0.15 % |
| errors | 1.1 % | 3.6 % | about 0.04 % |

With reduced motion the values are the same within a point. In the contact sheets the flow lines show as a few specks in the gaps between panels. R-02's commit message and the trend row ("reduced motion holds with the canvas") are literally true and say nothing about the feature being seen. For a feature whose purpose is to be seen, the round has no metric: this is the main thing R-02 does not measure.

### Comparability with the last round

R-01 measured a page with no canvas, no skip link, no toggle. R-02 measured the merged page, and every gating number is identical. The report never says what changed in the page, or that the new header control and the full-viewport canvas moved no number. R-01's reduced-motion pass on a page without a canvas was vacuous; R-02's is the first with a canvas, still blank in idle. The baseline is R-01's and was not rebased (its values still hold).

## 2. Findings the report misses

1. The token-flow canvas is 91 to 99.5 % hidden by the panels at 1366x768 (table above). Class cue/layout. Metric to add: `flow.visible_share`.
2. The theme toggle assertion cannot fail (T1). UXF-001 is closed on it.
3. At 1366x768 the Routes table scrolls sideways in every scenario, idle included, and the In-flight table in busy and errors (these are exactly the axe nodes: 1, 2, 1, 2). The columns "Cache hit" and after are off screen (`contact-1366x768-dark.png`, token-flow Routes header clipped at "Cac", idle clipped at "L"). Gate 1 reports "0 scrollers without keyboard access" and no count of scrollers that scroll. `details.scrollers` is collected in `measure.ts` and never reported.
4. The Cache hit rate tile text collides with the Throughput tile text (not only "past box"); the 390x844 and 960x1080 numbers (axe 3 to 4, past box up to 5) are the worst of the round and have no screenshot: shots are taken only at the gating viewports, and the "Worst moments" list mixes measured-only viewports with the gating-only table above it.
5. The contact sheets show only the top of the page at the last state (no sessions, calls table, expanded attempts), 0.75 s after the last state, with the warning status.
6. `blocked_requests` (the proof the run is hermetic) is written to the per-replay file under `test-results/` but not to `round.json`, not to the report and not asserted: I found 0 in all 40, the report could say so.
7. UXF-002, 003, 004, 006 cite source lines of the R-01 page; after the merge the lines moved (for example `.scroll` divs in `App.vue` 131, 164, 189 not 125, 158, 183; `.scroll` in `style.css` 124 not 64; `.tile .value` 153 not 93; `.badge` 138 not 78; root font 19 not 16).
8. `report.md` itself names no finding for the new toggle and canvas; the round's only findings are R-01's (the heuristic review files its own).

## 3. Tool defects

- **T1.** `e2e/harness.ts` `openReplay` emulates the colour scheme and then clicks the toggle to the same theme, so `theme_applied` is true for a toggle that does nothing (the replay is already in that scheme); `gates.e2e.ts` "affordance: a theme toggle…" and UXF-001's regression asset cannot fail. I checked the toggle itself works: under emulated dark, "Light" gives `data-theme=light` and a light background, "System" gives dark. Fix: emulate the opposite scheme (or none) before the click. Also `theme_applied` is not in `round.json` or the report.
- **T2.** `e2e/probe.ts` `canvasPixels` and `measure.ts` `reducedMotion`: the canvas check passes for a blank canvas and for a canvas nobody can see (idle: no lit pixel; busy: 99.5 % covered). No visibility metric exists; the `break:` test in `e2e/measure-gate.e2e.ts` covers only motion, only token-flow, only light.
- **T3.** `e2e/measure.ts` `measure` and `e2e/harness.ts` `advance`: the per-state layout shift depends on the time since page load. Token-flow 1280x720, same states: 0 0 0 0.016… with no wait, 0 0.051 0.059… after 300 ms, 0.074 0.051 0.059… after 2 s or after an axe run. The round gets the real numbers only because `measure` scans (probe and axe, about 1 s) before the first state. Remove that scan and the first states read 0, the ratchet then says "improved-not-taken", and the lower baseline is taken. Fix: wait for a settled load (or a fixed minimum) inside `openReplay`; add a test that compares to a held-stream run.
- **T4.** `e2e/harness.ts` `advance`: each state is a complete response, so the page flips live, polling, live and the `open` event of the held reconnect never fires; the status reads "polling every 5 s" in all shots, and the 5 s poll runs in the middle of a replay. Numbers still match the held stream (§ 1); the shots and the live region do not. Fix: hold one streaming response (a loopback server as in my probe) or hide the status text in shots.
- **T5.** `e2e/metrics-dir.ts`: metrics live under the project's `outputDir` (`test-results/`), which Playwright empties at the start of every run. A second Playwright run in the same tree during `ux:round` made the first end with "round measured 2 of 40 replays". Loud, not silent, but one `pnpm e2e` beside a round loses the round. Use a per-run directory.
- **T6.** `ux/round.ts` and `e2e/round-report.e2e.ts`: re-running `ux:round R-02` overwrites `rounds/R-02/` in place (I did it in my copy); a committed round is not immutable. `--baseline` writes the baseline from any round without checking the ratchet.
- **T7.** `e2e/known-failures.ts` with `gates.e2e.ts` `gate()`: `test.fail` marks a whole gate. UXF-002 is on `gate5-axe` for every scenario and UXF-003 on `gate1-overflow` for three; any new, different axe rule or overflow in them still passes. The ratchet counts nodes only; a swap of one violation for another passes. Hold the expected rule ids and element paths, and fail on a new one.
- **T8.** `e2e/measure.ts` `addAxe`: scans only the first state, the last state and after each action; takes the largest node count per rule instead of the union of targets. The states between are never scanned.
- **T9.** `e2e/probe.ts` `focusStep`: "visible focus" is `outline-style !== none || box-shadow !== none`; any resting box-shadow counts; opacity and contrast are not looked at.
- **T10.** `e2e/measure.ts` and `round-report.e2e.ts`: `key_font_min_px` is 0 when there is no key data, so the deficit is 14 and the trend takes the minimum 0; the trend's "key font px" and "Worst moments" mix gating-only and all-viewport bases without saying so.
- **T11.** `e2e/round-report.e2e.ts` `commit()`: stamps HEAD at run time and marks only `dashboard/src` and `dashboard.html` as dirty; the harness version, Playwright, Chromium and axe-core versions are not recorded, so a drift in the browser or axe rules cannot be told from a drift in the page. Without git history it writes `unknown` (my copy).
- **T12.** `ux/findings.test.ts`: checks the regression asset exists, but not `evidence_lines`; the four records above went stale at the merge. It also lets UXF-001 stay closed on the test of T1.

## 4. Caveats to send with the round

1. R-02 is the first round on a page with the toggle, the skip link and the token-flow canvas, and no gating number changed. That means the new features are not measured, not that they are good.
2. The theme-toggle assertion passed because the replay was already in the requested scheme; do not read it as proof the toggle sets the theme (it does, by hand check) until T1 is fixed. UXF-001 stays closed on that basis only provisionally.
3. The canvas is almost entirely behind opaque panels at 1366x768 (0.01 % to 0.15 % of the screen uncovered); "reduced motion holds with the canvas" does not mean the flow is visible.
4. "0 scrollers without keyboard access" and "12 axe nodes" describe the same containers: Chromium focuses them, markup gives no access. The Routes table needs sideways scrolling at 1366x768 in every scenario.
5. Every screenshot shows "polling every 5 s" in warning colour; a real stream shows "live". Layout shift and axe numbers were checked against a held stream and match (light theme only).
6. The layout-shift number is right because of the order of steps in the harness (T3); it is not robust.
7. No recordings, no questionnaire, no person in the loop: nothing here speaks to task time or comprehension. The anchors are three scenarios of four, all synthetic; measured-only viewports are not ratcheted.
8. Reproduced on rerun: 39 of 40 replays equal, one cell off by 0.001.
