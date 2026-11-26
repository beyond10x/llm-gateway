import type { Snapshot } from '../../src/extra.ts';

// A page interaction the replay performs after its last state, measured as one more state.
export type Action = 'expand-first-attempts' | 'show-all-calls';

export interface Scenario {
  name: string;
  description: string;
  // Anchor scenarios keep rounds comparable; a rotating one may change between rounds.
  anchor: boolean;
  // The states in replay order; `base` is the gateway's clock at the first state.
  states(base: number): Snapshot[];
  actions?: Action[];
}
