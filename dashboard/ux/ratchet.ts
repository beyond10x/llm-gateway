// The ratchet: per scenario, the worst value of every metric over the gating viewports and themes,
// compared with ux/baseline.json. Every metric is lower-is-better except the token-flow keys
// (HIGHER_IS_BETTER), whose worst value is the smallest. Besides the fixed metrics it counts the
// nodes of every axe rule (`axe.rule.<id>`), so one violation swapped for another of the same
// count fails: a rule absent on one side counts 0 there. A scenario with screen-only tasks also
// has the screen-only user's keys, and the sideways scrollers at each measured-only viewport are
// ratcheted (`overflow.scrollers.<viewport>`) without a gate.
import type { Measurement } from '../e2e/measure.ts';
import { AXE_RULE_PREFIX, flowMetrics, HIGHER_IS_BETTER, MEASURED_ONLY_SCROLLERS, screenOnlyMetrics, type ScreenOnlyResult } from '../e2e/metrics.ts';

export interface BaselineEntry {
  value: number;
  tolerance: number;
  decision?: string;
}

export interface Baseline {
  about: string;
  scenarios: Record<string, Record<string, BaselineEntry>>;
  tolerances: Record<string, number>;
}

// Counts and every other value: 0. Layout shift: relative to the baseline value, at least
// LAYOUT_SHIFT_FLOOR (the run-to-run noise is about 0.001), else LAYOUT_SHIFT_RELATIVE of the value.
export const LAYOUT_SHIFT_FLOOR = 0.002;
export const LAYOUT_SHIFT_RELATIVE = 0.1;

export const TOLERANCES: Record<string, number> = {
  counts: 0,
  'layout_shift.per_state_max.floor': LAYOUT_SHIFT_FLOOR,
  'layout_shift.per_state_max.relative': LAYOUT_SHIFT_RELATIVE,
};

// The tolerance of a metric around its baseline value.
export function toleranceOf(metric: string, baseline: number): number {
  if (metric === 'layout_shift.per_state_max') return Math.round(Math.max(LAYOUT_SHIFT_FLOOR, LAYOUT_SHIFT_RELATIVE * baseline) * 10000) / 10000;
  return TOLERANCES.counts!;
}

export type Aggregate = Record<string, Record<string, number>>;

export function aggregate(measurements: (Pick<Measurement, 'scenario' | 'metrics' | 'axe_rules'> & Partial<Pick<Measurement, 'flow' | 'reduced_flow'>> & { screen_only?: ScreenOnlyResult[] })[]): Aggregate {
  const out: Aggregate = {};
  for (const m of measurements) {
    const row = (out[m.scenario] ??= {});
    for (const [k, v] of Object.entries(m.metrics)) row[k] = Math.max(row[k] ?? -Infinity, v);
    if (m.flow && m.reduced_flow) for (const [k, v] of Object.entries(flowMetrics({ flow: m.flow, reduced_flow: m.reduced_flow }))) row[k] = Math.min(row[k] ?? Infinity, v);
    for (const [k, v] of Object.entries(screenOnlyMetrics(m.screen_only ?? []))) {
      const worse = HIGHER_IS_BETTER.has(k) ? Math.min : Math.max;
      row[k] = worse(row[k] ?? (HIGHER_IS_BETTER.has(k) ? Infinity : -Infinity), v);
    }
    for (const [rule, v] of Object.entries(m.axe_rules ?? {})) {
      const k = `${AXE_RULE_PREFIX}${rule}`;
      row[k] = Math.max(row[k] ?? -Infinity, v);
    }
  }
  return out;
}

// The sideways scrollers at each measured-only viewport, the worst over the themes, as
// `overflow.scrollers.<viewport>` per scenario.
export function measuredOnlyAggregate(rows: { scenario: string; viewport: string; scrollers: number }[]): Aggregate {
  const out: Aggregate = {};
  for (const r of rows) {
    const row = (out[r.scenario] ??= {});
    const k = MEASURED_ONLY_SCROLLERS(r.viewport);
    row[k] = Math.max(row[k] ?? 0, r.scrollers);
  }
  return out;
}

export function mergeAggregates(a: Aggregate, b: Aggregate): Aggregate {
  const out: Aggregate = {};
  for (const s of new Set([...Object.keys(a), ...Object.keys(b)])) out[s] = { ...a[s], ...b[s] };
  return out;
}

const zeroWhenAbsent = (metric: string) => metric.startsWith(AXE_RULE_PREFIX);

export interface Problem {
  scenario: string;
  metric: string;
  kind: 'worse' | 'improved-not-taken' | 'missing-measured' | 'missing-baseline';
  baseline: number | null;
  measured: number | null;
}

const EPSILON = 1e-9;

export function compare(baseline: Baseline, measured: Aggregate): Problem[] {
  const problems: Problem[] = [];
  const scenarios = new Set([...Object.keys(baseline.scenarios), ...Object.keys(measured)]);
  for (const scenario of [...scenarios].sort()) {
    const b = baseline.scenarios[scenario] ?? {};
    const m = measured[scenario] ?? {};
    const metrics = new Set([...Object.keys(b), ...Object.keys(m)]);
    for (const metric of [...metrics].sort()) {
      const zero = zeroWhenAbsent(metric);
      const entry = b[metric] ?? (zero ? { value: 0, tolerance: 0 } : undefined);
      const value = m[metric] ?? (zero ? 0 : undefined);
      if (entry === undefined) {
        problems.push({ scenario, metric, kind: 'missing-baseline', baseline: null, measured: value ?? null });
      } else if (value === undefined || !Number.isFinite(value)) {
        problems.push({ scenario, metric, kind: 'missing-measured', baseline: entry.value, measured: null });
      } else {
        // Worse is up for lower-is-better metrics, down for the others. The tolerance is a band on
        // the worse side only: any improvement, also one inside the band, must be taken into the
        // baseline (baselineFrom takes it). The replays are hermetic and the fonts pinned, so a
        // repeated measurement gives the same value; a value that moves down is a change.
        const sign = HIGHER_IS_BETTER.has(metric) ? -1 : 1;
        const tolerance = toleranceOf(metric, entry.value);
        const delta = sign * (value - entry.value);
        if (delta > tolerance + EPSILON) problems.push({ scenario, metric, kind: 'worse', baseline: entry.value, measured: value });
        else if (delta < -EPSILON) problems.push({ scenario, metric, kind: 'improved-not-taken', baseline: entry.value, measured: value });
      }
    }
  }
  return problems;
}

// A baseline from measured values. With `previous`: a value better than it is taken, also inside
// the tolerance, so a value can never creep back up through the band; a value worse than it but
// inside the tolerance keeps the previous value (run-to-run noise does not move the baseline up);
// a value worse beyond the tolerance is taken and carries the decision id; an unchanged entry keeps
// the decision it had.
export function baselineFrom(measured: Aggregate, about: string, previous?: Baseline, decision?: string): Baseline {
  const scenarios: Baseline['scenarios'] = {};
  for (const scenario of Object.keys(measured).sort()) {
    scenarios[scenario] = {};
    for (const metric of Object.keys(measured[scenario]!).sort()) {
      const before = previous?.scenarios[scenario]?.[metric];
      const sign = HIGHER_IS_BETTER.has(metric) ? -1 : 1;
      const measuredValue = measured[scenario]![metric]!;
      const delta = before ? sign * (measuredValue - before.value) : 0;
      const worseBeyond = before !== undefined && delta > toleranceOf(metric, before.value) + EPSILON;
      const value = before && delta > EPSILON && !worseBeyond ? before.value : measuredValue;
      const entry: BaselineEntry = { value, tolerance: toleranceOf(metric, value) };
      if (worseBeyond && decision) entry.decision = decision;
      else if (before?.decision && Math.abs(before.value - value) <= EPSILON) entry.decision = before.decision;
      scenarios[scenario]![metric] = entry;
    }
  }
  return { about, scenarios, tolerances: { ...TOLERANCES } };
}

export function describeProblem(p: Problem): string {
  const what = {
    worse: 'worse than the baseline',
    'improved-not-taken': 'better than the baseline: take it into ux/baseline.json in this change',
    'missing-measured': 'in the baseline but not measured',
    'missing-baseline': 'measured but not in the baseline',
  }[p.kind];
  return `${p.scenario} ${p.metric}: ${what} (baseline ${p.baseline ?? '–'}, measured ${p.measured ?? '–'})`;
}
