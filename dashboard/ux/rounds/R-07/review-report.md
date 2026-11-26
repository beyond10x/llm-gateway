# Report review: R-07

Reviewed: `report.md`, `round.json`, `shots/`, `../R-06` (triage.md, review-report.md, round.json), `../trend.md`, `ux/baseline.json`, `ux/round.ts`, `ux/ratchet.ts`, `ux/round-guard.ts`, `ux/findings.test.ts`, `ux/tools.test.ts`, `e2e/copy.ts`, `e2e/screen-only.ts`, `e2e/probe.ts`, `e2e/canvas-text.ts`, `e2e/measure.ts`, `e2e/gates.e2e.ts`, `e2e/r07.e2e.ts`, `e2e/r06.e2e.ts`, `e2e/r06-problems.ts`, `e2e/tools.e2e.ts`, the five scenarios. No recordings and no questionnaire exist for this round. Everything ran hermetically (fake origin, loopback, nothing live, nothing paid); every browser job went through the slot pool, none longer than the pool's limit, no test over 60 s. My own probes and the copies of the tree I measured in are scratch files outside the repository; nothing is committed and no code was edited.

## Verdict

**Fit to send, with § 4.** The headline numbers reproduce: 334 of 334 gate tests, and 49 of the 50 replays of the round equal on every metric when the round is measured again (the 50th is a probe race, § 1). All five R-07 changes moved their target, measured by running the R-06 page under the R-07 harness. All of T18 to T24 are fixed, with the residues listed in § 3. The two named metrics mean what they say within stated limits. Two things are not clean: the measured-only reduced-motion cell is a probe artefact that varies from run to run, and the new scroll cue costs layout shift at 390x844 that nothing gates. Default action: send R-07 with § 4, take T25 and T26 before R-08.

## 1. Are the numbers right and meaningful?

### Recomputed

| what | result |
|---|---|
| `pnpm e2e --workers=2` (project `gate`), quiet host | 334 passed, wall 68.4 s (74.9 s with the sync to the build host), slowest test 4.3 s (a measured replay), none over 5 s |
| the same, a second time, host load average 17 to 34 and my own probes running beside it | the run hit its 120 s limit with 103 tests done (slowest 9.3 s). The limit works; the step's margin is thin on a loaded host (R-06 review said the same). A third run on the same loaded host (load 18 to 25) passed: 334 passed, wall 115.2 s, slowest 7.3 s, ten slowest all measured replays |
| the 20 gating replays of the first run against `round.json` | identical in every metric, in the screen-only results (reached, actions, looks) and in the copy items; page sha256 `a07c861bbc3a` equals the built page |
| `ux:round` measured again on the same page (scratch copy, all 50 replays, five viewports) | 49 of 50 replays equal on every metric. The one difference is `reduced_motion.violations` at 960x1080: R-07 reports it for offline dark and token-flow light, my run for token-flow light only. See below |
| `pnpm exec vitest run` | 86 of 86 |
| ratchet | passes (`every value equals the baseline`), also on the first run |

### Did each R-07 change move its target? (R-06 page under the R-07 harness)

I built the R-06 page (page commit of R-06, sha256 `249874c33fcb`) behind the R-07 harness in a scratch copy and ran the round command and the R-07 and R-06 specs on it, then ran the round on the R-07 page the same way. Same tools, only the page differs.

| change | target | R-06 page, R-07 harness | R-07 page | moved |
|---|---|---|---|---|
| 1 UXF-021 headings 14 px | `heading_font.deficit_px` to 0; known failure removed | 1.4 (busy, errors) and 0.7 (idle, token-flow, offline), at every one of the 50 replays; key-data gate 0 of 10 in both themes | 0 at all 50; gate 10 of 10; `KNOWN_FAILURES` is empty | yes |
| 2 UXF-026 failed-calls line | screen-only "failing route" glance 1 with 0 actions in busy and errors | busy 2 actions glance 3; errors 1 action glance 3; at 390x844 and 960x1080 not reached at all (7 and 4 actions in busy, 4 and 2 in errors); both r07 tests red | 0 actions glance 1 at all ten (scenario, viewport) rows including 390x844 and 960x1080; tests green | yes, and more than the report says: the measured-only viewports are in `round.json` but not in the report's screen-only table |
| 3 UXF-018 Provider frame | assertion on the heading and the labels | red (heading and labels) | green | yes (assertion only; no metric) |
| 4 UXF-020 balance tile on the Overview row | assertion on tile rows at the gating viewports | red at both viewports, both scenarios | green | yes (assertion only) |
| 5 UXF-017 scroll cue | assertion that each scrolling region shows its cue | red (busy, errors) | green | yes (assertion only). No metric moves: `overflow.scrollers.<viewport>` counts scrollers and is unchanged by the cue (the one change, errors 960x1080 from 3 to 2, comes from the Routes padding, not the cue) |

Two notes. The R-06 report gave errors' failing-route glance as 2; under the fixed tool (T23) the R-06 page scores 3, so the true movement is 3 to 1, and R-06's number flattered its page, as the R-06 review said. And the R-06 UXF-014 canvas change, which no metric could see in R-06 (T22), is now seen: the new canvas assertion fails on the R-05 page, 3 of 3 (labels drawn at 12 px; route "claude-sonnet" cut to "claude-son…") and passes on R-06 and R-07.

Other metrics that differ between the R-06 page and the R-07 page under the same harness, none of them targets: `layout_shift.per_state_max` errors 1280x720 0 to 0.002 and 390x844 0 to 0.016, offline 390x844 0.012 to 0.017 (worse), offline at the three wide viewports 0.001 lower; `overflow.scrollers` errors 960x1080 3 to 2. The shift at 390x844 is not ratcheted (the ratchet takes the worst of the gating viewports) and is not in the report's worst moments. See § 2.

### Are T18 to T24 fixed? (the input used for each)

| | status | input and result |
|---|---|---|
| T18 key data by class name | fixed, residue T25 | a page with a metric whose value class was renamed to `.metric-value` at 12 px and its label declared in the document: key deficit 2 px, found. On the real page: every text of busy (750 elements) and errors (364) set in turn to 12.5 px, and the probe asked whether key or heading data falls short. Every table body cell, definition, tile value, status and heading is caught. Not caught: labels, column heads, notes (secondary by design) and the Problems strip links and pills (see T25) |
| T19 improvement inside the tolerance | fixed | `compare` reports any improvement; the test feeds the R-06 review's input (baseline 0.003, measured 0.001) and gets "better than the baseline: take it". Not flaky in the two quiet measurements I made |
| T20 copy judged by markup | fixed | `<div style="display:flex"><span>1.88k tok/s</span><span>last 60 s</span></div>`: item. `<span>alpha</span><span style="margin-left:8px">beta</span>`: none. Also checked: a real space with negative letter spacing, with two font sizes or with `space-between`: none; a 2 px inline-block gap, a flex gap of 0.1 em, "7m" then "ago", "12" then "days": items |
| T21 first run ending in `%`, `)`, `]` | partly fixed | `<span>12%</span><span>errors</span>` and the parenthesis and bracket cases: items. A first run ending in `:`, `,`, `;` or `.`, or a second starting with `$`, `(` or a quote, is still never compared (T27) |
| T22 flow labels asserted on the model | fixed | the canvas assertion fails on the R-05 page (3 of 3) and passes on R-06 and R-07 (above). The canvas is transparent, so the "painted" check is not vacuous |
| T23 screen-only | fixed, residue T28 | a link to a section 1500 px down (page jumps 1500): actions 1, looks 3 and the step reads "(the page jumped)"; truth in view: 1; a wheel scroll: 3. Text at opacity 0.3, the colour of its background, `#ccc` on white, cut by an ellipsis, indented off screen, covered by an overlay, drawn in a canvas, clipped for screen readers, dark on dark: not reached |
| T24 report gaps and slow connection tests | fixed | the key-data row now says "no failing replay is held by a known failure"; worst moments exist for every metric (a unit test feeds all of them); the two connection tests no longer wait 5 s (the slowest test of the step is 4.3 s, down from 6.1 s) |

### Do `copy.defects` and `screen_only` mean what their names say?

**`copy.defects` (by geometry).** It now means: two consecutive text runs whose boxes sit on one line and touch (gap under 0.2 em of the first run's size), where the first ends in a letter, digit, dash, `%`, `)` or `]` and the second starts with a letter or digit (a letter after a closer), a number and its unit and a superscript excepted; plus a raw identifier or header name as a label, a hex title, and a control named by a glyph. That is what the eye reads as a word, not the markup. Limits found:

- Recall on the real page by mutation: of the 16 whitespace characters that touch an inline run in busy (13 in errors, 8 in token-flow), removing each in turn is detected for 2. The rest are not defects (the space ends a block, a heading, a button's own text) except four that are the punctuation gap of T27: the space after "dots:" and after "right," in the flow caption, with the next run in another element. Removing those would run "dots:input tokens" together and the probe would stay silent.
- A zero for `copy.defects` therefore means "no touching runs of the listed classes", not "no copy defect". It is a name wider than its test, as in R-06.

**`screen_only` (jumps counted, readable text only).** `looks` is now the glance the person had: 3 when the bot scrolled or a click moved the page by more than half a viewport. Verified on a link that jumps 760 px (3). Not what the name says in two corners: a jump shorter than half a viewport, for example a link whose target the page can only scroll 280 px towards, scores 2 for text that was off the first screen. And the readability test reads colours through a canvas parse that does not understand `color()`, `oklch()` or `color-mix()`, and ignores gradients and font size: white text written as `color-mix(in srgb, #fff, #fff)` on white, or `oklch(0.99 0 0)`, or a 1 px font counts as read. The page uses none of these (checked in its CSS and document), so the numbers stand, but the guard is weaker than its description (T28). It plays only the gating viewports in the light theme in the report; `round.json` also holds 390x844 and 960x1080, where the failing route is now reached with no action.

### The measured-only reduced-motion violation at 960x1080 (offline dark, token-flow light)

**A probe artefact, not a motion a person can see; its source is real.** The page's reduced-motion rule sets `transition-duration: 0.01ms !important` on every element. Every property change then creates a CSS transition of 0.01 ms (the page declares no transition of its own, so with motion allowed none exists). `motionProbe` flags any transition whose duration is above 0 that is still "running" when it samples, right after a state arrives. Whether one is caught is a race with the next frame.

Evidence: R-07's round flags offline dark and token-flow light at 960x1080; my second measurement of the same page flags token-flow light only, and nothing else differs between the two runs. Six reduced-motion replays each of the same state sequence, on the R-07 page, with the animation's target printed: token-flow light 960x1080 0 of 6; busy light 960x1080 0 of 6; offline dark 960x1080 1 of 6 (`color` on `span.link`, `background-color` and `color` on `span.dot`); errors dark 390x844 3 of 6 (opacity on the `::before` and `::after` of `div.scroller`, background on `tr`, border colours on `button.expand`). The report prints "(0 ms)" because 0.01 ms rounds to 0. R-06 reported 0 everywhere; the R-07 scroll fade (opacity of the two pseudo-elements, UXF-017) adds candidates at the viewports where tables scroll, which is why it shows up now. At the gating viewports nothing scrolls and no violation appeared in 20 replays of the round, 20 of my own and 20 of the round I repeated. It is not ratcheted at the measured-only viewports, but the same race could turn the gate red at a gating viewport on a loaded host (T25).

### Is any gate vacuous?

None is vacuous; four are weaker than their rows say.

- **Gate 4 (screen-only)** hard-gates `reached` only; the row's "actions 0 max, glance 1 max" are ratcheted values, not a gate. With the failing-route line on the first screen the task needs no action, so the gate can only go red if the line or its words leave the first screen. That is the intent, but it is a check of the page against a truth written beside it.
- **Key data** is not vacuous (above). It floors the Problems strip's links and pills at 12 px, not 14: they are "every other text" (T25), while headings, cells, definitions, tile values and the status are at 14.
- **Reduced motion** is race-prone (above).
- **The UXF-026 test** compares the strip line with the Routes column, and both come from one function of the page (`recentByRoute`), so it proves the page agrees with itself. The only independent checks are the route and cause words the scenario declares. The tools' own kind check (`e2e/r06-problems.ts`, `kindsOf`) still defines a failed call as `outcome != ok || status >= 400`, without the page's new exclusion of a call the client left. No scenario holds a failed-and-cancelled call in the window (checked), so the two definitions agree on every input the suite has and the cancelled rule is not exercised end to end (T29).
- The copy gate (limits above) and the ratchet (gating values only) are as in R-06.

## 2. Findings the report misses

1. **The scroll cue costs layout shift at 390x844.** `ScrollRegion` inserts the cue line after mount (the table's width is measured in `onMounted`), which moves what is below it. Same harness, R-06 page to R-07 page: errors 390x844 0 to 0.016, offline 390x844 0.012 to 0.017. The report's "all viewports" column shows 0.016 and 0.060 without saying which is new; the ratchet and the gates do not see the viewport. A reserved line before measurement would remove it. It belongs with UXF-005.
2. **The failing-route task is now reached with no action at 390x844 and 960x1080** (it took 7 and 4 actions, and was not reached, on the R-06 page). That is the largest user-facing gain of UXF-026 and the report lists only the gating viewports.
3. **The Problems strip is outside the 14 px floor** (T25), the strip being the thing UXF-019 and UXF-026 built.
4. **Routes now carries three counts side by side** ("Failed, last 5 min", Calls, Errors) with the two older ones explained only on hover (the document's `help`).

## 3. Tool defects

T18 to T24 are rechecked above (all fixed; residues are T25, T27, T28). New:

- **T25. `e2e/probe.ts` `motionProbe` / `e2e/measure.ts` `reducedMotionReplay`**: counts a 0.01 ms transition the page's own `transition-duration: 0.01ms` rule creates as running motion, and samples it racing the next frame. Input: the R-07 page, token-flow light at 960x1080 reduced-motion replay, repeated: 0 of 6 here, 1 in the round; errors dark 390x844: 3 of 6; the report prints "(0 ms)". Fix: ignore durations under 1 ms, or sample after one frame.
- **T26. `e2e/probe.ts` `keyDataProbe`**: a metric is found only when its label is one of the document's `label:` strings (`e2e/document.ts`). Input: `<div><span>Spend</span><span class="metric-value" style="font-size:12px">42</span></div>` with `Spend` not declared: key data none, the 12 px value passes as secondary. A label built by the binding rather than declared in the document, or a value outside a table cell, definition, status or metric (`<p style="font-size:11px">1.9k tok/s</p>`, a `div` used as a title) falls to the 12 px floor. On the real page the Problems strip links and the `error` and `warning` pills are in that bucket.
- **T27. `e2e/copy.ts` `run-together`**: only a first run ending in a letter, digit, dash, `%`, `)` or `]` is compared, and only against a second starting with a letter or digit. Input: remove the space after "dots:" or after "right," in the token-flow caption on busy: no item. Also `cost` then `$3.20`, `alpha.` then `beta`.
- **T28. `e2e/screen-only.ts` `truthInView` / `playTask`**: (a) colours go through a canvas parse that reads `color(srgb 1 1 1)`, `oklch(...)` and `color-mix(...)` as near black: white text on white written that way is read (input: `<p style="color:color-mix(in srgb,#fff,#fff)">claude-sonnet 429</p>`, reached true, glance 1); a gradient behind text is ignored, and so is font size (1 px text reached); (b) a jump of less than half a viewport sets no scroll (input: a page 1000 px tall with the truth at 760 and a link to it: actions 1, looks 2).
- **T29. `e2e/r07.e2e.ts` UXF-026 and `e2e/r06-problems.ts`**: see § 1, "Is any gate vacuous". The count comparison is the page against itself, and the two definitions of a failed call differ.
- **T30. `e2e/round-report.e2e.ts`**: the screen-only table and "Worst moments at the measured-only viewports" skip what `round.json` holds (the measured-only screen-only rows, which carry the largest change of the round; worst moments only name three metrics there). `ux/rounds/trend.md` has no column for key data, copy or screen-only, so the R-07 row equals R-06's in every column it has, and the round's movement is invisible in it (the R-06 review's finding 1, still open).

## 4. Caveats to send with the round

1. Synthetic scenarios, no person in the loop: nothing here speaks to task time or comprehension.
2. `reduced_motion.violations` 1 at 960x1080 (offline dark, token-flow light) is a probe race on the page's 0.01 ms idiom, not a motion; it moves from run to run. The gating values are 0.
3. `copy.defects` 0 means no touching runs of the listed classes and no raw identifier, glyph control or hex title. It does not see punctuation then a word (`:`, `,`, `.`), a `$` amount or a parenthesis opening the second run, and not canvas text.
4. `screen_only` is the glance by the bot's scrolls and jumps over half a viewport, with a readability test that ignores `color()`, `oklch()`, `color-mix()`, gradients and font size. The new failing-route line makes every task glance 1 with 0 actions at every viewport, including 390x844.
5. Movement of UXF-018, UXF-020 and UXF-017 is shown by their assertions, not by a ratcheted metric; `overflow.scrollers` does not move with the cue.
6. The scroll cue adds layout shift at 390x844 (errors 0 to 0.016, offline 0.012 to 0.017), measured only, not ratcheted.
7. The step's wall time was 68 s on a quiet host and ran into the 120 s limit under load average 17 to 34. A third run under load 18 to 25 passed at 115.2 s, 5 s under the limit.

## 5. Decision

No decision this round. (The round dir holds no A and B measurements.)
