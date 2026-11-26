# R-03 triage: the change set for R-04

Applied by rule (SKILL § 3.3): priority = severity weight × frequency × confidence ÷ cost.
Frequency is scenarios affected ÷ 4 (idle, busy, token-flow, errors).

## Caveats

- The report reviewer found the numbers right and every improvement taken into the baseline.
  T1 to T11 are fixed; T12 is fixed in part (cited lines are checked for range, not content).
- Open tool defects N1 to N5 (review-report.md § 3) are fixed in the harness before R-04 is
  measured; they are not counted in the change set.
- UXF-004 was closed with a key-font deficit of 1.75 px left, because its test and the metric
  define key data differently (N4). It is reopened until the two agree.
- UXF-015 is new: the report reviewer found the theme toggle 20 % clipped at 390x844 in every
  scenario (N2). The toggle is a header control that must stay in view.

## Did the last change sets move their metrics?

| change | target metric | before → after | moved |
|---|---|---|---|
| UXF-002 scroll regions | axe.serious_critical | 2 → 0 | yes |
| UXF-003 cache tile | overflow.elements | 1 → 0 | yes |
| UXF-007 routes table | columns cut | 4 or more → 0 | yes |
| UXF-008 token-flow panel | flow.visible_ratio | 0 to 0.375 → 1 | yes |
| UXF-004 key data 14 px | key_font.deficit_px | 3.6 → 1.75 | in part (target 0) |

## Priorities

| finding | sev | freq | conf | cost | priority |
|---|---|---|---|---|---|
| UXF-015 theme toggle clipped at 390x844 | S2 (3) | 1.00 | 1 | S (1) | 3.00 |
| UXF-013 flow panel grows at 390x844 | S3 (1) | 1.00 | 1 | S (1) | 1.00 |
| UXF-004 key data under 14 px (reopened) | S3 (1) | 1.00 | 2 | M (2) | 1.00 |
| UXF-012 flow caption does not say what strings and dots mean | S3 (1) | 0.75 | 1 | S (1) | 0.75 |
| UXF-014 flow labels and idle strings below contrast | S3 (1) | 0.75 | 1 | S (1) | 0.75 |
| UXF-010 hash names, "▸ 2" button | S3 (1) | 0.75 | 1 | S (1) | 0.75 |
| UXF-011 three state vocabularies | S3 (1) | 0.75 | 1 | S (1) | 0.75 |
| UXF-006 status word broken mid-word | S3 (1) | 0.50 | 2 | S (1) | 1.00 |
| UXF-009 stale heartbeat in colour only | S3 (1) | 0.50 | 2 | S (1) | 1.00 |
| UXF-005 shift when calls arrive | S3 (1) | 0.75 | 2 | M (2) | 0.75 |

UXF-006 and UXF-009 rise to 1.00: both reviewers of R-03 saw them.

## Change set R-04 (5)

1. UXF-015: the theme toggle fully in view at 390x844. Target: header.hidden_ratio 0.2 → 0 at
   every viewport.
2. UXF-013: the flow panel keeps its height when sessions come and go. Target: layout_shift at
   390x844 for the first session → 0.
3. UXF-006: status values wrap at word boundaries or fit. Target: a new assertion, no word broken
   inside a status value.
4. UXF-009: a stale watchdog heartbeat says "stale" in words. Target: a new assertion.
5. UXF-012 with UXF-014: the flow panel's caption says what strings and dots mean, and its
   labels and idle strings meet 4.5:1 and 3:1. Target: new assertions on the caption text and the
   drawing colours.

Next in line: UXF-004 (after N4 aligns its definition), UXF-010, UXF-011, UXF-005.

Planning: task:dashboard-ux-r04 under story:dashboard-ux-loop.
