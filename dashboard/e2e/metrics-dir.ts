import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { TestInfo } from '@playwright/test';
import type { Measurement } from './measure.ts';

// Where the measure projects write one JSON per scenario, viewport and theme for the projects that
// depend on them. The output directory is emptied at the start of every run.
export function metricsDir(info: TestInfo, set: 'gate' | 'round'): string {
  return join(info.project.outputDir, 'ux-metrics', set);
}

export function readMeasurements(dir: string): Measurement[] {
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((f) => f.endsWith('.json'))
    .sort()
    .map((f) => JSON.parse(readFileSync(join(dir, f), 'utf8')) as Measurement);
}
