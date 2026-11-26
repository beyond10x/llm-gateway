# Round R-05

Date 2026-11-26. Page commit `a9bcd875`, built page sha256 `7c042a1eea54…`. Theme toggle: present.
Tools: Chromium 158.0.8127.18, axe-core 4.14.1 (@axe-core/playwright 4.14.1), Playwright 1.65.0, harness commit `a9bcd875`. Fonts: pinned: IBM Plex Sans and IBM Plex Mono 1.1.0, Latin-1.
Scenarios: `idle` (anchor), `busy` (anchor), `token-flow`, `errors` (anchor). Gating viewports 1366x768, 1280x720; measured-only 1536x754, 960x1080, 390x844.

## Method

- Hermetic: every snapshot comes from `ux/scenarios/`; the page is the built `dashboard.html` on a fake origin; every other request is aborted (`blocked` in round.json).
- The first state answers the one `dashboard/snapshot` fetch; every state then arrives as one `snapshot` event on a stand-in event stream that opened once and stays open, as the gateway serves it, so the connection status reads "live" and the page never polls.
- Layout shift per state: the sum of layout-shift entries from the moment the state is pushed to two animation frames later. No state is pushed within 600 ms of a harness click or key press, since the browser leaves out shifts that soon after an input; the value does not depend on what ran before it (an axe scan, a wait).
- axe-core scans every state of every replay and the page after every action; a rule counts the distinct nodes over all scans.
- Fonts: pinned: IBM Plex Sans and IBM Plex Mono 1.1.0, Latin-1. The pinned fonts are committed under `e2e/fonts/` and installed in every replay before the page's first script, so every host measures the same text widths (e2e/fonts.ts); "host" measures with the fonts the host has installed and is not the gate.
- The token-flow canvas is measured at the last state, scrolled to the top: `flow.visible_ratio` is the share of a 32 x 16 grid over its box that lies in the viewport with nothing painted over it; `flow.drawn` is whether it holds pixels other than its background.

## Hard gates (gating viewports)

| gate | theme | replays passing | headline |
|---|---|---|---|
| gate 1 layout | light | 8 of 8 | overflow 0 px max, 0 elements past their box max, 0 scrollers Tab never reaches max (Chromium focuses a scrolling box itself; axe judges the markup) |
| gate 1 truncation | light | 8 of 8 | 0 truncated without full value max |
| gate 2 always visible | light | 8 of 8 | header visible ratio 1 min; toggle present |
| gate 2 always visible, every viewport | light | 20 of 20 | hidden: none |
| gate 5 axe | light | 8 of 8 | 0 serious/critical nodes over 8 replays (none) |
| gate 5 keyboard | light | 8 of 8 | 0 unreachable, 0 focus-invisible max |
| gate 5 reduced motion | light | 8 of 8 | 0 violations max |
| key data | light | 8 of 8 | key 0 px short max (floor 14), secondary 0 px short max (floor 12) |
| flow (busy, token-flow, errors) | light | 6 of 6 | visible ratio 1 min (bar 0.95), drawn 6 of 6, drawn under reduced motion 6 of 6 |
| replay hermetic and live | light | 8 of 8 | 0 requests aborted |
| gate 1 layout | dark | 8 of 8 | overflow 0 px max, 0 elements past their box max, 0 scrollers Tab never reaches max (Chromium focuses a scrolling box itself; axe judges the markup) |
| gate 1 truncation | dark | 8 of 8 | 0 truncated without full value max |
| gate 2 always visible | dark | 8 of 8 | header visible ratio 1 min; toggle present |
| gate 2 always visible, every viewport | dark | 20 of 20 | hidden: none |
| gate 5 axe | dark | 8 of 8 | 0 serious/critical nodes over 8 replays (none) |
| gate 5 keyboard | dark | 8 of 8 | 0 unreachable, 0 focus-invisible max |
| gate 5 reduced motion | dark | 8 of 8 | 0 violations max |
| key data | dark | 8 of 8 | key 0 px short max (floor 14), secondary 0 px short max (floor 12) |
| flow (busy, token-flow, errors) | dark | 6 of 6 | visible ratio 1 min (bar 0.95), drawn 6 of 6, drawn under reduced motion 6 of 6 |
| replay hermetic and live | dark | 8 of 8 | 0 requests aborted |

## Metrics per scenario, worst value (lower is better)

"gating" is the worst over the gating viewports and both themes, the basis of the ratchet; "all" is the worst over every viewport.

| metric | idle gating | idle all | busy gating | busy all | token-flow gating | token-flow all | errors gating | errors all |
|---|---|---|---|---|---|---|---|---|
| axe.serious_critical | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| overflow.x_px | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| overflow.elements | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| overflow.scrollers | 0 | 3 | 0 | 5 | 0 | 4 | 0 | 5 |
| overflow.scrollers_unreachable | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| header.hidden_ratio | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| keyboard.unreachable | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| keyboard.focus_invisible | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| key_font.deficit_px | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| secondary_font.deficit_px | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| layout_shift.per_state_max | 0 | 0 | 0 | 0 | 0.039 | 0.059 | 0.006 | 0.011 |
| reduced_motion.violations | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| truncation.without_full_value | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| key font px, smallest (n/a: no key data) | 14 | 14 | 14 | 14 | 14 | 14 | 14 | 14 |
| flow.visible_ratio, smallest (higher is better) | 1 | 1 | 1 | 1 | 1 | 1 | 1 | 1 |
| flow.drawn, replays drawn in both the replay and its reduced-motion replay | 0/4 | 0/10 | 4/4 | 10/10 | 4/4 | 10/10 | 4/4 | 10/10 |

`key_font.deficit_px` is 14 px less the smallest key-data font, 0 where a page shows no key data, and `secondary_font.deficit_px` the same for secondary text against 12 px (one definition of both: `keyDataProbe` in e2e/probe.ts); `overflow.scrollers` counts the elements that scroll sideways; `header.hidden_ratio` is 1 less the smallest visible share of a header element; `axe.rule.<id>` counts the distinct nodes of one rule over every scan.

## Every replay

| scenario | viewport | theme | axe | overflow px | past box | scrollers | header visible | unreachable | focus invisible | key font px | layout shift max | reduced motion | truncated | flow visible | flow drawn (reduced motion) | status |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| busy | 1280x720 | dark | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | yes (yes) | live |
| busy | 1280x720 | light | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | yes (yes) | live |
| busy | 1366x768 | dark | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | yes (yes) | live |
| busy | 1366x768 | light | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | yes (yes) | live |
| busy | 1536x754 (measured) | dark | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | yes (yes) | live |
| busy | 1536x754 (measured) | light | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | yes (yes) | live |
| busy | 390x844 (measured) | dark | 0 | 0 | 0 | 5 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | yes (yes) | live |
| busy | 390x844 (measured) | light | 0 | 0 | 0 | 5 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | yes (yes) | live |
| busy | 960x1080 (measured) | dark | 0 | 0 | 0 | 3 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | yes (yes) | live |
| busy | 960x1080 (measured) | light | 0 | 0 | 0 | 3 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | yes (yes) | live |
| errors | 1280x720 | dark | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0.004 | 0 | 0 | 1 | yes (yes) | live |
| errors | 1280x720 | light | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0.004 | 0 | 0 | 1 | yes (yes) | live |
| errors | 1366x768 | dark | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0.006 | 0 | 0 | 1 | yes (yes) | live |
| errors | 1366x768 | light | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0.006 | 0 | 0 | 1 | yes (yes) | live |
| errors | 1536x754 (measured) | dark | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0.006 | 0 | 0 | 1 | yes (yes) | live |
| errors | 1536x754 (measured) | light | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0.006 | 0 | 0 | 1 | yes (yes) | live |
| errors | 390x844 (measured) | dark | 0 | 0 | 0 | 5 | 1 | 0 | 0 | 14 | 0.008 | 0 | 0 | 1 | yes (yes) | live |
| errors | 390x844 (measured) | light | 0 | 0 | 0 | 5 | 1 | 0 | 0 | 14 | 0.008 | 0 | 0 | 1 | yes (yes) | live |
| errors | 960x1080 (measured) | dark | 0 | 0 | 0 | 3 | 1 | 0 | 0 | 14 | 0.011 | 0 | 0 | 1 | yes (yes) | live |
| errors | 960x1080 (measured) | light | 0 | 0 | 0 | 3 | 1 | 0 | 0 | 14 | 0.011 | 0 | 0 | 1 | yes (yes) | live |
| idle | 1280x720 | dark | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | no (no) | live |
| idle | 1280x720 | light | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | no (no) | live |
| idle | 1366x768 | dark | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | no (no) | live |
| idle | 1366x768 | light | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | no (no) | live |
| idle | 1536x754 (measured) | dark | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | no (no) | live |
| idle | 1536x754 (measured) | light | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | no (no) | live |
| idle | 390x844 (measured) | dark | 0 | 0 | 0 | 3 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | no (no) | live |
| idle | 390x844 (measured) | light | 0 | 0 | 0 | 3 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | no (no) | live |
| idle | 960x1080 (measured) | dark | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | no (no) | live |
| idle | 960x1080 (measured) | light | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0 | 0 | 0 | 1 | no (no) | live |
| token-flow | 1280x720 | dark | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0.035 | 0 | 0 | 1 | yes (yes) | live |
| token-flow | 1280x720 | light | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0.035 | 0 | 0 | 1 | yes (yes) | live |
| token-flow | 1366x768 | dark | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0.039 | 0 | 0 | 1 | yes (yes) | live |
| token-flow | 1366x768 | light | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0.039 | 0 | 0 | 1 | yes (yes) | live |
| token-flow | 1536x754 (measured) | dark | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0.038 | 0 | 0 | 1 | yes (yes) | live |
| token-flow | 1536x754 (measured) | light | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 14 | 0.038 | 0 | 0 | 1 | yes (yes) | live |
| token-flow | 390x844 (measured) | dark | 0 | 0 | 0 | 4 | 1 | 0 | 0 | 14 | 0.012 | 0 | 0 | 1 | yes (yes) | live |
| token-flow | 390x844 (measured) | light | 0 | 0 | 0 | 4 | 1 | 0 | 0 | 14 | 0.012 | 0 | 0 | 1 | yes (yes) | live |
| token-flow | 960x1080 (measured) | dark | 0 | 0 | 0 | 2 | 1 | 0 | 0 | 14 | 0.059 | 0 | 0 | 1 | yes (yes) | live |
| token-flow | 960x1080 (measured) | light | 0 | 0 | 0 | 2 | 1 | 0 | 0 | 14 | 0.059 | 0 | 0 | 1 | yes (yes) | live |

## Worst moments at the gating viewports

- `layout_shift.per_state_max` = 0.039 at token-flow 1366x768 dark: per state: 0.024, 0.039, 0.027, 0.007, 0.013, 0.004, 0.004

## Worst moments at the measured-only viewports (no screenshot)

- `overflow.scrollers` = 5 at busy 390x844 dark: div.scroll "PodsPodModelStateGPU$/hUptimevLLM readyL" / div.scroll "RouteModelWireStateElapsedSessionqwen-co" / div.scroll "RouteCallsErrorsCancelledTokens in / out"
- `layout_shift.per_state_max` = 0.059 at token-flow 960x1080 dark: per state: 0.059, 0.053, 0.059, 0.015, 0.025, 0.007, 0.012

## Ratchet against `ux/baseline.json` (gating values)

- every value equals the baseline within its tolerance

## Contact sheets

The last state of each scenario, 0.75 s after it arrived, at half size.

![contact-1366x768-light.png](shots/contact-1366x768-light.png)
![contact-1366x768-dark.png](shots/contact-1366x768-dark.png)
![contact-1280x720-light.png](shots/contact-1280x720-light.png)
![contact-1280x720-dark.png](shots/contact-1280x720-dark.png)
