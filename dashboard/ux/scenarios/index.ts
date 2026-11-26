import { busy } from './busy/scenario.ts';
import { errors } from './errors/scenario.ts';
import { idle } from './idle/scenario.ts';
import { tokenFlow } from './token-flow/scenario.ts';
import type { Scenario } from './types.ts';

export type { Action, Scenario } from './types.ts';

export const SCENARIOS: Scenario[] = [idle, busy, tokenFlow, errors];
