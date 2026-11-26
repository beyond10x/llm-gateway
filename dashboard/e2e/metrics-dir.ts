import { existsSync, readdirSync, readFileSync, rmSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { Measurement } from './measure.ts';

// Where the measure projects write one JSON per scenario, viewport and theme for the projects that
// depend on them: ux/.metrics/<run id>/<set>/ (ignored by git). Not Playwright's output directory,
// which every run empties: a second run in the same tree would delete a round's measurements. The
// run id is set once per Playwright run in playwright.config.ts, and a round sets its own, so two
// runs never share a directory.
export const METRICS_ROOT = fileURLToPath(new URL('../ux/.metrics/', import.meta.url));

export function runId(): string {
  const id = process.env.UX_RUN_ID;
  if (!id) throw new Error('UX_RUN_ID is not set: run the measurements through playwright.config.ts or `pnpm ux:round`');
  return id;
}

export function metricsDir(set: 'gate' | 'round', run: string = runId(), root: string = METRICS_ROOT): string {
  return join(root, run, set);
}

// Every measurement of the directory; a file another run wrote is refused.
export function readMeasurements(dir: string, run: string = runId()): Measurement[] {
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((f) => f.endsWith('.json'))
    .sort()
    .map((f) => {
      const m = JSON.parse(readFileSync(join(dir, f), 'utf8')) as Measurement;
      if (m.run !== run) throw new Error(`${f} belongs to run ${m.run}, not to run ${run}`);
      return m;
    });
}

// Keeps the newest `keep` run directories besides the current one.
export function pruneRuns(keep: number, current: string, root: string = METRICS_ROOT): void {
  if (!existsSync(root)) return;
  const runs = readdirSync(root)
    .filter((d) => d !== current)
    .map((d) => ({ d, t: statSync(join(root, d)).mtimeMs }))
    .sort((a, b) => b.t - a.t);
  for (const { d } of runs.slice(keep)) rmSync(join(root, d), { recursive: true, force: true });
}
