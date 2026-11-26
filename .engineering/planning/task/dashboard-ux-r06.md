---
format: aep.planning-md/3
id: task:dashboard-ux-r06
kind: task
status: draft
title: 'R-06 change set: limit names, run-together words, one state vocabulary, flow labels, alert strip'
summary: 'Findings UXF-010, UXF-016, UXF-011, UXF-014 and UXF-019 (decision R-06), each failing-first; beside them the tool work of the R-05 triage: T13 to T17, a visible-text gate, the screen-only user on two tasks, a connection-loss replay, shots below the fold.'
relations:
- decomposes: story:dashboard-ux-loop
revision: 2
---
# Task: R-06 change set

## What

Fix UXF-010, UXF-016, UXF-011, UXF-014 and UXF-019 (an alert strip, decision R-06) failing-first,
and beside them build the tools of the R-05 triage: a visible-text gate, the screen-only user on two
tasks, probe fixes T13 to T17, an offline replay and full-page shots.

## Why

story:dashboard-ux-loop takes each round's change set as one task; dashboard/ux/rounds/R-05/triage.md
ranked these highest, with both open S2 findings among them.

## Done When

Each finding has a red test before its fix, a closed record with its asset, and round R-06 shows
copy items 0, one state vocabulary, flow labels at 14 px, and every failure kind on the first
screen at 1280x720.

## Notes

Outcome (R-06 reviews): four of five targets moved; UXF-014's assertions read the flow model, not
the canvas, so its movement is not observable; the failing-route task reaches glance 2 or 3, which
UXF-026 takes into R-07.
