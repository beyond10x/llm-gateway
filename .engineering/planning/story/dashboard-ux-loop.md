---
format: aep.planning-md/3
id: story:dashboard-ux-loop
kind: story
status: draft
title: The dashboard improves in measured UI rounds, gated on every merge
summary: 'A standing story of the UI improvement loop over the dashboard: each round''s change set is one task; acceptance is each finding''s regression asset in dashboard/e2e plus its target metric in dashboard/ux/baseline.json, never worse.'
relations:
- informed_by: story:live-dashboard
revision: 2
---
# Story: the dashboard improves in measured UI rounds

## What

The dashboard (dashboard/, built into crates/llm-gateway/src/dashboard.html) runs an improvement
loop of rounds: measure over hermetic scenarios, review by two fresh agents, triage by rule into a
change set of at most five findings, one design decision, build failing-first, gate, measure again.

## Why

The page is the owner's view of the gateway. Without a measured loop a change can make a reader's
task harder and nothing notices; with it every merge is held to hard gates and a ratchet.

## Done When

This story stays open while rounds run. Each round leaves under dashboard/ux/rounds/R-nn/ a report,
round.json, shots, review-heuristics.md, review-report.md and triage.md (and decision.md when a
layout question was put), one task per change set, and no ratchet value in dashboard/ux/baseline.json
worse than before.

## Notes

Gates: dashboard/e2e (layout, truncation, header in view, axe, keyboard, reduced motion, token
flow) at 1366x768 and 1280x720 in light and dark, with fonts pinned in the harness. Run locally with
`task dashboard-ux`; CI runs the same in the `dashboard` job of the Gate workflow.
