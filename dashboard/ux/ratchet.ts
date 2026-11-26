// The ratchet: per scenario, the worst value of every metric over the gating viewports and themes,
// compared with ux/baseline.json. Every metric is lower-is-better.
import type { Measurement } from '../e2e/measure.ts';

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

export const TOLERANCES: Record<string, number> = {
  counts: 0,
  'layout_shift.per_state_max': 0.01,
  'header.hidden_ratio': 0,
  'key_font.deficit_px': 0,
};

export function toleranceOf(metric: string): number {
  return TOLERANCES[metric] ?? TOLERANCES.counts!;
}

export type Aggregate = Record<string, Record<string, number>>;

export function aggregate(measurements: Measurement[]): Aggregate {
  const out: Aggregate = {};
  for (const m of measurements) {
    const row = (out[m.scenario] ??= {});
    for (const [k, v] of Object.entries(m.metrics)) row[k] = Math.max(row[k] ?? -Infinity, v);
  }
  return out;
}

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
      const entry = b[metric];
      const value = m[metric];
      if (entry === undefined) {
        problems.push({ scenario, metric, kind: 'missing-baseline', baseline: null, measured: value ?? null });
      } else if (value === undefined || !Number.isFinite(value)) {
        problems.push({ scenario, metric, kind: 'missing-measured', baseline: entry.value, measured: null });
      } else if (value > entry.value + entry.tolerance + EPSILON) {
        problems.push({ scenario, metric, kind: 'worse', baseline: entry.value, measured: value });
      } else if (value < entry.value - entry.tolerance - EPSILON) {
        problems.push({ scenario, metric, kind: 'improved-not-taken', baseline: entry.value, measured: value });
      }
    }
  }
  return problems;
}

export function baselineFrom(measured: Aggregate, about: string): Baseline {
  const scenarios: Baseline['scenarios'] = {};
  for (const scenario of Object.keys(measured).sort()) {
    scenarios[scenario] = {};
    for (const metric of Object.keys(measured[scenario]!).sort()) {
      scenarios[scenario]![metric] = { value: measured[scenario]![metric]!, tolerance: toleranceOf(metric) };
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
