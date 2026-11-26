import { snapshotAt } from '../build.ts';
import type { Scenario } from '../types.ts';

// A gateway that has just started: no call, no session, no route used, no provider panel.
export const idle: Scenario = {
  name: 'idle',
  description: 'A gateway just started: no calls, no sessions, no provider.',
  anchor: true,
  states: (base) => [snapshotAt(base, base - 42_000), snapshotAt(base + 2_000, base - 42_000)],
};
