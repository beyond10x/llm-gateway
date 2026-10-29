---
format: aep.planning-md/3
id: upstream-blocker:connectors-openai-connector
kind: upstream-blocker
status: open
title: No connectors release ships an OpenAI api-key connector
relations:
- blocks: story:hosted-credential-from-connectors
revision: 1
---
## What is missing

No beyond10x/connectors release ships an OpenAI api-key connector. `connectors list` on 0.59.1
shows `runpod.api-key`, `chatgpt.subscription`, `claude.subscription` and others, and no OpenAI
entry. Without one, the OpenAI API key for live qualification cannot be held in connectors, and
`story:hosted-credential-from-connectors` has no connection to launch the gateway under.

## Cleared when

A connectors release carries an OpenAI api-key connector whose connection
`connectors connections launch` can hand to a consumer on descriptor 3. Record its tag here.

## Not done from here

Nothing is written in the connectors repository from this one; the request is made there.
