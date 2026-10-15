---
format: aep.planning-md/3
id: story:model-profile-fallback
kind: story
status: draft
title: A model starts on its next declared profile when the provider has no capacity for the first
relations:
- decomposes: epic:hosting
- informed_by: architecture-decision-record:on-demand-gpu-lifecycle
revision: 1
---
## Outcome

A relayed model can declare an ordered list of profiles (GPU type with the vLLM settings that fit
it); when the provider refuses a start for capacity, the next profile is tried, so a model starts
when its first choice is out of stock.

## Acceptance

1. `[models.<alias>]` accepts an ordered `profiles` list, each a GPU type list plus its vLLM
   settings (context length at least); the existing single declaration is the one-profile case.
2. A create the emulated provider refuses for capacity is retried with the next profile within the
   same startup deadline and spend check; any other refusal is not retried. A test drives each.
3. The status and usage record name the profile that served.

## Blocked

By `decision-blocker:on-demand-gpu-lifecycle`.
