// The ratchet: per scenario, the worst value of every metric over the gating viewports and themes,
// compared with ux/baseline.json. Every metric is lower-is-better. Besides the fixed metrics it
// counts the nodes of every axe rule (`axe.rule.<id>`), so one violation swapped for another of
// the same count fails: a rule absent on one side counts 0 there.
import type { Measurement } from '../e2e/measure.ts';
import { AXE_RULE_PREFIX } from '../e2e/metrics.ts';

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

export function aggregate(measurements: Pick<Measurement, 'scenario' | 'metrics' | 'axe_rules'>[]): Aggregate {
  const out: Aggregate = {};
  for (const m of measurements) {
    const row = (out[m.scenario] ??= {});
    for (const [k, v] of Object.entries(m.metrics)) row[k] = Math.max(row[k] ?? -Infinity, v);
    for (const [rule, v] of Object.entries(m.axe_rules ?? {})) {
      const k = `${AXE_RULE_PREFIX}${rule}`;
      row[k] = Math.max(row[k] ?? -Infinity, v);
    }
  }
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
      } else if (value > entry.value + entry.tolerance + EPSILON) {
        problems.push({ scenario, metric, kind: 'worse', baseline: entry.value, measured: value });
      } else if (value < entry.value - entry.tolerance - EPSILON) {
        problems.push({ scenario, metric, kind: 'improved-not-taken', baseline: entry.value, measured: value });
      }
    }
  }
  return problems;
}

// A baseline from measured values. With `previous`, an entry made worse than it carries the
// decision id, and an unchanged entry keeps the decision it had.
export function baselineFrom(measured: Aggregate, about: string, previous?: Baseline, decision?: string): Baseline {
  const scenarios: Baseline['scenarios'] = {};
  for (const scenario of Object.keys(measured).sort()) {
    scenarios[scenario] = {};
    for (const metric of Object.keys(measured[scenario]!).sort()) {
      const value = measured[scenario]![metric]!;
      const tolerance = toleranceOf(metric);
      const before = previous?.scenarios[scenario]?.[metric];
      const entry: BaselineEntry = { value, tolerance };
      if (before && value > before.value + before.tolerance + EPSILON && decision) entry.decision = decision;
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
