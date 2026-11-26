---
format: aep.planning-md/3
id: story:dashboard-kpi-colours
kind: story
status: draft
title: The dashboard's colours follow live session KPIs from the interaction-record stream
summary: 'Idea, draft: the dashboard subscribes to the live stream of sessionspec IR records (agent-interaction records the gateway writes), derives per-session KPIs from them (e.g. autonomy, cache busts, waiting on the human, cost per session, error rate), and maps each KPI to colour: the token-flow strings, session labels and tiles change or vary their hue and intensity with the KPI''s value, so a session drifting into trouble shows by colour before anyone reads a number. Depends on the gateway writing interaction records and on sessionspec''s view model; colour mapping stays readable in both themes and under the loop''s contrast gates, and every colour has a word or number beside it.'
relations:
- informed_by: story:dashboard-ux-loop
revision: 1
---
<!-- Starting point for a `story` artifact, seeded by `aep artifact new story <name>`.
     No frontmatter here on purpose: the `---` block is written by the CLI from the id, kind, status
     and relations you gave it, and a second copy in this file would be the one that went stale.
     Delete the italic guidance as you fill each section. -->

# Story: <name>

## Outcome

*What is true for whom once this has shipped, in one sentence. If it names a component rather than a
person, it is a task — say what changes for someone.*

## Context

*Why this is worth doing now, and what it depends on. Link the epic or specification it comes from
rather than restating it; the `derived_from` relation already carries the edge.*

## Acceptance

*The conditions under which this is done, each one something a person or a check can observe. "Works
correctly" is not one of them.*

## Out of Scope

*What a reasonable reader would expect to be included and is not — the boundary that stops this
story quietly becoming an epic.*

## Ambiguities

*Each gap this story found and did not close, classified. `inferable` — the answer is already
written down, so give the `path:line` or the artifact id that settles it.
`requires-stakeholder-input` — nobody here can decide it, so name who does, and raise that entry as
a `decision-blocker` with a `blocks` edge to this story, or it is a sentence somebody improvises
later.*

## Open Questions

*Anything still undecided belongs in `## Ambiguities` above, classified and with its citation or its
decider. Keep this section for a question that is neither — an unowned question is a story that
stalls without anybody noticing.*
