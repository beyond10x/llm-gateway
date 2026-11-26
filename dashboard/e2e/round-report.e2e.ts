// Writes a round directory, ux/rounds/<id>/{report.md,round.json,shots/}, and its row in
// ux/rounds/trend.md, from what measure-round wrote. With UX_WRITE_BASELINE=1 it also writes
// ux/baseline.json from the round's gating numbers, refused unless every gate replay was measured
// and, for a value made worse, unless UX_DECISION names the decision.
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from '@playwright/test';
import { SCENARIOS } from '../ux/scenarios/index.ts';
import { aggregate, baselineFrom, compare, describeProblem, type Aggregate, type Baseline } from '../ux/ratchet.ts';
import { baselineRefusal, missingGateReplays, roundDirRefusal, shortCommit, toolsOf } from '../ux/round-guard.ts';
import { REPLAY_METHOD } from './harness.ts';
import type { Measurement } from './measure.ts';
import { activeSessions } from '../src/flow.ts';
import { AXE_RULE_PREFIX, flowProblems, fmtKeyFont, FLOW_VISIBLE_MIN, METRIC_KEYS } from './metrics.ts';
import { metricsDir, readMeasurements } from './metrics-dir.ts';
import { GATING, KEY_FONT_PX, SECONDARY_FONT_PX, THEMES, VIEWPORTS, type Theme } from './profiles.ts';

// The scenarios whose last state shows an active session: their token-flow canvas is gated.
const WITH_SESSIONS = new Set(SCENARIOS.filter((s) => { const st = s.states(Date.now()); return activeSessions(st[st.length - 1]!).size > 0; }).map((s) => s.name));

const DASHBOARD = fileURLToPath(new URL('../', import.meta.url));
const UX = join(DASHBOARD, 'ux');
const REPO = join(DASHBOARD, '..');
const BUILT = join(REPO, 'crates', 'llm-gateway', 'src', 'dashboard.html');

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
    const every = ms.filter((m) => m.theme === theme);
    const g = (gate: string, of: Measurement[], ok: (m: Measurement) => boolean, headline: string) =>
      out.push({ gate, theme, passed: of.filter(ok).length, of: of.length, headline });
    const sum = (k: keyof Measurement['metrics']) => mine.reduce((a, m) => a + m.metrics[k], 0);
    const max = (k: keyof Measurement['metrics']) => Math.max(0, ...mine.map((m) => m.metrics[k]));
    g(
      'gate 1 layout',
      mine,
      (m) => m.metrics['overflow.x_px'] === 0 && m.metrics['overflow.elements'] === 0 && m.metrics['overflow.scrollers_unreachable'] === 0,
      `overflow ${max('overflow.x_px')} px max, ${max('overflow.elements')} elements past their box max, ${max('overflow.scrollers_unreachable')} scrollers Tab never reaches max (Chromium focuses a scrolling box itself; axe judges the markup)`,
    );
    g('gate 1 truncation', mine, (m) => m.metrics['truncation.without_full_value'] === 0, `${max('truncation.without_full_value')} truncated without full value max`);
    g('gate 2 always visible', mine, (m) => m.header_min_visible_ratio === 1, `header visible ratio ${Math.min(1, ...mine.map((m) => m.header_min_visible_ratio))} min; toggle ${mine.some((m) => m.toggle === 'present') ? 'present' : 'absent'}`);
    g('gate 2 always visible, every viewport', every, (m) => m.header_min_visible_ratio === 1, `hidden: ${[...new Set(every.flatMap((m) => m.details.header.filter((h) => h.visible_ratio < 1).map((h) => `${h.name} ${h.visible_ratio} at ${m.scenario} ${m.viewport.name}`)))].slice(0, 3).join('; ') || 'none'}`);
    g('gate 5 axe', mine, (m) => m.metrics['axe.serious_critical'] === 0, `${sum('axe.serious_critical')} serious/critical nodes over ${mine.length} replays (${[...new Set(mine.flatMap((m) => m.details.axe.map((a) => a.id)))].join(', ') || 'none'})`);
    g('gate 5 keyboard', mine, (m) => m.metrics['keyboard.unreachable'] === 0 && m.metrics['keyboard.focus_invisible'] === 0, `${max('keyboard.unreachable')} unreachable, ${max('keyboard.focus_invisible')} focus-invisible max`);
    g('gate 5 reduced motion', mine, (m) => m.metrics['reduced_motion.violations'] === 0, `${max('reduced_motion.violations')} violations max`);
    g('key data', mine, (m) => m.details.key_font_below.length === 0 && m.details.secondary_font_below.length === 0, `key ${max('key_font.deficit_px')} px short max (floor ${KEY_FONT_PX}), secondary ${max('secondary_font.deficit_px')} px short max (floor ${SECONDARY_FONT_PX})`);
    const flow = mine.filter((m) => WITH_SESSIONS.has(m.scenario));
    g(`flow (${[...WITH_SESSIONS].join(', ')})`, flow, (m) => flowProblems(m).length === 0, `visible ratio ${Math.min(1, ...flow.map((m) => m.flow.visible_ratio))} min (bar ${FLOW_VISIBLE_MIN}), drawn ${flow.filter((m) => m.flow.drawn).length} of ${flow.length}, drawn under reduced motion ${flow.filter((m) => m.reduced_flow.drawn).length} of ${flow.length}`);
    g('replay hermetic and live', mine, (m) => m.blocked_requests === 0 && m.snapshot_fetches === 1 && m.status.every((s) => s === 'live'), `${mine.reduce((a, m) => a + m.blocked_requests, 0)} requests aborted`);
  }
  return out;
}

function git(args: string[]): string {
  return execFileSync('git', args, { encoding: 'utf8', cwd: REPO }).trim();
}

// The commit of the page and of the harness, each marked `+changes` when the tree differs from it.
function commits(): { page: string; harness: string } {
  try {
    const head = git(['rev-parse', '--short=8', 'HEAD']);
    const page = git(['status', '--porcelain', '--', 'dashboard/src', 'crates/llm-gateway/src/dashboard.html']);
    const harness = git(['status', '--porcelain', '--', 'dashboard/e2e', 'dashboard/ux', 'dashboard/playwright.config.ts', 'dashboard/package.json', 'dashboard/pnpm-lock.yaml', ':(exclude)dashboard/ux/rounds']);
    return { page: page ? `${head}+changes` : head, harness: harness ? `${head}+changes` : head };
  } catch {
    return { page: 'unknown', harness: 'unknown' };
  }
}

function packageVersion(name: string): string {
  const file = join(DASHBOARD, 'node_modules', name, 'package.json');
  return existsSync(file) ? (JSON.parse(readFileSync(file, 'utf8')) as { version: string }).version : 'unknown';
}

const fmt = (v: number | undefined) => (v === undefined || !Number.isFinite(v) ? '–' : Number.isInteger(v) ? String(v) : v.toFixed(3));

test('write the round directory', async ({ page }) => {
  const id = process.env.UX_ROUND!;
  const dir = metricsDir('round');
  const all = readMeasurements(dir);
  const expected = SCENARIOS.length * VIEWPORTS.length * THEMES.length;
  if (all.length !== expected) throw new Error(`round measured ${all.length} of ${expected} replays`);
  const roundDir = join(UX, 'rounds', id);
  const refusal = roundDirRefusal(roundDir, false);
  if (refusal) throw new Error(refusal);
  mkdirSync(join(roundDir, 'shots'), { recursive: true });

  const gatingAgg = aggregate(all.filter((m) => m.viewport.gating));
  const allAgg = aggregate(all);
  const gates = hardGates(all);
  const pageDigest = createHash('sha256').update(readFileSync(BUILT)).digest('hex');
  const date = new Date().toISOString().slice(0, 10);
  const baselineFile = join(UX, 'baseline.json');
  const previous = existsSync(baselineFile) ? (JSON.parse(readFileSync(baselineFile, 'utf8')) as Baseline) : null;
  const problems = previous ? compare(previous, gatingAgg) : [];
  const ratchet = previous ? problems.map(describeProblem) : ['no baseline yet'];
  const stamp = commits();
  const tools = {
    chromium: toolsOf(all).chromium,
    axe_core: toolsOf(all).axe_core,
    fonts: toolsOf(all).fonts,
    axe_core_playwright: packageVersion('@axe-core/playwright'),
    playwright: packageVersion('@playwright/test'),
    harness_commit: stamp.harness,
  };

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
    commit: stamp.page,
    page_sha256: pageDigest,
    tools,
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
      axe_rules: m.axe_rules,
      axe_scans: m.axe_scans,
      flow: m.flow,
      reduced_flow: m.reduced_flow,
      key_font_min_px: m.key_font_min_px,
      key_font_at: m.key_font_at,
      secondary_font_min_px: m.secondary_font_min_px,
      secondary_font_at: m.secondary_font_at,
      shifts: m.shifts,
      status: m.status,
      snapshot_fetches: m.snapshot_fetches,
      blocked: m.blocked,
      details: m.details,
    })),
  };
  writeFileSync(join(roundDir, 'round.json'), `${JSON.stringify(round, null, 2)}\n`);

  const rows = (agg: Aggregate) => new Set(Object.values(agg).flatMap((r) => Object.keys(r)));
  const ruleKeys = [...new Set([...rows(gatingAgg), ...rows(allAgg)])].filter((k) => k.startsWith(AXE_RULE_PREFIX)).sort();
  const minOf = (ms: Measurement[], f: (m: Measurement) => number | null) => {
    const v = ms.map(f).filter((x): x is number => x !== null);
    return v.length ? Math.min(...v) : null;
  };

  const md: string[] = [];
  md.push(`# Round ${id}`, '');
  md.push(`Date ${date}. Page commit \`${round.commit}\`, built page sha256 \`${pageDigest.slice(0, 12)}…\`. Theme toggle: ${round.toggle}.`);
  md.push(`Tools: Chromium ${tools.chromium}, axe-core ${tools.axe_core} (@axe-core/playwright ${tools.axe_core_playwright}), Playwright ${tools.playwright}, harness commit \`${tools.harness_commit}\`. Fonts: ${tools.fonts}.`);
  md.push(`Scenarios: ${SCENARIOS.map((s) => `\`${s.name}\`${s.anchor ? ' (anchor)' : ''}`).join(', ')}. Gating viewports ${GATING.map((v) => v.name).join(', ')}; measured-only ${VIEWPORTS.filter((v) => !v.gating).map((v) => v.name).join(', ')}.`, '');
  md.push('## Method', '', ...REPLAY_METHOD.split('\n').map((l) => `- ${l}`), '');
  md.push('## Hard gates (gating viewports)', '', '| gate | theme | replays passing | headline |', '|---|---|---|---|');
  for (const g of gates) md.push(`| ${g.gate} | ${g.theme} | ${g.passed} of ${g.of} | ${g.headline} |`);

  md.push('', '## Metrics per scenario, worst value (lower is better)', '');
  md.push('"gating" is the worst over the gating viewports and both themes, the basis of the ratchet; "all" is the worst over every viewport.', '');
  md.push(`| metric | ${SCENARIOS.map((s) => `${s.name} gating | ${s.name} all`).join(' | ')} |`, `|---|${SCENARIOS.map(() => '---|---').join('|')}|`);
  for (const k of [...METRIC_KEYS, ...ruleKeys]) {
    md.push(`| ${k} | ${SCENARIOS.map((s) => `${fmt(gatingAgg[s.name]?.[k] ?? (k.startsWith(AXE_RULE_PREFIX) ? 0 : undefined))} | ${fmt(allAgg[s.name]?.[k] ?? (k.startsWith(AXE_RULE_PREFIX) ? 0 : undefined))}`).join(' | ')} |`);
  }
  const forScenario = (s: string, gatingOnly: boolean) => all.filter((m) => m.scenario === s && (!gatingOnly || m.viewport.gating));
  const keyFont = (ms: Measurement[]) => fmtKeyFont(minOf(ms, (m) => m.key_font_min_px));
  md.push(`| key font px, smallest (n/a: no key data) | ${SCENARIOS.map((s) => `${keyFont(forScenario(s.name, true))} | ${keyFont(forScenario(s.name, false))}`).join(' | ')} |`);
  md.push(`| flow.visible_ratio, smallest (higher is better) | ${SCENARIOS.map((s) => `${fmt(minOf(forScenario(s.name, true), (m) => m.flow.visible_ratio) ?? undefined)} | ${fmt(minOf(forScenario(s.name, false), (m) => m.flow.visible_ratio) ?? undefined)}`).join(' | ')} |`);
  md.push(`| flow.drawn, replays drawn in both the replay and its reduced-motion replay | ${SCENARIOS.map((s) => { const g = forScenario(s.name, true); const a = forScenario(s.name, false); return `${g.filter((m) => m.flow.drawn && m.reduced_flow.drawn).length}/${g.length} | ${a.filter((m) => m.flow.drawn && m.reduced_flow.drawn).length}/${a.length}`; }).join(' | ')} |`);
  md.push('', `\`key_font.deficit_px\` is ${KEY_FONT_PX} px less the smallest key-data font, 0 where a page shows no key data, and \`secondary_font.deficit_px\` the same for secondary text against ${SECONDARY_FONT_PX} px (one definition of both: \`keyDataProbe\` in e2e/probe.ts); \`overflow.scrollers\` counts the elements that scroll sideways; \`header.hidden_ratio\` is 1 less the smallest visible share of a header element; \`axe.rule.<id>\` counts the distinct nodes of one rule over every scan.`);

  md.push('', '## Every replay', '', '| scenario | viewport | theme | axe | overflow px | past box | scrollers | header visible | unreachable | focus invisible | key font px | layout shift max | reduced motion | truncated | flow visible | flow drawn (reduced motion) | status |', '|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|');
  for (const m of all) {
    const x = m.metrics;
    const status = [...new Set(m.status)].join(', ');
    md.push(`| ${m.scenario} | ${m.viewport.name}${m.viewport.gating ? '' : ' (measured)'} | ${m.theme} | ${x['axe.serious_critical']} | ${x['overflow.x_px']} | ${x['overflow.elements']} | ${x['overflow.scrollers']} | ${m.header_min_visible_ratio} | ${x['keyboard.unreachable'] + x['overflow.scrollers_unreachable']} | ${x['keyboard.focus_invisible']} | ${fmtKeyFont(m.key_font_min_px)} | ${fmt(x['layout_shift.per_state_max'])} | ${x['reduced_motion.violations']} | ${x['truncation.without_full_value']} | ${fmt(m.flow.visible_ratio)} | ${m.flow.drawn ? 'yes' : 'no'} (${m.reduced_flow.drawn ? 'yes' : 'no'}) | ${status} |`);
  }

  const worstMoments = (ms: Measurement[]) => {
    const out: string[] = [];
    for (const k of METRIC_KEYS) {
      const worst = [...ms].sort((a, b) => b.metrics[k] - a.metrics[k])[0];
      if (!worst || worst.metrics[k] <= 0) continue;
      const d = worst.details;
      const sample =
        k === 'axe.serious_critical' ? d.axe.map((a) => `${a.id}: ${a.targets.join('; ')}`)
        : k === 'overflow.elements' ? d.overflowing.map((o) => o.label)
        : k === 'overflow.scrollers' ? d.scrollers.map((x) => x.label)
        : k === 'overflow.scrollers_unreachable' ? d.scrollers_unreachable
        : k === 'header.hidden_ratio' ? d.header.filter((h) => h.visible_ratio < 1).map((h) => `${h.name} visible ${h.visible_ratio}`)
        : k === 'keyboard.unreachable' ? d.unreachable
        : k === 'keyboard.focus_invisible' ? d.focus_invisible
        : k === 'reduced_motion.violations' ? d.reduced_motion
        : k === 'truncation.without_full_value' ? d.truncated.map((t) => t.label)
        : k === 'key_font.deficit_px' ? [`${fmtKeyFont(worst.key_font_min_px)} px at ${worst.key_font_at}`]
        : k === 'secondary_font.deficit_px' ? [`${fmtKeyFont(worst.secondary_font_min_px)} px at ${worst.secondary_font_at}`]
        : k === 'layout_shift.per_state_max' ? [`per state: ${worst.shifts.join(', ')}`]
        : [];
      out.push(`- \`${k}\` = ${fmt(worst.metrics[k])} at ${worst.scenario} ${worst.viewport.name} ${worst.theme}: ${sample.slice(0, 3).join(' / ') || '–'}`);
    }
    return out.length ? out : ['- none above 0'];
  };
  md.push('', '## Worst moments at the gating viewports', '', ...worstMoments(all.filter((m) => m.viewport.gating)));
  md.push('', '## Worst moments at the measured-only viewports (no screenshot)', '', ...worstMoments(all.filter((m) => !m.viewport.gating)));
  md.push('', '## Ratchet against `ux/baseline.json` (gating values)', '');
  md.push(...(ratchet.length ? ratchet.map((r) => `- ${r}`) : ['- every value equals the baseline within its tolerance']));
  md.push('', '## Contact sheets', '', 'The last state of each scenario, 0.75 s after it arrived, at half size.', '');
  for (const s of sheets) md.push(`![${s}](shots/${s})`);
  md.push('');
  writeFileSync(join(roundDir, 'report.md'), md.join('\n'));

  const trendFile = join(UX, 'rounds', 'trend.md');
  const header = ['# Trend', '', 'One row per round: hard-gate replays passing at the gating viewports (light / dark), and the worst values over every scenario at the gating viewports only (the ratchet\'s basis). "key font px" is the smallest key-data font there, n/a when no replay shows key data.', '', '| round | date | commit | toggle | gate 1 | gate 2 | gate 5 axe | gate 5 keyboard | gate 5 motion | axe nodes | past box | key font px | layout shift |', '|---|---|---|---|---|---|---|---|---|---|---|---|---|'];
  const trendRows = existsSync(trendFile) ? readFileSync(trendFile, 'utf8').split('\n').filter((l) => /^\| R-/.test(l) && !l.startsWith(`| ${id} |`)).map(shortCommit) : [];
  const pass = (gate: string) => THEMES.map((t) => { const g = gates.find((x) => x.gate === gate && x.theme === t)!; return `${g.passed}/${g.of}`; }).join(' / ');
  const worst = (k: keyof Measurement['metrics']) => fmt(Math.max(...Object.values(gatingAgg).map((r) => r[k] ?? 0)));
  trendRows.push(`| ${id} | ${date} | ${round.commit} | ${round.toggle} | ${pass('gate 1 layout')} | ${pass('gate 2 always visible')} | ${pass('gate 5 axe')} | ${pass('gate 5 keyboard')} | ${pass('gate 5 reduced motion')} | ${worst('axe.serious_critical')} | ${worst('overflow.elements')} | ${keyFont(all.filter((m) => m.viewport.gating))} | ${worst('layout_shift.per_state_max')} |`);
  trendRows.sort();
  writeFileSync(trendFile, `${[...header, ...trendRows].join('\n')}\n`);

  if (process.env.UX_WRITE_BASELINE === '1') {
    const decision = process.env.UX_DECISION || undefined;
    const missing = missingGateReplays(all, { scenarios: SCENARIOS.map((s) => s.name), viewports: GATING.map((v) => v.name), themes: THEMES });
    const refused = baselineRefusal(missing, problems, decision, tools.fonts);
    if (refused) throw new Error(refused);
    // The round directory may be a scratch one that is not kept: the text names the page it measured
    // (the sha256 a committed round.json carries as page_sha256), not the round.
    const baseline = baselineFrom(gatingAgg, `Worst value per scenario over the gating viewports (${GATING.map((v) => v.name).join(', ')}) and both themes, measured on the built page with sha256 ${pageDigest.slice(0, 12)} (page commit ${stamp.page}). Lower is better, except flow.visible_ratio and flow.drawn, where higher is better. Written by \`pnpm ux:round R-nn --baseline\`.`, previous ?? undefined, decision);
    writeFileSync(baselineFile, `${JSON.stringify(baseline, null, 2)}\n`);
  }
});
