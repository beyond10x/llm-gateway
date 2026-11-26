// The round command: `pnpm ux:round R-nn [--baseline]` measures every scenario at every viewport in
// both themes and writes ux/rounds/R-nn/{report.md,round.json,shots/} and its row in
// ux/rounds/trend.md. --baseline also writes ux/baseline.json from the round's gating numbers.
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const args = process.argv.slice(2);
const id = args.find((a) => !a.startsWith('--'));
if (!id || !/^R-\d{2,}$/.test(id)) {
  console.error('usage: pnpm ux:round R-nn [--baseline] [--workers=N]');
  process.exit(2);
}
const workers = args.find((a) => a.startsWith('--workers=')) ?? '--workers=2';
const cwd = fileURLToPath(new URL('..', import.meta.url));
const result = spawnSync('pnpm', ['exec', 'playwright', 'test', '--project=round', workers], {
  cwd,
  stdio: 'inherit',
  env: { ...process.env, UX_ROUND: id, UX_WRITE_BASELINE: args.includes('--baseline') ? '1' : '0' },
});
process.exit(result.status ?? 1);
