import { defineConfig, devices } from '@playwright/test';

const ROUND = !!process.env.UX_ROUND;

export default defineConfig({
  testDir: './e2e',
  testMatch: '**/*.e2e.ts',
  timeout: 60_000,
  globalTimeout: ROUND ? 300_000 : 120_000,
  fullyParallel: true,
  forbidOnly: true,
  retries: 0,
  reporter: [['list'], ['./e2e/slowest-reporter.ts']],
  use: {
    ...devices['Desktop Chrome'],
    trace: 'off',
    screenshot: 'off',
  },
  projects: [
    { name: 'measure-gate', testMatch: 'measure-gate.e2e.ts' },
    { name: 'gate', testMatch: 'gates.e2e.ts', dependencies: ['measure-gate'] },
    { name: 'measure-round', testMatch: 'measure-round.e2e.ts' },
    { name: 'round', testMatch: 'round-report.e2e.ts', dependencies: ['measure-round'] },
  ],
});
