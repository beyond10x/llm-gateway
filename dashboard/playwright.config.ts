import { defineConfig, devices } from '@playwright/test';

const IN_ROUND = !!process.env.UX_ROUND;

// One id per Playwright run, inherited by its workers: the measurements of this run go to
// ux/.metrics/<id>/ (e2e/metrics-dir.ts). `pnpm ux:round` sets its own.
process.env.UX_RUN_ID ||= `run-${new Date().toISOString().replace(/\D/g, '').slice(0, 14)}-${process.pid}`;

// The gate runs in two steps. `measure-gate` holds every spec that drives a browser: the measured
// replays and every assertion that replays a page itself (the tools' break tests and the change
// sets' regression tests), so they share the workers with the measurements instead of waiting for
// them. `gate` then reads what the measured replays wrote and asserts the hard gates and the
// ratchet on it. The round's projects run only through `pnpm ux:round`.
const READS_MEASUREMENTS = ['gates.e2e.ts'];
const ROUND = ['measure-round.e2e.ts', 'round-report.e2e.ts'];

export default defineConfig({
  testDir: './e2e',
  testMatch: '**/*.e2e.ts',
  timeout: 60_000,
  globalTimeout: IN_ROUND ? 300_000 : 120_000,
  globalSetup: './e2e/global-setup.ts',
  fullyParallel: true,
  forbidOnly: true,
  retries: 0,
  // On CI (`CI` set) the HTML report is written too; the Gate workflow uploads it when a test fails.
  reporter: [['list'], ['./e2e/slowest-reporter.ts'], ...(process.env.CI ? [['html', { open: 'never' }] as const] : [])],
  use: {
    ...devices['Desktop Chrome'],
    trace: 'off',
    screenshot: 'off',
  },
  projects: [
    { name: 'measure-gate', testIgnore: [...READS_MEASUREMENTS, ...ROUND] },
    { name: 'gate', testMatch: READS_MEASUREMENTS, dependencies: ['measure-gate'] },
    { name: 'measure-round', testMatch: 'measure-round.e2e.ts' },
    { name: 'round', testMatch: 'round-report.e2e.ts', dependencies: ['measure-round'] },
  ],
});
