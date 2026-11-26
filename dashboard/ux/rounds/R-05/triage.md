# R-05 triage: the change set for R-06

Applied by rule (SKILL § 3.3): priority = severity weight × frequency × confidence ÷ cost.
Frequency is scenarios that show the defect ÷ 4 (idle, busy, token-flow, errors), counted from the
R-05 shots and the page source, not from the record's own count. Confidence is 2 when two sources
saw it (a reviewer of any round, the process rating, a metric), else 1.

## Caveats

- R-04 was measured but neither reviewed nor triaged; R-05 changed the whole page (the ess-ui
  document and its renderer) outside any change set. The process rating (process-rating.md) scores
  findings, missed findings, process, measurement and outcome 3 of 5 each. This round goes back to
  the full order: two reviews, this triage, one decision, a failing-first build.
- The process rating corrected six records: UXF-015 was a probe defect (now S3, class engine);
  UXF-001 S2 → S3; UXF-010 S3 → S2; UXF-004, UXF-013 and UXF-014 are open again. It filed
  UXF-019 to UXF-023.
- The R-03 triage overstated frequencies (UXF-013 at 1.00 where 2 of 4 scenarios show it).
  This table counts again.
- Tool work runs beside the change set and is not counted in it: T13 to T17 (R-05
  review-report.md), a gate on visible text (no gate read visible text, so run-together words
  passed), the screen-only user of SKILL § 7 on two tasks, a connection-loss replay (UXF-022) and
  shots below the fold (UXF-023).

## Did the last change sets move their metrics?

| change | target | moved |
|---|---|---|
| UXF-015 (R-04) | header.hidden_ratio 0.2 → 0 at 390x844 | yes, by a probe fix; the page was never wrong |
| UXF-013 (R-04) | no growth when sessions arrive | yes; the idle box is still 180 px, so reopened |
| UXF-006 (R-04) | no word broken in a status value | yes |
| UXF-009 (R-04) | "stale" in words | yes |
| UXF-012, UXF-014 (R-04) | caption and contrast assertions | caption yes; labels 12 px and cut at 390 px, so UXF-014 reopened |
| renderer (R-05, outside a set) | no gate worse | yes; copy regressions UXF-016 and UXF-018 |

## Priorities

| finding | sev | freq | conf | cost | priority |
|---|---|---|---|---|---|
| UXF-010 limits named by a hash, raw header words, "▸ 2" | S2 (3) | 0.50 | 2 | S (1) | 3.00 |
| UXF-016 words run together after the renderer | S3 (1) | 1.00 | 2 | S (1) | 2.00 |
| UXF-011 three state vocabularies | S3 (1) | 0.75 | 2 | S (1) | 1.50 |
| UXF-014 flow labels 12 px, route names cut at 390 px | S3 (1) | 0.75 | 2 | S (1) | 1.50 |
| UXF-019 failures below the fold, no summary | S2 (3) | 0.75 | 1 | M (2) | 1.13 |
| UXF-018 Provider frame labels | S3 (1) | 0.50 | 2 | S (1) | 1.00 |
| UXF-021 section headings 13.3 px | S3 (1) | 1.00 | 1 | S (1) | 1.00 |
| UXF-004 key data, definition | S3 (1) | 1.00 | 2 | M (2) | 1.00 |
| UXF-013 idle flow box 180 px | S3 (1) | 0.25 | 2 | S (1) | 0.50 |
| UXF-020 balance tile alone on a row | S3 (1) | 0.50 | 1 | S (1) | 0.50 |
| UXF-017 no scroll cue at 390 px | S3 (1) | 0.75 | 1 | M (2) | 0.38 |
| UXF-005 shift when calls arrive | S3 (1) | 0.50 | 1 | M (2) | 0.25 |

No S1 finding. Both S2 findings are in the set.

## Change set R-06 (5)

1. UXF-010: limit blocks named by what they limit, not by a hash (the hash stays in a title);
   header words as readable labels; the attempts expander says "2 attempts". Target: the visible-text
   gate finds no raw identifier as a label.
2. UXF-016: a space or a line break before every second line. Target: the visible-text gate finds
   no run-together word.
3. UXF-011: one state vocabulary, one table of display words in the document. Target: a new
   assertion, each state shown with one spelling.
4. UXF-014: flow labels at the key-data size, route names not cut at 390 px (or their full name in
   the caption). Target: label size and cut count assertions.
5. UXF-019, with decision R-06 (decision.md): an alert strip under the header that names every
   current problem and links to its section. Target: the screen-only user finds the failing route
   and the watchdog state above the fold, in at most 2 actions each.

Next in line: UXF-018, UXF-021, UXF-004.

Planning: task:dashboard-ux-r06 under story:dashboard-ux-loop.
