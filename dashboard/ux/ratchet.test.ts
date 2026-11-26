// The ratchet refuses a worse value, an improvement not taken into the baseline and a missing value.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { expect, test } from 'vitest';
import { compare, type Baseline } from './ratchet.ts';

const baseline: Baseline = {
  about: 'test',
  scenarios: { s: { 'axe.serious_critical': { value: 2, tolerance: 0 }, 'layout_shift.per_state_max': { value: 0.05, tolerance: 0.01 } } },
  tolerances: {},
};

test('equal values and values within the tolerance pass', () => {
  expect(compare(baseline, { s: { 'axe.serious_critical': 2, 'layout_shift.per_state_max': 0.059 } })).toEqual([]);
});

test('a worse value fails', () => {
  expect(compare(baseline, { s: { 'axe.serious_critical': 3, 'layout_shift.per_state_max': 0.05 } }).map((p) => p.kind)).toEqual(['worse']);
});

test('an improvement not taken into the baseline fails', () => {
  expect(compare(baseline, { s: { 'axe.serious_critical': 2, 'layout_shift.per_state_max': 0.03 } }).map((p) => p.kind)).toEqual(['improved-not-taken']);
});

test('a missing value fails, on either side', () => {
  expect(compare(baseline, { s: { 'axe.serious_critical': 2 } }).map((p) => p.kind)).toEqual(['missing-measured']);
  expect(compare(baseline, { s: { 'axe.serious_critical': 2, 'layout_shift.per_state_max': 0.05, 'new.metric': 0 } }).map((p) => p.kind)).toEqual(['missing-baseline']);
  expect(compare(baseline, {}).map((p) => p.kind)).toEqual(['missing-measured', 'missing-measured']);
});

test('the committed baseline has the loop shape and covers every scenario', async () => {
  const file = fileURLToPath(new URL('./baseline.json', import.meta.url));
  const b = JSON.parse(readFileSync(file, 'utf8')) as Baseline;
  const { SCENARIOS } = await import('./scenarios/index.ts');
  expect(typeof b.about).toBe('string');
  expect(Object.keys(b.scenarios).sort()).toEqual(SCENARIOS.map((s) => s.name).sort());
  for (const metrics of Object.values(b.scenarios)) {
    for (const e of Object.values(metrics)) {
      expect(Number.isFinite(e.value)).toBe(true);
      expect(Number.isFinite(e.tolerance)).toBe(true);
    }
  }
});
