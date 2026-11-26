# R-06 triage: the change set for R-07

Applied by rule (SKILL § 3.3): priority = severity weight × frequency × confidence ÷ cost.
Frequency is scenarios that show the defect ÷ 5 (idle, busy, token-flow, errors, offline), counted
from the R-06 shots and the page source. Confidence is 2 when two sources saw it (a reviewer of any
round, the process rating, a metric), else 1. Severities are the records' own.

## Caveats

- Both reviews ran before this triage. The report reviewer reproduced every gating number (320 of
  320 tests, page sha256 equal) and ran the R-05 page under the R-06 harness to judge each change.
- The new metrics mean less than their names: `copy.defects` judges DOM whitespace, not what the
  eye sees (T20, T21); `screen_only.looks` misses a page that a link jumped (T23) and reads text,
  not pixels; the screen-only keys have no value before R-06. Tool defects T18 to T24 are fixed
  beside the change set and are not counted in it.
- The gate step grew from 61 s to 142 s of summed test time (87 s wall locally): the offline
  scenario, the R-06 spec, and two connection tests that wait out the page's 5 s poll. The R-07
  tools work brings it back under 70 s wall without dropping a check.
- UXF-004 and UXF-022 no longer hold (heuristic review): closed in R-07 with their assets.

## Did the last change set move its metrics?

| change | target | moved |
|---|---|---|
| UXF-010 limit names | copy items for raw identifiers → 0 | yes (R-05 page: 1, 11, 4, 9) |
| UXF-016 run-together words | copy items → 0 | yes |
| UXF-011 one state vocabulary | its assertion red on R-05, green now | yes |
| UXF-014 flow labels | label size and cut count | not observable: the assertions read the model, not the canvas (T22) |
| UXF-019 alert strip | screen-only glance 1 for both tasks | watchdog yes (glance 3 → 1); failing route no (busy glance 3, errors glance 2): the strip does not name the cause |

## Priorities

| finding | sev | freq | conf | cost | priority |
|---|---|---|---|---|---|
| UXF-021 section headings 13.3 px | S3 (1) | 1.00 | 2 | S (1) | 2.00 |
| UXF-026 failed-calls line disagrees with Routes, names no cause | S3 (1) | 0.40 | 2 | S (1) | 0.80 |
| UXF-018 Provider frame labels | S3 (1) | 0.40 | 2 | S (1) | 0.80 |
| UXF-020 balance tile alone on a row | S3 (1) | 0.40 | 2 | S (1) | 0.80 |
| UXF-017 no scroll cue at 390 px | S3 (1) | 0.80 | 2 | M (2) | 0.80 |
| UXF-024 session label loses the client at 390 px | S3 (1) | 0.60 | 1 | S (1) | 0.60 |
| UXF-025 offline page keeps drawing the last snapshot | S3 (1) | 0.20 | 1 | S (1) | 0.20 |
| UXF-027 Problems link lands under the sticky header at 390 px | S3 (1) | 0.40 | 1 | S (1) | 0.40 |
| UXF-013 idle flow panel tall for one line | S3 (1) | 0.20 | 2 | S (1) | 0.40 |
| UXF-005 shift when calls arrive | S3 (1) | 0.40 | 1 | M (2) | 0.20 |

No S1 or S2 finding is open. UXF-026 takes the unmet half of UXF-019: the failing-route line names
the cause and agrees with Routes.

## Change set R-07 (5)

1. UXF-021: section and card headings at 14 px or more. Target: heading_font.deficit_px → 0, the
   UXF-021 known failure removed.
2. UXF-026: the failed-calls line counts what Routes counts, names every failing route and its
   cause (status and error type), and links to Routes. Target: screen-only "failing route" at
   glance 1 with 0 actions in busy and errors.
3. UXF-018: the Provider frame names the provider in its heading again; figure labels a reader can
   say. Target: an assertion on the heading and labels.
4. UXF-020: the balance tile stays on the Overview row at 1280x720. Target: an assertion on tile
   rows at the gating viewports.
5. UXF-017: a visible cue on every table that scrolls sideways at 390x844. Target: an assertion
   that each scrolling region shows its cue.

Next in line: UXF-024, UXF-027, UXF-013, UXF-025.

Planning: task:dashboard-ux-r07 under story:dashboard-ux-loop.
