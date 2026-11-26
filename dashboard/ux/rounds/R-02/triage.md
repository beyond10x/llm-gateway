# R-02 triage: the change set for R-03

Applied by rule (SKILL § 3.3): priority = severity weight × frequency × confidence ÷ cost.
Frequency is scenarios affected ÷ 4 (idle, busy, token-flow, errors).

## Caveats

- The report reviewer found the numbers right (39 of 40 replays identical on a rerun) and their
  meaning weak in four places: no metric measures the token-flow canvas (T2); the theme-toggle
  assertion cannot fail (T1), so UXF-001 stays closed only on a hand check until T1 lands; per-state
  layout shift depends on step order (T3); the replay shows "polling" where the real stream shows
  "live" (T4).
- T1 to T8 and T12 are fixed in the harness before R-03 is measured. They change the measurement,
  not the page, and are not counted in the change set.
- No change set before this one, so the table of moved metrics is empty.

## Did the last change sets move their metrics?

| change set | target metric | moved |
|---|---|---|
| R-02 (theme toggle, token flow) | theme_applied; none for the canvas | toggle present; canvas unmeasured |

## Priorities

| finding | sev | freq | conf | cost | priority |
|---|---|---|---|---|---|
| UXF-002 scroll regions without keyboard access | S2 (3) | 1.00 | 2 | S (1) | 6.00 |
| UXF-003 cache tile runs into its neighbour | S2 (3) | 0.75 | 2 | S (1) | 4.50 |
| UXF-007 routes table cut at the right edge | S2 (3) | 1.00 | 2 | M (2) | 3.00 |
| UXF-008 token flow hidden behind panels | S3 (1) | 0.75 | 2 | S (1) | 1.50 |
| UXF-004 key data under 14 px | S3 (1) | 1.00 | 2 | M (2) | 1.00 |
| UXF-010 hash names, "▸ 2" button | S3 (1) | 0.75 | 1 | S (1) | 0.75 |
| UXF-011 three state vocabularies | S3 (1) | 0.75 | 1 | S (1) | 0.75 |
| UXF-006 status word broken mid-word | S3 (1) | 0.50 | 1 | S (1) | 0.50 |
| UXF-009 stale heartbeat in colour only | S3 (1) | 0.50 | 1 | S (1) | 0.50 |
| UXF-005 shift when calls arrive | S3 (1) | 0.75 | 1 | M (2) | 0.38 |

## Change set R-03 (5)

1. UXF-002: every horizontal scroll container is a labelled region with `tabindex="0"`.
   Target: axe.serious_critical 2 → 0.
2. UXF-003: the cache tile's table stays inside its tile. Target: overflow.elements 1 → 0.
3. UXF-007: the routes table shows every column at 1366x768 and 1280x720, or shows that it
   scrolls. Target: a new assertion, no table column cut without a scroll cue.
4. UXF-008, with decision R-03 (decision.md): the token flow gets its own panel.
   Target: a new metric, flow.visible_ratio of the drawn canvas ≥ 0.95.
5. UXF-004: key data at 14 px. Target: key_font.deficit_px 3.6 → 0.

Planning: task:dashboard-ux-r03 under story:dashboard-ux-loop.
