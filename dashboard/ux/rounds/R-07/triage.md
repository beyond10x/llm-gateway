# R-07 triage: the change set for R-08

Applied by rule (SKILL § 3.3): priority = severity weight × frequency × confidence ÷ cost.
Frequency is scenarios that show the defect ÷ 5 (idle, busy, token-flow, errors, offline), counted
from the R-07 shots, the review shots and the page source. Confidence is 2 when two sources saw it
(a reviewer of any round, the process rating, a metric), else 1. Severities are the records' own.

## Caveats

- Both reviews ran before this triage. The report reviewer reproduced the gating numbers (334 of
  334 tests; the 20 gating replays equal round.json in every metric) and ran the R-06 page under
  the R-07 harness to judge each change.
- The e2e step took 68 s wall on a quiet host, 115 s on a loaded one, and once ran into the 120 s
  limit under load with 103 tests done. The step is within budget on a quiet host; the margin under
  load is the reason T25 leads the tool work.
- The measured-only reduced-motion violation at 960x1080 is a probe artefact (T25): the probe
  counts the page's 0.01 ms transitions as motion.
- The metrics still mean less than their names in places: `copy.defects` misses a space removed
  after a colon or a comma (T27); `screen_only` is blind to colour functions and gradients and
  scores a short clamped jump as looks 2 (T28); the UXF-026 count test compares the page with itself
  (T29). Tool defects T25 to T30 (review-report.md) are fixed beside the change set and are not
  counted in it.
- New records: UXF-028 and UXF-029 (heuristic review), UXF-030 (the report review's layout shift of
  the new scroll cue at 390x844, which no ratchet sees).

## Did the last change set move its metrics?

| change | target | moved |
|---|---|---|
| UXF-021 heading size | heading_font.deficit_px → 0 | yes (R-06 page: 1.4 and 0.7 px) |
| UXF-026 failed-calls line | screen-only failing route at glance 1, 0 actions, busy and errors | yes (R-06 page: busy 2 actions glance 3, errors 1 action glance 3); now also reached at 390x844 and 960x1080 |
| UXF-018 Provider labels | its assertion | yes: red on the R-06 page, green now |
| UXF-020 balance tile row | its assertion | yes: red on the R-06 page, green now |
| UXF-017 scroll cue | its assertion | yes: red on the R-06 page, green now; it added layout shift at 390x844 (UXF-030) |

## Priorities

| finding | sev | freq | conf | cost | priority |
|---|---|---|---|---|---|
| UXF-024 session label loses the client at 390 px | S3 (1) | 0.60 | 2 | S (1) | 1.20 |
| UXF-027 Problems link lands under the sticky header at 390 px | S3 (1) | 0.40 | 2 | S (1) | 0.80 |
| UXF-028 Pods Model column 52 px at 390 px | S3 (1) | 0.60 | 1 | S (1) | 0.60 |
| UXF-029 5 minute count beside counts since start | S3 (1) | 0.60 | 1 | S (1) | 0.60 |
| UXF-030 scroll cue shifts the page at 390 px | S3 (1) | 0.40 | 1 | S (1) | 0.40 |
| UXF-013 idle flow panel tall for one line | S3 (1) | 0.20 | 2 | S (1) | 0.40 |
| UXF-025 offline page keeps drawing the last snapshot | S3 (1) | 0.20 | 2 | S (1) | 0.40 |
| UXF-005 shift when calls arrive | S3 (1) | 0.40 | 2 | M (2) | 0.40 |

No S1 or S2 finding is open. Four findings tie at 0.40; each of UXF-030, UXF-013 and UXF-005 shows
on an anchor card. UXF-030 takes the fifth place: it is a regression of the last change set, cost
S, on the errors anchor.

## Change set R-08 (5)

1. UXF-024: every session label in the token flow keeps its client name at 390x844. Target: an
   assertion that no label is cut to its id alone.
2. UXF-027: a Problems link brings its section's heading below the sticky header at every viewport.
   Target: an assertion over every strip link in busy and errors, the heading's top at or below the
   header's bottom.
3. UXF-028: the Pods model name stays on one line inside the scroll box. Target: an assertion that
   no Pods row is taller than two lines at 390x844.
4. UXF-029: the Routes columns say on screen what period each counts. Target: the visible-text
   probe finds the period in each column heading.
5. UXF-030: the scroll cue takes its line whenever its region can scroll, so it shifts nothing.
   Target: `layout_shift.per_state_max` at 390x844 in errors and offline back to its R-06 value or
   lower.

Next in line: UXF-013, UXF-025, UXF-005.

Planning: task:dashboard-ux-r08 under story:dashboard-ux-loop.
