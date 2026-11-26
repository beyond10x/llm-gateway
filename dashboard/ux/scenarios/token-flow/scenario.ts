import type { InFlight, Snapshot } from '../../../src/extra.ts';
import { Ledger, inFlight, type SessionDef } from '../build.ts';
import type { Scenario } from '../types.ts';

const A: SessionDef = { session: '5d0c7e19a2b4', label: 'claude-code' };
const B: SessionDef = { session: 'e83f10b6c95d', label: 'codex' };
const C: SessionDef = { session: '1a7b2fe04c38', label: 'opencode' };

const STEP_MS = 4_000;

// Eight states, 4 s apart on the gateway's clock: session A starts on two Anthropic routes, B joins
// on the Runpod route, C joins on two routes, and A ends after state 5 (no more calls, nothing in
// flight). Each state adds calls with tokens, so rows enter the tables at the top.
interface Step {
  calls: { s: SessionDef; route: string; input: number; output: number; cacheRead?: number }[];
  flying: { s: SessionDef; route: string; since: number; state?: InFlight['state'] }[];
}

const STEPS: Step[] = [
  { calls: [], flying: [{ s: A, route: 'claude-sonnet', since: 1_500 }] },
  { calls: [{ s: A, route: 'claude-sonnet', input: 18_420, output: 612, cacheRead: 41_000 }], flying: [{ s: A, route: 'claude-haiku', since: 800 }] },
  {
    calls: [{ s: A, route: 'claude-haiku', input: 3_210, output: 188, cacheRead: 9_400 }],
    flying: [
      { s: A, route: 'claude-sonnet', since: 2_200 },
      { s: B, route: 'qwen-coder', since: 600 },
    ],
  },
  {
    calls: [
      { s: B, route: 'qwen-coder', input: 24_980, output: 1_402 },
      { s: A, route: 'claude-sonnet', input: 22_004, output: 941, cacheRead: 59_420 },
    ],
    flying: [{ s: B, route: 'qwen-coder', since: 1_100 }],
  },
  {
    calls: [{ s: C, route: 'claude-haiku', input: 1_240, output: 96, cacheRead: 0 }],
    flying: [
      { s: B, route: 'qwen-coder', since: 3_400 },
      { s: C, route: 'qwen-coder', since: 400 },
      { s: A, route: 'claude-sonnet', since: 900 },
    ],
  },
  {
    calls: [
      { s: A, route: 'claude-sonnet', input: 26_771, output: 2_310, cacheRead: 81_200 },
      { s: B, route: 'qwen-coder', input: 31_502, output: 2_048 },
    ],
    flying: [{ s: C, route: 'qwen-coder', since: 2_600 }],
  },
  {
    calls: [{ s: C, route: 'qwen-coder', input: 8_806, output: 744 }],
    flying: [
      { s: B, route: 'qwen-coder', since: 1_300 },
      { s: C, route: 'claude-haiku', since: 700 },
    ],
  },
  {
    calls: [
      { s: C, route: 'claude-haiku', input: 2_118, output: 301, cacheRead: 1_240 },
      { s: B, route: 'qwen-coder', input: 36_090, output: 1_877 },
    ],
    flying: [{ s: B, route: 'qwen-coder', since: 500 }],
  },
];

export const tokenFlow: Scenario = {
  name: 'token-flow',
  description: '8 states: calls with tokens appear for 3 sessions over 3 routes; sessions start, one ends.',
  anchor: false,
  states(base: number): Snapshot[] {
    const ledger = new Ledger();
    const started = base - 90_000;
    return STEPS.map((step, i) => {
      const now = base + i * STEP_MS;
      step.calls.forEach((c, k) => ledger.add({ session: c.s, route: c.route, at: now - 300 - k * 200, input: c.input, output: c.output, cacheRead: c.cacheRead }));
      const flying = step.flying.map((f, k) => inFlight(9000 + i * 10 + k, f.s, f.route, now, now - f.since, f.state));
      return ledger.snapshot(now, started, { inFlight: flying });
    });
  },
};
