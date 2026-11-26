# Report review: R-06

Reviewed: `report.md`, `round.json`, `shots/`, `../R-05` (triage.md, decision.md, review-report.md), `../trend.md`, `ux/baseline.json`, `ux/round.ts`, `ux/ratchet.ts`, `ux/round-guard.ts`, `ux/findings.test.ts`, `e2e/copy.ts`, `e2e/screen-only.ts`, `e2e/probe.ts`, `e2e/metrics.ts`, `e2e/r06.e2e.ts`, `e2e/r06-problems.ts`, `e2e/tools.e2e.ts`, the offline scenario. No recordings and no questionnaire exist for this round. Everything was run hermetically (fake origin, loopback, nothing live, nothing paid). The probes of my own are scratch files, none committed. The brief named `references/review-agents.md` under the skill directory; that path does not exist on this host, so the section 2 text was taken from the plugin cache copy.

## Verdict

**Fit to send, with § 4.** The headline numbers reproduce exactly. Of the five R-06 changes, four moved their target; the fifth (UXF-014) has no assertion that sees the page, and UXF-019 moved the watchdog task fully and the failing-route task only partly. Two metrics mean less than their names say (`screen_only.looks`, and `copy.defects` as "run together"), and the report prints one red hard gate without saying why. Default action: send R-06 with § 4, take T18 to T24 before R-07.

## 1. Are the numbers right and meaningful?

### Recomputed

| what | result |
|---|---|
| `pnpm e2e --workers=2` (project `gate`), run twice | 320 passed both times; wall 87 s (includes about 5 s of sync to the build host and a queue) and 67.5 s; slowest tests 6.1 s (two) |
| the 20 gating replays of that run against `round.json` | identical in every metric of all 20; screen-only reached, actions, looks identical; copy item counts identical; page sha256 `249874c33fcb` equals the built page |
| `ratchet` test | passes: every gating value equals the baseline within tolerance |
| baseline against R-05's | only two values changed, both improvements (errors layout shift 0.007 to 0.003, token-flow 0.043 to 0.039); no old metric moved worse. 58 keys are new (offline 20) |
| `pnpm test` (vitest) | 84 of 84 pass (three untracked finding drafts of another reviewer present) |
| the measured-only replays (1536x754, 960x1080, 390x844) | not re-run: `ux:round` writes into the round directory. Their values are the same code path as the gating ones, which matched |

### Did each R-06 change move its target? (R-05 page measured by me with the R-06 tools)

I put the R-05 page (`dashboard.html` of a9bcd875) behind the R-06 harness and ran the R-06 assertions and the two probes on it.

| change | target | before (R-05 page) | after | moved |
|---|---|---|---|---|
| 1 UXF-010 | copy gate: no raw identifier as a label | `hex-title` 1, `snake-value` 2, `header-name` 1, `glyph-control` 1 in busy; errors has 4 of these | 0 in all five scenarios | yes |
| 2 UXF-016 | copy gate: no run-together word | `run-together` 1 idle, 6 busy, 4 token-flow, 5 errors | 0 | yes (see § 1 copy: some of the old items are not defects) |
| 3 UXF-011 | each state with one spelling | r06 UXF-011 test red on the R-05 page | green | yes |
| 4 UXF-014 | label size and cut-count assertions | the three route-name tests and the font-size test pass on the R-05 page too | pass | not observable: they read `FlowModel` and `FONT_PX` from `src/flow.ts` and only the box size from the page, so they pass whatever page is built. The canvas text is invisible to the copy probe and the key-data probe (both skip `.flow-panel`). The source change is real; no metric of the built page shows it (T22) |
| 5 UXF-019 | screen-only: failing route and watchdog above the fold, at most 2 actions each | watchdog 1 to 3 actions, looks 3 (busy), 0 actions looks 2 to 3 (errors); failing route busy 3 actions looks 3, errors 2 actions looks 3 | watchdog 0 actions, looks 1, both scenarios; failing route busy 2 actions looks 3, errors 1 action looks 2. Six r06 strip tests red on the R-05 page, green now | actions yes; looks 1 only for the watchdog. The decision's "expected glance 1" is not met for the failing route (busy 3, errors 2): the strip names the route and its count but not the cause, and the truth needs both (a person may read the route off the strip, the bot's task says it needs the cause). Errors' 2 comes from the click-jump rule, see below |

The baseline for the screen-only keys was first written in R-06 from the R-06 page, so the ratchet holds no before value; the table above is the only evidence of movement.

### Do the new metrics mean what their names say?

**`copy.defects`** counts distinct `<kind> <place>` items from `e2e/copy.ts`. Checked on a scratch page through the real function:

- Can it miss a run-together word? Yes, three ways. (a) Two elements side by side in a flex or grid container with no gap ("1.88k tok/s" then "last 60 s"): every flex item is blockified, so `broken()` counts the element boundary as a break; the text reads "tok/slast". (b) The first run ends in `%`, `)` or any other non-letter, non-digit, non-dash ("12%" then "errors", "(3 of 5)" then "errors"). (c) Two text nodes of one element (a template or `append` that joins strings): runs of one element are merged and never compared; nothing could tell a joined string from a word anyway. It does catch the plain inline case ("claude-code" then "3f9a1c0e", "7m" then "ago"). The metric is therefore "no missing whitespace in the DOM between inline elements", which is what copy-and-paste and a screen reader get, not what the eye sees.
- Does it flag legitimate text? Yes. Two inline spans separated only by a margin or padding (they look spaced), a number and its unit in two spans ("12" "ms"), `x<sup>2</sup>`, and any `snake_case` value in a cell that is not `.mono` ("qwen_coder" in a plain `td`). Some of the 20 R-05 items ("qwen-coder" then "vLLM", "retry 2" then "repeated body") are badges that probably looked spaced; fixing them by adding a space is harmless but they were not defects to a reader.
- A glyph control is flagged even when it has an `aria-label`; by design ("named by a glyph"), but then the name says "control named by a glyph", not "control without a name".

**`screen_only.*`** (e2e/screen-only.ts, `ux/ratchet.ts`):

- Visible, uncovered, role and name: yes for controls. A control is clicked only when fully inside the viewport and `elementFromPoint` at its centre returns it; it is found with `getByRole(link|button, {name})`. The break test covers the covered link. Limits: only links and buttons (not summaries or tabs), the accessible name rather than the visible text is what is matched, and the `follow` expression (`error|fail`, `watchdog|heartbeat`) is written by the tool's author who knows the page, so it measures "can the page be driven by those words", not discovery.
- Does it read the truth from the screen? From the DOM text, not pixels (`innerText` of elements in the viewport). Truth in an element that is fully in view and hit-testable counts even when a person could not read it: text cut by `text-overflow: ellipsis` (found), `opacity: 0` (found), text the colour of its background (found), 1 px text (found). It is blind to what is drawn but is not an element's text: canvas labels and generated content (`::before`) are never found, and the clipped-away and screen-reader-only text is correctly refused. The truncation gate covers part of the ellipsis case on the page today.
- Does a scroll count? A wheel scroll is one action and sets `looks` 3. A click on a link that jumps the page is one action and does not: after the jump `looks` is 1 or 2 by position. Scratch page: click an anchor 3000 px down, result `actions 1, looks 1`, `scrollY 3058`; the break test in `tools.e2e.ts` asserts that as intended. So errors' failing route reads `looks 2` after a jump to Routes (by the stated definition it should be 3), which flatters the R-06 page against busy (click then scroll, 3). `screen_only.looks` is "scrolls the bot itself made", not "screens the person saw".
- `reached` and `actions`: `reached` is a minimum over viewports and tasks, `actions` a sum over the tasks and then a maximum, so a two-task scenario's number is not a per-task count; the report's table shows the per-task rows, which is the right reading. Only light theme, two viewports, two scenarios; the `connection` truth of `offline` has no task (the offline gate reads the status text instead).

**Other new numbers.** `heading_font.deficit_px` 1.4 (busy, errors) and 0.7 (others) is real (13.3 px `h2`, 12.6 px `h3` and a `span` in limit titles) and held by UXF-021 in `known-failures.ts`. `overflow.scrollers.<viewport>` and the offline scenario's values are meaningful; offline's layout shift (0.021 to 0.024 at the gating viewports, 0.060 at 960x1080) is one state, the lost connection, and is the largest in the table at 960x1080. It is a first baseline for a new scenario, not comparable with R-05.

## 2. Findings the report misses

1. Key data row of the hard-gate table reads "0 of 10" in both themes with no mention that UXF-021 holds it in `e2e/known-failures.ts`; a reader sees a failed hard gate beside "headings 1.4 px short". Neither the report nor `trend.md` carries a column for key data, copy or screen-only.
2. The strip appears when the state turns bad; offline's replay shows the page moving 0.021 to 0.060 when the "Connection" line is inserted above the Token flow. The first appearance of the strip in busy and errors is the unmeasured first render. A person reading the page is moved by the strip (UXF-005 family); no finding exists for it.
3. The strip's route line counts only the last 5 minutes ("claude-sonnet 1 of 3" in busy) while the Routes table's Errors column counts all errors of the same route (2); the strip does not say its window apart from "in the last 5 min" in its own words, and the section it links to shows a different number. Seen in the busy contact sheet.
4. The failing route's cause ("429", the rate limit) is not in the strip: the person must follow the link and then scroll in busy. That is the open part of UXF-019 and of the decision's target.
5. The offline strip text ("Connection: the gateway does not answer; the page shows the last snapshot") and the red `offline` status read well; the header also shows `live` while the stream has not yet errored, by design.

## 3. Tool defects

R-05's T13 to T17 rechecked on this tree (the input for each in brackets):

- **T13 mostly fixed.** [keyDataProbe on the page in `tools.e2e.ts` "key data", plus my own page: `.tile .value` at 12 px.] Text outside the key and heading lists is now floored (12 px default), headings are a third bucket (14 px), and "no key text" refuses an empty page; the 20 unclassified texts of R-05 are now classified. Still open: the key bucket is a list of class names (`.tile .value`, `main tbody td`, ...). My page with the same value renamed to `.metric-value` at 12 px reports key 14 px from the one remaining table cell and no deficit. A rename of every class at once is caught only by "no key text"; a rename of one is not. (T18)
- **T14 fixed.** [`tools.e2e.ts` "truncation": ellipsis injected on `td.col-route`, then a title added.] The break test exists and passes.
- **T15 fixed.** [`ux/round-guard.ts` `PAGE_PATHS`.] `dashboard/ui`, `index.html` and `vite.config.ts` now mark the page `+changes`; the harness has its own list.
- **T16 fixed.** [`ux/baseline.json` `about`, `baselineRefusal`.] The baseline names the round, a commit and the page digest, and a baseline on a dirty tree is refused. It names commit `5a2e3003` while the report's page commit is `c5cc2cb3`; the digest `249874c33fcb` is the same on both, and the two commits differ in the baseline file and a decision record only.
- **T17 half fixed.** [`ux/baseline.json`, the ratchet test.] Narrow-viewport scrollers are ratcheted (`overflow.scrollers.<viewport>`: 390x844 3 to 5, 960x1080 0 to 3) but still not gated. An improvement inside the tolerance is still not flagged (layout shift: 10 % of the value, at least 0.002): `compare()` reports "improved" only beyond the band, although the test title says "every improvement is taken into it". `baselineFrom` does take it when a baseline is re-taken. (T19 below keeps this open item)

New:

- **T19.** `ux/ratchet.ts` `compare` and the title of the `ratchet` test in `e2e/gates.e2e.ts`: see T17; input: errors layout shift baseline 0.003 and a measured 0.001 passes silently.
- **T20.** `e2e/copy.ts` `broken()`: judges a break by element display and by text between the runs, not by geometry. Input: `<div style="display:flex"><span>1.88k tok/s</span><span>last 60 s</span></div>` gives no item (missed); `<span>alpha</span><span style="margin-left:8px">beta</span>` gives an item (false). Use the rectangles of the two runs (a gap of at least a space's width is a break).
- **T21.** `e2e/copy.ts` tail pattern `/[\p{L}\p{N}‐-―-]$/u`: a run ending in `%`, `)`, `]` or `s.` is never compared. Input: `<span>12%</span><span>errors</span>`.
- **T22.** `e2e/r06.e2e.ts` UXF-014 tests and `e2e/probe.ts` (`.flow-panel` skipped): the flow labels are asserted on `FlowModel`, not on the built page; they pass on the R-05 page (run: 8 passed, 12 failed on the R-05 page, all 3 route-name tests and the font test among the passes). No metric reads the canvas text.
- **T23.** `e2e/screen-only.ts` `playTask` `scrolled`: set only in the wheel-scroll branch, so a link that jumps the page leaves `looks` at 1 or 2. Input: scratch page with an anchor link to a section 3000 px down: `actions 1, looks 1, scrollY 3058`. Also `truthInView` reads `innerText`: ellipsis-clipped, `opacity: 0` and same-colour text count as seen.
- **T24.** `e2e/round-report.e2e.ts` (the key-data row, the "Worst moments" lines): the row prints "0 of 10" without the known-failure holding it; the worst-moment line for `heading_font.deficit_px` prints "–" for its location although the measurement holds `heading_font_at`. Also two UXF-019 tests wait for the 5 s poll interval and take 6.1 s each (the rule that a test over 5 s is named in the report: they are named here).

### What costs the time in the e2e step

Test time summed over the two workers: 142 s (R-05: 61 s); wall 67.5 to 87 s against the 120 s budget. By file: `measure-gate` 30 replays 71 s (idle 1.2 to 1.4 s, offline 1.6 s, errors 3.0 to 3.2 s, token-flow 3.7 to 3.9 s, busy 4.1 to 4.5 s each; the 10 header-only measures 15 s), `r06` 20 tests 24 s, `r03` 14 tests 18 s, `r04` 10 tests 15 s, `tools` 23 tests 12 s, the 223 assertion tests of `gates` 2 s. Alone, one busy replay costs: axe scan 0.36 to 0.48 s each (88 scans over the 20 gating replays, about 35 s, so about 55 % of the replay time), the screen-only replay 0.8 s (own context, light only, runs beside the main replay), one copy probe 0.02 s, key-data probe 0.01 s, first state 0.3 s; fixed waits of 750 ms (shot) and 500 ms (canvas check) and 600 ms after every input are waiting, not CPU. Each measure test runs three browser contexts at once (the replay, a reduced-motion replay, and for light the screen-only user). The growth over R-05 is the offline scenario (4 replays plus 2 header measures, about 8 s), the `r06` file (24 s, of which 12 s are the two connection tests waiting out the 5 s poll), and the screen-only replays; the new probes themselves cost nothing measurable. I did not re-run `ux:round` (112 to 123 s as stated): it replaces the round directory's files. The wall time moves with the host's load (load average 55 to 69 during this review, on a pool of 40 slots); the 120 s cut-off has the same thin margin R-05 noted.

## 4. Caveats to send with the round

1. Synthetic scenarios, no person in the loop: nothing here speaks to task time or comprehension.
2. The key-data hard gate is held by UXF-021 (headings 13.3 px and limit titles 12.6 px against the 14 px floor); "0 of 10" is that, not a new regression.
3. `copy.defects` 0 means "no missing whitespace between inline elements, no raw identifier, no glyph-named control"; it does not see run-together text in flex or grid containers, after `%` or `)`, or in canvas text.
4. `screen_only.looks` counts the bot's own scrolls; a link that jumps the page counts as one action and no scroll. Failing route: busy 3, errors 2 (a jump). Watchdog: 1 in both. The cause of the failures is not on the first screen.
5. The screen-only baseline was first taken in this round; its movement from R-05 comes from my measurement (above), not from the ratchet.
6. The UXF-014 change (labels at 14 px, route names whole) is verified on the source model only; no probe reads the canvas.
7. Offline's layout shift (0.021 to 0.060) is the first value of a new scenario.
8. Not re-run here: the three measured-only viewports and the `round` project.
