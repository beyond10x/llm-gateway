import { fixture } from '../../../src/fixture.ts';
import { CAUSE_429 } from '../build.ts';
import type { Scenario } from '../types.ts';

// The page's own development fixture: 40 calls over three routes and three sessions, three in
// flight, two credentials' limits and a provider with three pods.
export const busy: Scenario = {
  name: 'busy',
  description: "The page's development fixture: 40 calls, 3 in flight, limits, a provider with 3 pods.",
  anchor: true,
  states: (base) => [fixture(base), fixture(base + 1_000), fixture(base + 2_000)],
  actions: ['show-all-calls'],
  // claude-sonnet has the most errors (2), each a 429; the heartbeat is 7 minutes old.
  truth: { failing_route: { route: 'claude-sonnet', cause: CAUSE_429 }, watchdog: { state: ['stale'] } },
};
