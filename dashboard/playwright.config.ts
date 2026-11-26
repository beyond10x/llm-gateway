import { defineConfig, devices } from '@playwright/test';

const ROUND = !!process.env.UX_ROUND;

// One id per Playwright run, inherited by its workers: the measurements of this run go to
// ux/.metrics/<id>/ (e2e/metrics-dir.ts). `pnpm ux:round` sets its own.
process.env.UX_RUN_ID ||= `run-${new Date().toISOString().replace(/\D/g, '').slice(0, 14)}-${process.pid}`;

// The measuring projects; every other spec file belongs to the gate.
const MEASURING = ['measure-gate.e2e.ts', 'tools.e2e.ts', 'measure-round.e2e.ts', 'round-report.e2e.ts'];

export default defineConfig({
  testDir: './e2e',
  testMatch: '**/*.e2e.ts',
  timeout: 60_000,
  globalTimeout: ROUND ? 300_000 : 120_000,
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
    { name: 'measure-gate', testMatch: ['measure-gate.e2e.ts', 'tools.e2e.ts'] },
    { name: 'gate', testIgnore: MEASURING, dependencies: ['measure-gate'] },
    { name: 'measure-round', testMatch: 'measure-round.e2e.ts' },
    { name: 'round', testMatch: 'round-report.e2e.ts', dependencies: ['measure-round'] },
  ],
});
