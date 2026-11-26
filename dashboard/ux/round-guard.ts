// The round command's refusals and the stamp of the tools a round was measured with. Pure, so
// ux/round-guard.test.ts can feed them bad input.
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import type { Measurement } from '../e2e/measure.ts';
import { METRIC_KEYS } from '../e2e/metrics.ts';
import type { Problem } from './ratchet.ts';

// What the round command writes into a round directory. Reviews, triage and decisions written
// there afterwards are never touched.
export const ROUND_OUTPUTS = ['report.md', 'round.json', 'shots'];

export function roundDirRefusal(dir: string, force: boolean): string | null {
  const present = ROUND_OUTPUTS.filter((f) => existsSync(join(dir, f)));
  if (present.length === 0 || force) return null;
  return `${dir} already holds ${present.join(', ')}: a measured round is not overwritten; pass --force to measure it again`;
}

export interface GateSet {
  scenarios: string[];
  viewports: string[];
  themes: string[];
}

// The gate replays (every scenario at every gating viewport in every theme) without a complete
// measurement.
export function missingGateReplays(ms: Pick<Measurement, 'scenario' | 'viewport' | 'theme' | 'metrics'>[], gate: GateSet): string[] {
  const missing: string[] = [];
  for (const s of gate.scenarios) {
    for (const v of gate.viewports) {
      for (const t of gate.themes) {
        const m = ms.find((x) => x.scenario === s && x.viewport.name === v && x.theme === t);
        if (!m || METRIC_KEYS.some((k) => !Number.isFinite(m.metrics[k]))) missing.push(`${s} ${v} ${t}`);
      }
    }
  }
  return missing;
}

// --baseline is refused unless every gate replay was measured, and a value made worse (or no longer
// measured) needs a decision id.
export function baselineRefusal(missing: string[], problems: Problem[], decision: string | undefined): string | null {
  if (missing.length > 0) return `--baseline refused: ${missing.length} gate replay(s) not measured: ${missing.slice(0, 5).join(', ')}`;
  const worse = problems.filter((p) => p.kind === 'worse' || p.kind === 'missing-measured');
  if (worse.length > 0 && !decision) {
    return `--baseline refused: ${worse.length} value(s) worse than ux/baseline.json or no longer measured (${worse
      .slice(0, 3)
      .map((p) => `${p.scenario} ${p.metric}`)
      .join(', ')}); name the decision with --decision=<id>`;
  }
  return null;
}

// The browser and axe-core every measurement of a round ran on; a round measured on two is refused.
export function toolsOf(ms: Pick<Measurement, 'tools'>[]): Measurement['tools'] {
  const seen = new Set(ms.map((m) => JSON.stringify(m.tools ?? null)));
  if (seen.size !== 1) throw new Error(`the round's measurements ran on different tools: ${[...seen].join(' / ')}`);
  const tools = ms[0]!.tools;
  if (!tools?.chromium || !tools.axe_core) throw new Error('a measurement names no Chromium or axe-core version');
  return tools;
}

// A trend row with its commit cut to 8 characters, the length every row is written with.
export function shortCommit(row: string): string {
  return row.replace(/^(\| [^|]+ \| [^|]+ \| )([0-9a-f]{8})[0-9a-f]+/, '$1$2');
}
