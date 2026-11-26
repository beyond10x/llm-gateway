// The round command's refusals and the stamp of the tools a round was measured with. Pure, so
// ux/tools.test.ts can feed them bad input; treeStamp reads git.
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { PINNED_LABEL } from '../e2e/fonts.ts';
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

// --baseline is refused unless every gate replay was measured with the pinned fonts on a page and
// a harness that are committed (`stamp`, from treeStamp: no `+changes`, so the commit the baseline
// names reproduces it), and a value made worse (or no longer measured) needs a decision id.
export function baselineRefusal(
  missing: string[],
  problems: Problem[],
  decision: string | undefined,
  fonts: string = PINNED_LABEL,
  stamp: TreeStamp = { page: 'committed', harness: 'committed' },
): string | null {
  const dirty = Object.entries(stamp).filter(([, v]) => v.endsWith('+changes') || v === 'unknown');
  if (dirty.length > 0) return `--baseline refused: the ${dirty.map(([k, v]) => `${k} (${v})`).join(' and the ')} differ from a commit; commit them first, so the baseline names the commit it was taken on`;
  if (fonts !== PINNED_LABEL) return `--baseline refused: the round was measured with the fonts "${fonts}"; the baseline holds what the pinned fonts measure (unset UX_FONTS)`;
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

// The browser, axe-core and fonts every measurement of a round ran on; a round measured on two is
// refused.
export function toolsOf(ms: Pick<Measurement, 'tools'>[]): Measurement['tools'] {
  const seen = new Set(ms.map((m) => JSON.stringify(m.tools ?? null)));
  if (seen.size !== 1) throw new Error(`the round's measurements ran on different tools: ${[...seen].join(' / ')}`);
  const tools = ms[0]!.tools;
  if (!tools?.chromium || !tools.axe_core || !tools.fonts) throw new Error('a measurement names no Chromium or axe-core version, or no fonts');
  return tools;
}

// A trend row with its commit cut to 8 characters, the length every row is written with.
export function shortCommit(row: string): string {
  return row.replace(/^(\| [^|]+ \| [^|]+ \| )([0-9a-f]{8})[0-9a-f]+/, '$1$2');
}

// What the page is built from: a change under any of these marks the page `+changes`, whether the
// built page was rebuilt or not.
export const PAGE_PATHS = ['dashboard/src', 'dashboard/ui', 'dashboard/index.html', 'dashboard/vite.config.ts', 'crates/llm-gateway/src/dashboard.html'];
// What measures it. The round directories and the baseline are what a round writes.
export const HARNESS_PATHS = [
  'dashboard/e2e',
  'dashboard/ux',
  'dashboard/playwright.config.ts',
  'dashboard/package.json',
  'dashboard/pnpm-lock.yaml',
  ':(exclude)dashboard/ux/rounds',
  ':(exclude)dashboard/ux/baseline.json',
];

export interface TreeStamp {
  page: string;
  harness: string;
}

// The commit of the page and of the harness in the repository at `repo`, each marked `+changes`
// when the tree differs from it (an untracked file counts); `unknown` without git.
export function treeStamp(repo: string): TreeStamp {
  const git = (args: string[]) => execFileSync('git', args, { encoding: 'utf8', cwd: repo }).trim();
  try {
    const head = git(['rev-parse', '--short=8', 'HEAD']);
    const page = git(['status', '--porcelain', '--', ...PAGE_PATHS]);
    const harness = git(['status', '--porcelain', '--', ...HARNESS_PATHS]);
    return { page: page ? `${head}+changes` : head, harness: harness ? `${head}+changes` : head };
  } catch {
    return { page: 'unknown', harness: 'unknown' };
  }
}

// The baseline's `about`: what it holds, and the round and the commit it was taken on.
export function baselineAbout(gating: string[], round: string, commit: string, pageDigest: string): string {
  return `Worst value per scenario over the gating viewports (${gating.join(', ')}) and both themes, and the sideways scrollers at each measured-only viewport (overflow.scrollers.<viewport>, the worst over both themes). Taken by round ${round} at commit ${commit} on the built page with sha256 ${pageDigest.slice(0, 12)}. Lower is better, except flow.visible_ratio, flow.drawn and screen_only.reached, where higher is better. Written by \`pnpm ux:round R-nn --baseline\`.`;
}

// The round, the commit and the page digest a baseline's `about` names; null when it names none,
// or a commit with changes. A baseline re-taken by new harness tools on an unchanged page, in a
// round directory that is not kept, names the committed round whose page it measured.
export function aboutRound(about: string): { round: string; commit: string; page: string } | null {
  const m = about.match(/Taken by round (R-[0-9A-Za-z-]+) at commit ([0-9a-f]{8}) on the built page with sha256 ([0-9a-f]{12})\./);
  if (m) return { round: m[1]!, commit: m[2]!, page: m[3]! };
  const r = about.match(/Re-taken by the harness at commit ([0-9a-f]{8}) on the built page of round (R-[0-9A-Za-z-]+) with sha256 ([0-9a-f]{12})\./);
  return r ? { round: r[2]!, commit: r[1]!, page: r[3]! } : null;
}
