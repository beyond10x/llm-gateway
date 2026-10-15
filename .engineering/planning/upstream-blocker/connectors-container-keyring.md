---
format: aep.planning-md/3
id: upstream-blocker:connectors-container-keyring
kind: upstream-blocker
status: open
title: No connectors release ships a container artifact and a Secret-backed keyring
relations:
- blocks: story:in-pod-connectors
revision: 1
---
## What is missing

A gateway in a Kubernetes pod reaches the Runpod control plane through the `connectors` CLI. No
beyond10x/connectors release ships a container artifact of that CLI, and its keyring cannot yet be
read from a mounted Secret file.

## Cleared when

A connectors release carries both. Record its tag here.
