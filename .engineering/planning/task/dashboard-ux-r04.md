---
format: aep.planning-md/3
id: task:dashboard-ux-r04
kind: task
status: draft
title: 'R-04 change set: theme toggle at 390 px, flow panel height, status word wrap, stale heartbeat in words, flow caption and contrast'
summary: Findings UXF-015, UXF-013, UXF-006, UXF-009, UXF-012 with UXF-014; tool defects N1 to N5 fixed before R-04 is measured.
relations:
- decomposes: story:dashboard-ux-loop
revision: 2
---
# Task: R-04 change set

## What

Fix the five findings of the R-03 triage failing-first: UXF-015 (theme toggle clipped at
390x844), UXF-013 (token-flow panel grows when sessions arrive), UXF-006 (status value broken
inside a word), UXF-009 (stale heartbeat in colour only), UXF-012 with UXF-014 (token-flow caption
and contrast); fix tool defects N1 to N5 before measuring.

## Why

story:dashboard-ux-loop takes each round's change set as one task; these ranked highest in
dashboard/ux/rounds/R-03/triage.md.

## Done When

Each finding has a red test before its fix and a closed record with its regression asset, and round
R-04 shows header.hidden_ratio 0 at every viewport, layout shift 0 at the first-session state at
390x844, no word broken in a status value, "stale" in words, caption and contrast assertions green.

## Notes

R-04 was measured but not reviewed and not triaged before the next round started; the R-05 process
rating records this. UXF-015 turned out to be a defect of the probe. UXF-013 and UXF-014 were
reopened after the rating.
