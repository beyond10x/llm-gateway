import type { Snapshot } from '../../src/extra.ts';

// A page interaction the replay performs after its last state, measured as one more state.
// `lose-connection` is not a click: the event stream errors and every later snapshot fetch fails,
// then the page's poll runs once.
export type Action = 'expand-first-attempts' | 'show-all-calls' | 'lose-connection';

// What the right end state shows, in words. Each list holds regular expressions (case-insensitive)
// of which one must match. The screen-only user (e2e/screen-only.ts) reaches a task when one
// element in view shows a match of every list of the task.
export interface Truth {
  // The route with errors and the words of its cause.
  failing_route?: { route: string; cause: string[] };
  // The watchdog heartbeat's state in words.
  watchdog?: { state: string[] };
  // The connection status after the replay lost its connection, in words.
  connection?: { state: string[] };
}

export interface Scenario {
  name: string;
  description: string;
  // Anchor scenarios keep rounds comparable; a rotating one may change between rounds.
  anchor: boolean;
  // The states in replay order; `base` is the gateway's clock at the first state.
  states(base: number): Snapshot[];
  actions?: Action[];
  truth?: Truth;
}
