# Report review: R-05

Reviewed: `report.md`, `round.json`, `notes.md`, `shots/`, `../R-04`, `../R-02/review-report.md`, `../R-03/review-report.md`, `../trend.md`, `ux/baseline.json`, `ux/round.ts`, `ux/ratchet.ts`, `e2e/*`, the renderer (`src/essui`) and the document (`ui/llm-gateway.ui.yaml`) at the round's tree. No recordings and no questionnaire exist for this round. The numbers were recomputed by rerunning the hermetic suite and by probes of my own (loopback fake origin, Playwright, none committed, nothing live, nothing paid). Chromium 158.0.8127.18, axe-core 4.14.1, Playwright 1.65.0.

## Verdict

**Fit to send.** Every number reproduces, and the harness measures the same things on the renderer page as on the hand-written one: the page-specific selectors were carried over, and none of the checks that depend on them passes for lack of a match. Default action: send R-05 with the § 4 caveats (the R-04 to R-05 column in `notes.md` changes three things at once; use the same-font comparison below), and take § 3 T13 to T17 before R-06.

## 1. Are the numbers right and meaningful?

### Recomputed

| what | result |
|---|---|
| `pnpm e2e --workers=2` (project `gate`) | 221 passed; 33 s wall locally (20 s user CPU), 55 s on the build host; slowest test 4.7 s, none near 60 s |
| the 16 gating replays against `round.json` | identical in every metric, per-state shift, key and secondary font, status, axe scan count and every detail list; only `flow.drawn_share` differs (0.0734 against 0.0627 for busy 1280x720 dark), which moves with the animation phase and is not a metric |
| the 24 measured-only replays (1536x754, 960x1080, 390x844) against `round.json` | identical in metrics, shifts, key font and details |
| gate 2 at the measured-only viewports | smallest visible share 1 over all 8 header sets, as reported |
| page | `crates/llm-gateway/src/dashboard.html` equals a fresh `vite build` of the tree (sha256 `7c042a1eea54…`, as in the report) |
| `UX_FONTS=host:DejaVu Sans` (claim in `notes.md`) | 219 passed, 1 skipped (the pinned-fonts test), 1 failed: the ratchet reports token-flow layout shift better than the baseline (0.036 against 0.043). Same as the note |
| `pnpm test` (vitest) | 74 of 75 pass. The one failure is the findings check on two untracked draft records (UXF-016, UXF-017) written beside this review; their quoted tokens do not match the cited lines. Not part of the round |

### The renderer against the hand-written page, same fonts

`notes.md` compares R-05 with R-04, but R-04 was measured with the host's fonts and without the wrapping fix, so that column carries three changes. The fair comparison is the page the baseline was taken on (built at `1368926d3`, sha256 `951512ba2fae…`, pinned fonts, wrapping fix): I measured it with the same harness at all 5 viewports, both themes, 4 scenarios (40 replays). It equals R-05 in every metric except layout shift, where R-05 is lower or equal everywhere: token-flow 1366x768 0.043 to 0.039, 1280x720 0.039 to 0.035, errors 960x1080 0.021 to 0.011. "No metric is worse" holds. The busy scrollers 1 to 0 against R-04 comes from the merged wrapping and fonts, not from the renderer (the old page with pinned fonts already reads 0).

### Selector audit (what changed, and what still finds something)

| harness selector | old page | new page | verdict |
|---|---|---|---|
| `.limit` | busy 2, errors 1 | gone; no reference left in `e2e/` or `ux/` | n/a |
| `.card` (`.card .tile .value`, `.card h3`; `r03`, `r04`) | 0 | busy 2, errors 1, idle and token-flow 0, equal to the old `.limit` counts | holds; the UXF-006 title assertion compares against the limits in the snapshot, so an empty match fails it |
| `col-<name>` cells | 0 | 504 cells in busy | no selector reads them; the layout probe keys use the first class (`td.col-route:nth-of-type(1)`), stable per element |
| `td.mono` (pinned-fonts test) | 36 in busy | 36 | holds; the test throws on no element |
| `.tile .value`, `.tile .sub`, `.tile .label`, `.small`, `.badge`, `.stats dt`, `.flow-panel`, `.skip`, `thead th`, `main tbody td`, `header [role=status]` (key-data probe) | present (busy: tile values 10, second lines 8, labels 10, `.small` 68) | same except `.stats dt` 3 to 4, `main dd` 12 to 13 (the new Name figure) | key text 540 elements, secondary 138 to 139, unclassified 20 to 20 in busy; the other 3 scenarios equal |
| controls and scrollers numbered for the Tab walk | idle 9, busy 17, token-flow 9, errors 14 | same labels | identical |
| `.alarm`, `.flow-empty`, `.flow-key`, `.flow-count`, `#h-routes`, `#h-calls`, `h1`, `[role=status]` | present | same counts | holds |

Vacuous checks: none found among the selector-dependent ones. Two checks pass for a reason other than "the page is good", see T13 and T14; both are the same on the old page.

### Do the metrics mean what their names say here?

- `overflow.scrollers` 0 at the gating viewports is real: `div.scroll` still exists on every table (5 elements scroll-capable) and none overflows at 1280x720 or 1366x768. At 390x844 and 960x1080 it reads 3 to 5, as before, and is neither gated nor ratcheted (T17).
- `truncation.without_full_value` 0 is true and nearly empty: only two elements on the page clip their content at all (the theme toggle and the canvas); there is no `text-overflow: ellipsis` (T14).
- `key_font.deficit_px` 0 and `key font px` 14 hold with the same classification as before, but 20 text elements in busy sit in neither bucket and are not floored (T13).
- `layout_shift.per_state_max` is the same quantity on both pages; the first render of a page (placeholder to content) is never measured, by design since R-03.

## 2. Findings the report misses

1. Recent calls: the Status column lost the `.n` class (right-aligned and tabular before, left now), `td.n` in busy 327 to 297. The Pod cell in Provider is monospaced now. No probe sees either; the heuristic review should.
2. The `.ok`, `.bad`, `.warn` colour classes are gone (`tone-*` pills and classes replace them: 50 in busy). axe contrast reads 0 on both themes, so no failure, but nothing in the harness checks that a bad status is still distinguishable from a good one other than by colour.
3. `notes.md` says what changed for the reader (Provider heading, "Provider balance", `.card`); `report.md` does not say that R-04 used host fonts, so a reader of `trend.md` alone sees R-04 to R-05 flat in key font and shift and cannot tell the page, the fonts and the wrapping all moved.

## 3. Tool defects

R-02 and R-03 defects, rechecked on this tree:

- **T1 to T12** hold: the break tests are present in `e2e/tools.e2e.ts` and `ux/*.test.ts` and pass in the 221, and `findings.test.ts` now checks cited tokens, not only line ranges (it fired on the two draft records above).
- **N1** fixed: flow gated on busy, token-flow and errors, drawn under reduced motion asserted, `flow.*` in the baseline.
- **N2** mostly fixed: scrollers are a metric and a "worst moments" list; the measured-only values are not gated (T17).
- **N3** fixed: layout-shift tolerance is 10 % of the baseline, at least 0.002.
- **N4** closed: key-font deficit reads 0.
- **N5** partly: `trend.md` stamps every round with 8 characters; the baseline's `about` still cites sha256 `951512ba2fae` at "076c9fb9+changes", which is the page built at `1368926d3` and no committed round (T16).

Open:

- **T13.** `e2e/probe.ts` `keyDataProbe` and `e2e/gates.e2e.ts` "key data": the gate has no guard that key or secondary text was found, and text outside the two class lists is not floored. In busy: `h2` 13.3 px (6), `.card h3` 12.6 px (2), `span.mono` in the card heading 12.6 px (2), `dl.headers dt` 14 px (9), the "Show all" button 12.6 px (1). Same on the old page, so not a regression. The buckets are class names the renderer emits (`.small`, `.badge`, `.tile`, `.stats`, `.flow-panel`, `.skip`): a rename moves text to "neither" with no failure.
- **T14.** `e2e/tools.e2e.ts`: the truncation probe has no break test. With `text-overflow: ellipsis` injected on `td.col-route` the probe reports all 33 cells, so it works; it passes today because the page clips nothing but the toggle and the canvas.
- **T15.** `e2e/round-report.e2e.ts` `commits()`: the page's `+changes` mark looks at `dashboard/src` and `dashboard.html` only, not `dashboard/ui/` (the document the page is drawn from), `dashboard/index.html` or `dashboard/vite.config.ts`. An edited document with an unbuilt page is not marked. The sha256 of the page still identifies what was measured, and the gate's build-and-diff step ties page to source.
- **T16.** `ux/baseline.json` `about` and `notes.md`: see N5. The baseline names no committed round, and the R-04 comparison in `notes.md` is not like for like (§ 1).
- **T17.** `ux/ratchet.ts` and `e2e/gates.e2e.ts`: `overflow.scrollers` at 390x844 and 960x1080 (3 to 5, every scenario but idle at 960) is measured and neither gated nor ratcheted, so a table that starts to scroll on a narrow screen is invisible to the loop. Also: an improvement inside the tolerance is never taken into the baseline (token-flow 0.043 baseline, 0.039 measured, band 0.0043), so the shift can climb back to 0.047 unnoticed.

## 4. Caveats to send with the round

1. Scenarios are synthetic and no person was in the loop: nothing here speaks to task time or comprehension.
2. Read the renderer's effect from the same-font comparison (§ 1): every metric equal, layout shift lower or equal. The R-04 column mixes the fonts, the wrapping fix and the renderer.
3. At 390x844 and 960x1080 tables scroll sideways (3 to 5 regions) and nothing gates it. At the two gating viewports none does.
4. The truncation gate and the key-data gate are quiet because the page has nothing to truncate and the text lands in the probe's known buckets; see T13 and T14.
5. The e2e step: 33 s on an idle local run, 55 s on the build host. Cost: the 24 measure replays are 35 s of the 61 s summed test time, and axe is about 60 % of a busy replay (76 scans per run); the 10 break and regression tests of `r04` take 9 s, the 13 of `r03` 10 s, `tools` 6 s, the 159 assertion tests in `gates` 1 s. A 107 s reading was not reproduced: the step is CPU-bound (about 23 CPU-seconds over 2 workers), so on a machine with a load average of 16 to 20 on 20 cores its wall time stretches in proportion; the 120 s cut-off in `Taskfile.yml` and the gate workflow has no margin against that.
