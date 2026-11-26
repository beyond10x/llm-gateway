---
format: aep.planning-md/3
id: task:dashboard-ux-r07
kind: task
status: draft
title: 'R-07 change set: heading size, failing-route line, Provider labels, balance tile, scroll cue'
summary: Findings UXF-021, UXF-026, UXF-018, UXF-020 and UXF-017, each failing-first; beside them tool defects T18 to T24 and the gate step back under 70 s wall.
relations:
- decomposes: story:dashboard-ux-loop
revision: 2
---
# Task: R-07 change set

## What

Fix UXF-021, UXF-026, UXF-018, UXF-020 and UXF-017 failing-first, and beside them fix tool defects
T18 to T24 of the R-06 report review and bring the gate step back under 70 s wall.

## Why

story:dashboard-ux-loop takes each round's change set as one task; dashboard/ux/rounds/R-06/triage.md
ranked these five highest. UXF-026 carries the half of UXF-019 that R-06 did not move.

## Done When

Each finding has a red test before its fix, a closed record with its asset, and round R-07 shows
heading_font.deficit_px 0, the screen-only failing-route task at glance 1 with 0 actions in busy and
errors, assertions on the Provider heading, the balance tile row and each scroll cue, and the e2e
step under 70 s wall.

## Notes

Outcome (R-07 reviews): all five targets moved, judged with the R-06 page under the R-07 harness.
The e2e step ran 68 s wall on a quiet host and 115 s on a loaded one; T25 takes the margin. The
scroll cue added layout shift at 390x844, filed as UXF-030 and taken into R-08.
