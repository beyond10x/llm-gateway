import { busy } from './busy/scenario.ts';
import { errors } from './errors/scenario.ts';
import { idle } from './idle/scenario.ts';
import { offline } from './offline/scenario.ts';
import { tokenFlow } from './token-flow/scenario.ts';
import type { Scenario } from './types.ts';

export type { Action, Scenario, Truth } from './types.ts';

export const SCENARIOS: Scenario[] = [idle, busy, tokenFlow, errors, offline];
