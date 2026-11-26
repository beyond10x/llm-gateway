// Break tests of the measuring tools that need no browser: each feeds a tool a bad input and
// asserts that it reports it.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { afterAll, expect, test } from 'vitest';
import { KNOWN_FAILURES, stale, unexpected, wholeCheck, type KnownFailure } from '../e2e/known-failures.ts';
import { fmtKeyFont, keyFontDeficit, mergeAxe, METRIC_KEYS, type AxeFinding, type Metrics } from '../e2e/metrics.ts';
import { METRICS_ROOT, metricsDir, readMeasurements } from '../e2e/metrics-dir.ts';
import { aggregate, baselineFrom, compare, type Baseline } from './ratchet.ts';
import { baselineRefusal, missingGateReplays, roundDirRefusal, toolsOf } from './round-guard.ts';

mkdirSync(METRICS_ROOT, { recursive: true });
const scratch = mkdtempSync(join(METRICS_ROOT, 'unit-'));
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

const zeros = Object.fromEntries(METRIC_KEYS.map((k) => [k, 0])) as unknown as Metrics;

test('metrics of two runs go to two directories, outside the Playwright output directory', () => {
  const a = metricsDir('round', 'R-01-1');
  const b = metricsDir('round', 'R-01-2');
  expect(a).not.toEqual(b);
  expect(a).not.toMatch(/test-results/);
  expect(a.startsWith(METRICS_ROOT)).toBe(true);
});

test('break: a measurement another run wrote is refused', () => {
  const dir = join(scratch, 'gate');
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, 'idle__1366x768__light.json'), JSON.stringify({ run: 'other', scenario: 'idle' }));
  expect(() => readMeasurements(dir, 'mine')).toThrow(/belongs to run other/);
  expect(readMeasurements(dir, 'other')).toHaveLength(1);
});

test('break: a round directory that holds a report is refused without --force', () => {
  const dir = join(scratch, 'R-99');
  mkdirSync(dir, { recursive: true });
  expect(roundDirRefusal(dir, false)).toBeNull();
  writeFileSync(join(dir, 'review-report.md'), 'a review');
  expect(roundDirRefusal(dir, false)).toBeNull();
  writeFileSync(join(dir, 'round.json'), '{}');
  expect(roundDirRefusal(dir, false)).toMatch(/already holds round.json/);
  expect(roundDirRefusal(dir, true)).toBeNull();
});

test('break: --baseline is refused when a gate replay is missing or incomplete, or a value got worse without a decision', () => {
  const gate = { scenarios: ['idle', 'busy'], viewports: ['1366x768'], themes: ['light'] };
  const m = (scenario: string, metrics: Metrics = zeros) => ({ scenario, viewport: { name: '1366x768', width: 1366, height: 768, gating: true }, theme: 'light' as const, metrics });
  expect(missingGateReplays([m('idle'), m('busy')], gate)).toEqual([]);
  expect(missingGateReplays([m('idle')], gate)).toEqual(['busy 1366x768 light']);
  expect(missingGateReplays([m('idle'), m('busy', { ...zeros, 'overflow.x_px': NaN })], gate)).toEqual(['busy 1366x768 light']);
  expect(baselineRefusal(['busy 1366x768 light'], [], 'D1')).toMatch(/1 gate replay\(s\) not measured/);
  const worse = [{ scenario: 'idle', metric: 'overflow.x_px', kind: 'worse' as const, baseline: 0, measured: 3 }];
  expect(baselineRefusal([], worse, undefined)).toMatch(/--decision/);
  expect(baselineRefusal([], worse, 'D7')).toBeNull();
  const previous: Baseline = { about: '', scenarios: { idle: { 'overflow.x_px': { value: 0, tolerance: 0 } } }, tolerances: {} };
  expect(baselineFrom({ idle: { 'overflow.x_px': 3 } }, '', previous, 'D7').scenarios.idle!['overflow.x_px']).toEqual({ value: 3, tolerance: 0, decision: 'D7' });
});

test('break: a round measured on two browser or axe versions is refused', () => {
  expect(toolsOf([{ tools: { chromium: '1', axe_core: '4' } }, { tools: { chromium: '1', axe_core: '4' } }])).toEqual({ chromium: '1', axe_core: '4' });
  expect(() => toolsOf([{ tools: { chromium: '1', axe_core: '4' } }, { tools: { chromium: '2', axe_core: '4' } }])).toThrow(/different tools/);
  expect(() => toolsOf([{ tools: { chromium: '', axe_core: '4' } }])).toThrow(/no Chromium/);
});

test('break: axe nodes are the union over every scan, not the largest scan', () => {
  const union = new Map<string, AxeFinding>();
  const f = (targets: string[]): AxeFinding => ({ id: 'rule', impact: 'serious', help: '', nodes: targets.length, targets });
  mergeAxe(union, [f(['#a'])]);
  mergeAxe(union, [f(['#b'])]);
  mergeAxe(union, [f(['#a'])]);
  expect(union.get('rule')).toMatchObject({ nodes: 2, targets: ['#a', '#b'] });
});

test('break: a known failure holds its own items only; a new item in the same gate fails, and a fixed item goes stale', () => {
  const list: KnownFailure[] = [{ finding: 'UXF-900', check: 'axe', scenarios: ['busy'], items: ['rule-a #x'] }, { finding: 'UXF-901', check: 'flow' }];
  expect(unexpected('axe', 'busy', ['rule-a #x'], list)).toEqual([]);
  expect(unexpected('axe', 'busy', ['rule-a #x', 'rule-a #y', 'rule-b #x'], list)).toEqual(['rule-a #y', 'rule-b #x']);
  expect(unexpected('axe', 'idle', ['rule-a #x'], list)).toEqual(['rule-a #x']);
  expect(stale('axe', 'busy', new Set(), list)).toEqual(['UXF-900: rule-a #x']);
  expect(stale('axe', 'busy', new Set(['rule-a #x']), list)).toEqual([]);
  expect(wholeCheck('flow', 'busy', list)).toBe('UXF-901');
  expect(wholeCheck('axe', 'busy', list)).toBeNull();
});

test('every known failure names its items, except a whole single-valued check', () => {
  for (const k of KNOWN_FAILURES) expect(k.check === 'flow' || (k.items?.length ?? 0) > 0, k.finding).toBe(true);
});

test('break: the ratchet counts per axe rule, so a violation swapped for another of the same count fails', () => {
  const m = (rules: Record<string, number>) => ({ scenario: 's', metrics: { ...zeros, 'axe.serious_critical': 2 }, axe_rules: rules });
  const measured = aggregate([m({ 'rule-a': 2 })]);
  const baseline = baselineFrom(measured, 'test');
  expect(compare(baseline, measured)).toEqual([]);
  const swapped = compare(baseline, aggregate([m({ 'rule-b': 2 })]));
  expect(swapped.map((p) => `${p.metric} ${p.kind}`)).toEqual(['axe.rule.rule-a improved-not-taken', 'axe.rule.rule-b worse']);
});

test('break: a page without key data reads n/a and no deficit, not a deficit of 14 px', () => {
  expect(keyFontDeficit(null)).toBe(0);
  expect(fmtKeyFont(null)).toBe('n/a');
  expect(keyFontDeficit(10.4)).toBe(3.6);
  expect(keyFontDeficit(16)).toBe(0);
});
