// The round command: `pnpm ux:round R-nn [--force] [--baseline [--decision=<id>]] [--workers=N]`
// measures every scenario at every viewport in both themes and writes
// ux/rounds/R-nn/{report.md,round.json,shots/} and its row in ux/rounds/trend.md.
//
// A round directory that already holds a report, round.json or shots is refused; --force measures
// it again and replaces only those. --baseline also writes ux/baseline.json from the round's gating
// numbers, refused unless every gate replay was measured; a value made worse needs --decision.
//
// The method (the replay, the layout-shift window, the axe scans) is REPLAY_METHOD in
// e2e/harness.ts; every report prints it.
import { rmSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { ROUND_OUTPUTS, roundDirRefusal } from './round-guard.ts';

const args = process.argv.slice(2);
const id = args.find((a) => !a.startsWith('--'));
if (!id || !/^R-[0-9A-Za-z-]+$/.test(id)) {
  console.error('usage: pnpm ux:round R-nn [--force] [--baseline [--decision=<id>]] [--workers=N]');
  process.exit(2);
}
const force = args.includes('--force');
const baseline = args.includes('--baseline');
const decision = args.find((a) => a.startsWith('--decision='))?.slice('--decision='.length) ?? '';
const workers = args.find((a) => a.startsWith('--workers=')) ?? '--workers=2';
const cwd = fileURLToPath(new URL('..', import.meta.url));
const roundDir = join(cwd, 'ux', 'rounds', id);

const refusal = roundDirRefusal(roundDir, force);
if (refusal) {
  console.error(refusal);
  process.exit(2);
}
if (force) for (const f of ROUND_OUTPUTS) rmSync(join(roundDir, f), { recursive: true, force: true });

const runId = `${id}-${new Date().toISOString().replace(/\D/g, '').slice(0, 14)}`;
const result = spawnSync('pnpm', ['exec', 'playwright', 'test', '--project=round', workers], {
  cwd,
  stdio: 'inherit',
  env: { ...process.env, UX_ROUND: id, UX_RUN_ID: runId, UX_WRITE_BASELINE: baseline ? '1' : '0', UX_DECISION: decision },
});
process.exit(result.status ?? 1);
