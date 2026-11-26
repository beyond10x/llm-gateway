import { tokenFlow } from '../token-flow/scenario.ts';
import type { Scenario } from '../types.ts';

// A healthy gateway whose connection drops: two states of the token-flow traffic arrive on the
// stream, then the stream errors and the gateway answers no snapshot fetch, and the page's poll
// runs once. The page must say in words that it is not live.
export const offline: Scenario = {
  name: 'offline',
  description: 'Two states of traffic, then the stream errors and the snapshot poll fails.',
  anchor: false,
  states: (base) => tokenFlow.states(base).slice(0, 2),
  actions: ['lose-connection'],
  truth: { connection: { state: ['\\boffline\\b', '\\bdisconnected\\b', 'connection lost', 'not live'] } },
};
