---
format: aep.planning-md/3
id: story:serve-stop-within-a-bound
kind: story
status: draft
title: serve stops within a bound after SIGTERM
summary: SIGTERM ends serve within a declared bound even while a connection stays open, so a service manager never kills it.
revision: 2
---
## Problem

`serve` stops on SIGTERM by draining: `Serving::wait_for_stop` (`crates/llm-gateway-cli/src/serve.rs:174-193`) calls `begin_drain` and then `shutdown`, which returns once every connection already accepted is answered. No bound applies to that wait.

On 2026-11-27 the systemd user unit that runs `serve` was restarted to install 0.27.0. The 0.26.0 process had not exited when the unit's stop timeout (`TimeoutStopUSec=1min 30s`) ran out; systemd sent SIGKILL. The new process's first cleanup pass then reported `orphans=1` and stopped one pod that the killed process had left running.

A process killed this way writes none of the lines a graceful stop writes, and does not stop or record its own pods. The pod survived only because the next start found it.

## Outcome

A SIGTERM ends the process within a bound the document declares, and that bound is below a service manager's usual stop timeout. Connections still open at the bound are closed. The stop lines are written, and the pods the process owns are handled as a graceful stop handles them.

## Acceptance

- A test holds one streamed call open past the bound, sends SIGTERM, and sees the process exit 0 within the bound plus 1 s, with its stop lines written.
- A test where every connection ends before the bound sees the same stop as today.
