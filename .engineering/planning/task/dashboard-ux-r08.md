---
format: aep.planning-md/3
id: task:dashboard-ux-r08
kind: task
status: draft
title: 'R-08 change set: session labels, strip links, Pods model column, Routes periods, scroll cue shift'
summary: Findings UXF-024, UXF-027, UXF-028, UXF-029 and UXF-030, each failing-first; beside them tool defects T25 to T30.
relations:
- decomposes: story:dashboard-ux-loop
- informed_by: task:dashboard-ux-r07
revision: 1
---
# Task: R-08 change set

## What

Fix UXF-024, UXF-027, UXF-028, UXF-029 and UXF-030 failing-first, and beside them fix tool defects
T25 to T30 of the R-07 report review.

## Why

story:dashboard-ux-loop takes each round's change set as one task; dashboard/ux/rounds/R-07/triage.md
ranked these five highest. UXF-030 is a regression of the R-07 scroll cue.

## Done When

Each finding has a red test before its fix and a closed record with its asset, and round R-08 shows
no session label cut to its id at 390x844, every strip link's heading below the sticky header, no
Pods row over two lines at 390x844, the period in each Routes column heading, and
layout_shift.per_state_max at 390x844 in errors and offline at or below its R-06 value.

## Notes

T25 first: the reduced-motion probe counts the page's 0.01 ms transitions as motion and can turn red
on a loaded host.
