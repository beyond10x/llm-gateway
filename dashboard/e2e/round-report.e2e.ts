// Writes a round directory, ux/rounds/<id>/{report.md,round.json,shots/}, and its row in
// ux/rounds/trend.md, from what measure-round wrote. With UX_WRITE_BASELINE=1 it also writes
// ux/baseline.json from the round's gating numbers.
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from '@playwright/test';
import { SCENARIOS } from '../ux/scenarios/index.ts';
import { aggregate, baselineFrom, compare, describeProblem, type Baseline } from '../ux/ratchet.ts';
import { METRIC_KEYS, type Measurement } from './measure.ts';
import { metricsDir, readMeasurements } from './metrics-dir.ts';
import { GATING, KEY_FONT_PX, THEMES, VIEWPORTS, type Theme } from './profiles.ts';

const UX = fileURLToPath(new URL('../ux/', import.meta.url));
const BUILT = fileURLToPath(new URL('../../crates/llm-gateway/src/dashboard.html', import.meta.url));

test.skip(!process.env.UX_ROUND, 'a round runs through `pnpm ux:round <id>`');

interface GateResult {
  gate: string;
  theme: Theme;
  passed: number;
  of: number;
  headline: string;
}

function hardGates(ms: Measurement[]): GateResult[] {
  const out: GateResult[] = [];
  for (const theme of THEMES) {
    const mine = ms.filter((m) => m.theme === theme && m.viewport.gating);
    const g = (gate: string, ok: (m: Measurement) => boolean, headline: string) =>
      out.push({ gate, theme, passed: mine.filter(ok).length, of: mine.length, headline });
    const sum = (k: keyof Measurement['metrics']) => mine.reduce((a, m) => a + m.metrics[k], 0);
    const max = (k: keyof Measurement['metrics']) => Math.max(0, ...mine.map((m) => m.metrics[k]));
    g(
      'gate 1 layout',
      (m) => m.metrics['overflow.x_px'] === 0 && m.metrics['overflow.elements'] === 0 && m.metrics['overflow.scrollers_unreachable'] === 0,
      `overflow ${max('overflow.x_px')} px max, ${max('overflow.elements')} elements past their box max, ${max('overflow.scrollers_unreachable')} scrollers without keyboard access max`,
    );
    g('gate 1 truncation', (m) => m.metrics['truncation.without_full_value'] === 0, `${max('truncation.without_full_value')} truncated without full value max`);
    g('gate 2 always visible', (m) => m.header_min_visible_ratio === 1, `header visible ratio ${Math.min(1, ...mine.map((m) => m.header_min_visible_ratio))} min; toggle ${mine.some((m) => m.toggle === 'present') ? 'present' : 'absent'}`);
    g('gate 5 axe', (m) => m.metrics['axe.serious_critical'] === 0, `${sum('axe.serious_critical')} serious/critical nodes over ${mine.length} replays (${[...new Set(mine.flatMap((m) => m.details.axe.map((a) => a.id)))].join(', ') || 'none'})`);
    g('gate 5 keyboard', (m) => m.metrics['keyboard.unreachable'] === 0 && m.metrics['keyboard.focus_invisible'] === 0, `${max('keyboard.unreachable')} unreachable, ${max('keyboard.focus_invisible')} focus-invisible max`);
    g('gate 5 reduced motion', (m) => m.metrics['reduced_motion.violations'] === 0, `${max('reduced_motion.violations')} violations max`);
  }
  return out;
}

function commit(): string {
  try {
    const head = execFileSync('git', ['rev-parse', '--short=8', 'HEAD'], { encoding: 'utf8' }).trim();
    const dirty = execFileSync('git', ['status', '--porcelain', '--', 'dashboard/src', 'crates/llm-gateway/src/dashboard.html'], { encoding: 'utf8', cwd: join(UX, '..', '..') }).trim();
    return dirty ? `${head}+changes` : head;
  } catch {
    return 'unknown';
  }
}

const fmt = (v: number) => (Number.isInteger(v) ? String(v) : v.toFixed(3));

test('write the round directory', async ({ page }, info) => {
  const id = process.env.UX_ROUND!;
  const dir = metricsDir(info, 'round');
  const all = readMeasurements(dir);
  const expected = SCENARIOS.length * VIEWPORTS.length * THEMES.length;
  if (all.length !== expected) throw new Error(`round measured ${all.length} of ${expected} replays`);
  const roundDir = join(UX, 'rounds', id);
  mkdirSync(join(roundDir, 'shots'), { recursive: true });

  const gatingAgg = aggregate(all.filter((m) => m.viewport.gating));
  const allAgg = aggregate(all);
  const gates = hardGates(all);
  const pageDigest = createHash('sha256').update(readFileSync(BUILT)).digest('hex').slice(0, 12);
  const date = new Date().toISOString().slice(0, 10);
  const baselineFile = join(UX, 'baseline.json');
  const previous = existsSync(baselineFile) ? (JSON.parse(readFileSync(baselineFile, 'utf8')) as Baseline) : null;
  const ratchet = previous ? compare(previous, gatingAgg).map(describeProblem) : ['no baseline yet'];

  // Contact sheets: per theme and gating viewport, the last state of every scenario at half size.
  const sheets: string[] = [];
  for (const viewport of GATING) {
    for (const theme of THEMES) {
      const tiles = SCENARIOS.map((s) => {
        const png = readFileSync(join(dir, 'shots', `${s.name}__${viewport.name}__${theme}.png`)).toString('base64');
        return `<figure><img src="data:image/png;base64,${png}"><figcaption>${s.name}</figcaption></figure>`;
      }).join('');
      const w = viewport.width / 2;
      const h = viewport.height / 2;
      await page.setViewportSize({ width: w * 2 + 12, height: (h + 22) * 2 + 12 });
      await page.setContent(
        `<style>body{margin:0;padding:4px;display:grid;grid-template-columns:repeat(2,${w}px);gap:4px;background:#888;font:12px system-ui}figure{margin:0}img{width:${w}px;height:${h}px;display:block}figcaption{height:18px;color:#fff}</style>${tiles}`,
      );
      const name = `contact-${viewport.name}-${theme}.png`;
      await page.screenshot({ path: join(roundDir, 'shots', name), fullPage: true });
      sheets.push(name);
    }
  }

  const round = {
    round: id,
    date,
    commit: commit(),
    page_sha256_12: pageDigest,
    toggle: all.some((m) => m.toggle === 'present') ? 'present' : 'absent',
    viewports: VIEWPORTS,
    themes: THEMES,
    scenarios: SCENARIOS.map((s) => ({ name: s.name, anchor: s.anchor, description: s.description })),
    hard_gates: gates,
    gating: gatingAgg,
    all_viewports: allAgg,
    ratchet_against_baseline: ratchet,
    replays: all.map((m) => ({
      scenario: m.scenario,
      viewport: m.viewport.name,
      gating: m.viewport.gating,
      theme: m.theme,
      toggle: m.toggle,
      theme_applied: m.theme_applied,
      metrics: m.metrics,
      key_font_min_px: m.key_font_min_px,
      key_font_at: m.key_font_at,
      shifts: m.shifts,
      details: m.details,
    })),
  };
  writeFileSync(join(roundDir, 'round.json'), `${JSON.stringify(round, null, 2)}\n`);

  const md: string[] = [];
  md.push(`# Round ${id}`, '');
  md.push(`Date ${date}. Page commit \`${round.commit}\`, built page sha256 \`${pageDigest}…\`. Theme toggle: ${round.toggle}.`);
  md.push(`Scenarios: ${SCENARIOS.map((s) => `\`${s.name}\`${s.anchor ? ' (anchor)' : ''}`).join(', ')}. Gating viewports ${GATING.map((v) => v.name).join(', ')}; measured-only ${VIEWPORTS.filter((v) => !v.gating).map((v) => v.name).join(', ')}.`);
  md.push('Hermetic: every snapshot comes from `ux/scenarios/`; the page is the built `dashboard.html` on a fake origin; every other request is aborted.');
  md.push('Each state arrives as one complete `dashboard/events` response; when it ends the page falls back to polling until the next connection, so the connection status reads "polling every 5 s" in every shot.', '');
  md.push('## Hard gates (gating viewports)', '', '| gate | theme | replays passing | headline |', '|---|---|---|---|');
  for (const g of gates) md.push(`| ${g.gate} | ${g.theme} | ${g.passed} of ${g.of} | ${g.headline} |`);
  md.push('', '## Metrics per scenario, worst over the gating viewports and themes (lower is better)', '');
  md.push(`| scenario | ${METRIC_KEYS.join(' | ')} |`, `|---|${METRIC_KEYS.map(() => '---').join('|')}|`);
  for (const s of SCENARIOS) md.push(`| ${s.name} | ${METRIC_KEYS.map((k) => fmt(gatingAgg[s.name]?.[k] ?? NaN)).join(' | ')} |`);
  md.push('', `\`key_font.deficit_px\` is ${KEY_FONT_PX} px less the smallest key-data font; \`header.hidden_ratio\` is 1 less the smallest visible share of a header element.`);
  md.push('', '## Every replay', '', '| scenario | viewport | theme | axe | overflow px | past box | header visible | unreachable | focus invisible | key font px | layout shift max | reduced motion | truncated |', '|---|---|---|---|---|---|---|---|---|---|---|---|---|');
  for (const m of all) {
    const x = m.metrics;
    md.push(`| ${m.scenario} | ${m.viewport.name}${m.viewport.gating ? '' : ' (measured)'} | ${m.theme} | ${x['axe.serious_critical']} | ${x['overflow.x_px']} | ${x['overflow.elements']} | ${m.header_min_visible_ratio} | ${x['keyboard.unreachable'] + x['overflow.scrollers_unreachable']} | ${x['keyboard.focus_invisible']} | ${m.key_font_min_px} | ${fmt(x['layout_shift.per_state_max'])} | ${x['reduced_motion.violations']} | ${x['truncation.without_full_value']} |`);
  }
  md.push('', '## Worst moments', '');
  for (const k of METRIC_KEYS) {
    const worst = [...all].sort((a, b) => b.metrics[k] - a.metrics[k])[0]!;
    if (worst.metrics[k] <= 0) continue;
    const d = worst.details;
    const sample =
      k === 'axe.serious_critical' ? d.axe.map((a) => `${a.id}: ${a.targets.join('; ')}`)
      : k === 'overflow.elements' ? d.overflowing
      : k === 'overflow.scrollers_unreachable' ? d.scrollers_unreachable
      : k === 'keyboard.unreachable' ? d.unreachable
      : k === 'keyboard.focus_invisible' ? d.focus_invisible
      : k === 'reduced_motion.violations' ? d.reduced_motion
      : k === 'truncation.without_full_value' ? d.truncated
      : k === 'key_font.deficit_px' ? [`${worst.key_font_min_px} px at ${worst.key_font_at}`]
      : k === 'layout_shift.per_state_max' ? [`per state: ${worst.shifts.join(', ')}`]
      : [];
    md.push(`- \`${k}\` = ${fmt(worst.metrics[k])} at ${worst.scenario} ${worst.viewport.name} ${worst.theme}: ${sample.slice(0, 3).join(' / ') || '–'}`);
  }
  md.push('', '## Ratchet against `ux/baseline.json`', '');
  md.push(...(ratchet.length ? ratchet.map((r) => `- ${r}`) : ['- every value equals the baseline within its tolerance']));
  md.push('', '## Contact sheets', '', 'The last state of each scenario, 0.75 s after it arrived, at half size.', '');
  for (const s of sheets) md.push(`![${s}](shots/${s})`);
  md.push('');
  writeFileSync(join(roundDir, 'report.md'), md.join('\n'));

  const trendFile = join(UX, 'rounds', 'trend.md');
  const header = ['# Trend', '', 'One row per round: hard-gate replays passing at the gating viewports (light / dark), and the worst values over every scenario.', '', '| round | date | commit | toggle | gate 1 | gate 2 | gate 5 axe | gate 5 keyboard | gate 5 motion | axe nodes | past box | key font px | layout shift |', '|---|---|---|---|---|---|---|---|---|---|---|---|---|'];
  const rows = existsSync(trendFile) ? readFileSync(trendFile, 'utf8').split('\n').filter((l) => /^\| R-/.test(l) && !l.startsWith(`| ${id} |`)) : [];
  const pass = (gate: string) => THEMES.map((t) => { const g = gates.find((x) => x.gate === gate && x.theme === t)!; return `${g.passed}/${g.of}`; }).join(' / ');
  const worst = (k: keyof Measurement['metrics']) => fmt(Math.max(...Object.values(gatingAgg).map((r) => r[k] ?? 0)));
  rows.push(`| ${id} | ${date} | ${round.commit} | ${round.toggle} | ${pass('gate 1 layout')} | ${pass('gate 2 always visible')} | ${pass('gate 5 axe')} | ${pass('gate 5 keyboard')} | ${pass('gate 5 reduced motion')} | ${worst('axe.serious_critical')} | ${worst('overflow.elements')} | ${fmt(Math.min(...all.filter((m) => m.viewport.gating).map((m) => m.key_font_min_px)))} | ${worst('layout_shift.per_state_max')} |`);
  rows.sort();
  writeFileSync(trendFile, `${[...header, ...rows].join('\n')}\n`);

  if (process.env.UX_WRITE_BASELINE === '1') {
    const baseline = baselineFrom(gatingAgg, `Worst value per scenario over the gating viewports (${GATING.map((v) => v.name).join(', ')}) and both themes, from round ${id}. Lower is better. Written by \`pnpm ux:round ${id} --baseline\`.`);
    writeFileSync(baselineFile, `${JSON.stringify(baseline, null, 2)}\n`);
  }
});
