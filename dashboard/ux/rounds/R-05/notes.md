# R-05 against R-04

R-05 measures the page that the ess-ui renderer draws from `ui/llm-gateway.ui.yaml`. It replaces the
hand-written App.vue and Kpis.vue. The base branch's pinned fonts and Recent calls wrapping are
merged in. R-04 measured the hand-written page with the host's fonts.

Not like for like: the R-04 column below differs from R-05 in three things at once: the page (the
renderer), the fonts (host against pinned) and the Recent calls wrapping. Read the renderer's own
effect from the same-font comparison in review-report.md § 1: the page the baseline was taken on,
measured with the pinned fonts and the wrapping, equals R-05 in every metric, and R-05's layout
shift is lower or equal everywhere. The baseline at the time named that page by its sha256 and
`076c9fb9+changes`, not a committed round; from R-06 a baseline is refused on a tree that differs
from its commit, and it names the round and the commit it was taken on.

## Hard gates, gating viewports, light / dark

| gate | R-04 | R-05 |
|---|---|---|
| gate 1 layout | 8/8 / 8/8 | 8/8 / 8/8 |
| gate 1 truncation | 8/8 / 8/8 | 8/8 / 8/8 |
| gate 2 always visible | 8/8 / 8/8 | 8/8 / 8/8 |
| gate 2 always visible, every viewport | 20/20 / 20/20 | 20/20 / 20/20 |
| gate 5 axe | 8/8 / 8/8 | 8/8 / 8/8 |
| gate 5 keyboard | 8/8 / 8/8 | 8/8 / 8/8 |
| gate 5 reduced motion | 8/8 / 8/8 | 8/8 / 8/8 |
| key data | 8/8 / 8/8 | 8/8 / 8/8 |
| flow | 6/6 / 6/6 | 6/6 / 6/6 |
| replay hermetic and live | 8/8 / 8/8 | 8/8 / 8/8 |

## Ratchet metrics, worst over the gating viewports (idle / busy / token-flow / errors)

| metric | R-04 | R-05 | change |
|---|---|---|---|
| axe.serious_critical | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | none |
| overflow.x_px | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | none |
| overflow.elements | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | none |
| overflow.scrollers | 0 / 1 / 0 / 0 | 0 / 0 / 0 / 0 | busy 1 to 0, from the merged calls wrapping; the baseline already holds 0 |
| overflow.scrollers_unreachable | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | none |
| header.hidden_ratio | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | none |
| keyboard.unreachable | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | none |
| keyboard.focus_invisible | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | none |
| key_font.deficit_px | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | none |
| secondary_font.deficit_px | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | none |
| layout_shift.per_state_max | 0 / 0 / 0.043 / 0.007 | 0 / 0 / 0.039 / 0.006 | lower, but within the ratchet's tolerance (0.0043 and 0.002), so the baseline stays |
| reduced_motion.violations | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | none |
| truncation.without_full_value | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | none |
| flow.visible_ratio (higher is better) | 1 / 1 / 1 / 1 | 1 / 1 / 1 / 1 | none |
| flow.drawn | 0/4, 4/4, 4/4, 4/4 | 0/4, 4/4, 4/4, 4/4 | none |

No metric is worse. With `UX_FONTS=host:DejaVu Sans` every hard gate also passes, and the Recent
calls table does not scroll sideways at 1280x720. The ratchet's only finding there is a token-flow
layout shift better than the baseline (0.036). That run uses host fonts, so it cannot set the
baseline.

## What the reader sees differently

- The Provider frame is titled "Provider". The provider's name is a "Name" figure, where it used to
  appear in the heading.
- The balance tile reads "Provider balance".
- Subscription limit cards are `.card` elements, which were `.limit`. r03 and r04 read the new class.
