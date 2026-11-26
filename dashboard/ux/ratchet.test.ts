// The ratchet refuses a worse value, an improvement not taken into the baseline and a missing value.
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { expect, test } from 'vitest';
import { compare, toleranceOf, type Baseline } from './ratchet.ts';

const baseline: Baseline = {
  about: 'test',
  scenarios: { s: { 'axe.serious_critical': { value: 2, tolerance: 0 }, 'layout_shift.per_state_max': { value: 0.05, tolerance: 0.005 } } },
  tolerances: {},
};

test('equal values and values within the tolerance pass', () => {
  expect(compare(baseline, { s: { 'axe.serious_critical': 2, 'layout_shift.per_state_max': 0.054 } })).toEqual([]);
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

test('every tolerance in the committed baseline is the one the ratchet applies', () => {
  const b = JSON.parse(readFileSync(fileURLToPath(new URL('./baseline.json', import.meta.url)), 'utf8')) as Baseline;
  for (const [scenario, metrics] of Object.entries(b.scenarios)) {
    for (const [metric, e] of Object.entries(metrics)) expect(e.tolerance, `${scenario} ${metric}`).toBe(toleranceOf(metric, e.value));
  }
});

test('the committed baseline names no round that is not committed', () => {
  const b = JSON.parse(readFileSync(fileURLToPath(new URL('./baseline.json', import.meta.url)), 'utf8')) as Baseline;
  const named = [...b.about.matchAll(/\bR-[0-9A-Za-z-]*[0-9A-Za-z]/g)].map((m) => m[0]).filter((r) => r !== 'R-nn');
  for (const r of named) expect(existsSync(fileURLToPath(new URL(`./rounds/${r}/round.json`, import.meta.url))), r).toBe(true);
  expect(b.about).not.toMatch(/scratch/i);
});
