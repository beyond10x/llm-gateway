---
format: aep.planning-md/3
id: task:dashboard-ux-r03
kind: task
status: draft
title: 'R-03 change set: scroll regions, cache tile overflow, routes table, visible token flow, key-data font'
summary: Findings UXF-002, UXF-003, UXF-007, UXF-008 and UXF-004; each fixed failing-first against its e2e assertion, baseline moved in the same merge.
relations:
- decomposes: story:dashboard-ux-loop
revision: 3
---
# Task: R-03 change set

## What

Fix the five findings of the R-02 triage failing-first: UXF-002 (scroll regions without keyboard
access), UXF-003 (cache tile runs into its neighbour), UXF-007 (routes table cut at the right
edge), UXF-008 (token flow hidden behind the panels: a panel of its own, decision R-03) and UXF-004
(key data under 14 px).

## Why

story:dashboard-ux-loop takes each round's change set as one task. These five ranked highest in
dashboard/ux/rounds/R-02/triage.md; three are S2.

## Done When

Each finding has a red test commit before its fix commit, its record is closed with a regression
asset in dashboard/e2e, and round R-03 shows its target metric moved: axe serious/critical 2 → 0,
overflow.elements 1 → 0, Routes columns cut → 0, flow.visible_ratio ≥ 0.95, key-font deficit → 0.

## Notes

Outcome (R-03 and its reviews): four targets met; UXF-004 reached a deficit of 1.75 px of 3.6 px
and was reopened in the R-03 triage. Commits on the merged branch of pull request 68.
