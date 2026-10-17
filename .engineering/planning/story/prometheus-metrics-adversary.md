---
format: aep.planning-md/3
id: story:prometheus-metrics-adversary
kind: story
status: draft
title: An adversarial pass over the Prometheus metrics finds nothing the suite would not catch
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
- depends_on: story:prometheus-metrics
revision: 1
---
## Story

The Prometheus metrics of 0.12.0 shipped without an adversarial pass, because they are off by
default and read-only. This story runs that pass: try to break the scrape listener, the latency
histogram and the outcome counter, and fix what it confirms.

## Acceptance

- An adversary pass over story:prometheus-metrics records each finding as a failing test or a
  judgement; every confirmed finding is fixed and its test passes in the conformance suite or the
  crate's tests.
- Attack surface named: the scrape listener's bind and guard (no owner credential, nothing but
  metrics served), label cardinality from client-chosen model names, counter and histogram
  overflow and concurrency, and that no series carries a credential, prompt or document content.
