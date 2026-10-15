---
format: aep.planning-md/3
id: architecture-decision-record:on-demand-gpu-lifecycle
kind: architecture-decision-record
status: proposed
title: The gateway owns the on-demand GPU lifecycle of its models, spend ceilings included
relations:
- decides: epic:hosting
revision: 1
---
## Status

Proposed. No implementing wave on the stories it governs until it is accepted.

## Context

The gateway already starts a Runpod pod on demand for a relayed model, holds a request while it
starts (`503` with `retry-after` past the hold budget), and stops it after its idle timeout
(`docs/hosting.md`, rows K27, K28, L6). A deployment that needs more is asking for the rest of a
GPU lifecycle to live in the gateway too:

- a spend ceiling, daily and total, read from the provider's billing plus a running estimate, that
  refuses a start which would pass it; today `ComputeAuthorization` is fixed `unmetered` and no
  budget ledger is consulted;
- an ordered list of model profiles (GPU type and the vLLM settings that fit it) walked when the
  provider has no stock for the first;
- owner routes to wake a model ahead of demand and to read its phase (renting, pulling, loading,
  ready);
- the same for any on-demand GPU provider, not Runpod alone.

Without these in the gateway, a deployment runs its own pod controller that edits the gateway's
configuration and secrets to repoint it, which needs write access to another component's objects.

## Decision (proposed)

The gateway owns the on-demand GPU lifecycle of the models it relays, spend control included:

1. A start is admitted only through a spend check against `daily_cap_usd` and `total_cap_usd`
   per provider account, from the provider's reported spend plus the gateway's running estimate of
   its live pods; an unknown spend refuses the start. This replaces the fixed `unmetered`
   authorization with a budget ledger.
2. A model may declare ordered profiles; a start that the provider refuses for capacity tries the
   next, within the same startup deadline and spend check.
3. `POST` wake and `GET` status routes per relayed model, owner-authenticated, the phase taken from
   the hosting state machine's existing phases.
4. These are written against the provider-neutral hosting contract (`b10x-llm-provision`), Runpod
   its first implementation.

## Consequences

- The gateway becomes the component that can spend money; the spend check is a safety boundary
  and needs its own adversary and security review, and a live test needs a funded account and an
  approved ceiling.
- Billing reads go through the connectors route already used for the control plane; a new
  connectors operation (account spend) is needed.
- External pod controllers and their write access to the gateway's objects become unnecessary.

## Alternatives

- Keep lifecycle outside: an external controller owns pods and spend, the gateway only relays to
  a static upstream (`[hosted.*]`). Simpler gateway, but every deployment rebuilds the controller.
- Spend caps only, no wake/status routes or profiles: the smallest safety gain, no new surface.

## Governs

`story:spend-caps`, `story:model-profile-fallback`, `story:model-wake-and-status`.
