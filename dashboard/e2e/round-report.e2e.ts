// Writes a round directory, ux/rounds/<id>/{report.md,round.json,shots/}, and its row in
// ux/rounds/trend.md, from what measure-round wrote. With UX_WRITE_BASELINE=1 it also writes
// ux/baseline.json from the round's gating numbers and the scrollers at the measured-only
// viewports, refused unless every gate replay was measured on a committed page and harness and,
// for a value made worse, unless UX_DECISION names the decision.
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from '@playwright/test';
import { SCENARIOS } from '../ux/scenarios/index.ts';
import { aggregate, baselineFrom, compare, describeProblem, measuredOnlyAggregate, mergeAggregates, type Aggregate, type Baseline } from '../ux/ratchet.ts';
import { baselineAbout, baselineRefusal, missingGateReplays, roundDirRefusal, shortCommit, toolsOf, treeStamp } from '../ux/round-guard.ts';
import { REPLAY_METHOD } from './harness.ts';
import type { Measurement } from './measure.ts';
import { activeSessions } from '../src/flow.ts';
import { AXE_RULE_PREFIX, expectedFetches, flowProblems, fmtKeyFont, FLOW_VISIBLE_MIN, keyDataItems, METRIC_KEYS, SCREEN_ONLY_KEYS, screenOnlyProblems, statusProblems } from './metrics.ts';
import { metricsDir, readMeasurements } from './metrics-dir.ts';
import { FULL_PAGE_VIEWPORT, GATING, HEADING_FONT_PX, KEY_FONT_PX, SECONDARY_FONT_PX, THEMES, VIEWPORTS, type Theme } from './profiles.ts';

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
    g('key data', mine, (m) => keyDataItems(m).length === 0, `key ${max('key_font.deficit_px')} px short max (floor ${KEY_FONT_PX}), headings ${max('heading_font.deficit_px')} px short max (floor ${HEADING_FONT_PX}), secondary ${max('secondary_font.deficit_px')} px short max (floor ${SECONDARY_FONT_PX}); no key text in ${mine.filter((m) => m.key_font_min_px === null).length} replays`);
    g('copy', mine, (m) => m.metrics['copy.defects'] === 0, `${max('copy.defects')} distinct items max (${[...new Set(mine.flatMap((m) => m.details.copy.map((c) => c.key.split(' ')[0])))].join(', ') || 'none'})`);
    const played = mine.filter((m) => m.screen_only.length > 0);
    if (played.length > 0) g('gate 4 screen-only user', played, (m) => screenOnlyProblems(m.screen_only).length === 0, `${played.flatMap((m) => m.screen_only).filter((r) => r.reached).length} of ${played.flatMap((m) => m.screen_only).length} tasks reached; actions ${Math.max(...played.flatMap((m) => m.screen_only.map((r) => r.actions)))} max, glance ${Math.max(...played.flatMap((m) => m.screen_only.map((r) => r.looks)))} max`);
    const flow = mine.filter((m) => WITH_SESSIONS.has(m.scenario));
    g(`flow (${[...WITH_SESSIONS].join(', ')})`, flow, (m) => flowProblems(m).length === 0, `visible ratio ${Math.min(1, ...flow.map((m) => m.flow.visible_ratio))} min (bar ${FLOW_VISIBLE_MIN}), drawn ${flow.filter((m) => m.flow.drawn).length} of ${flow.length}, drawn under reduced motion ${flow.filter((m) => m.reduced_flow.drawn).length} of ${flow.length}`);
    const actionsOf = (m: Measurement) => SCENARIOS.find((s) => s.name === m.scenario)?.actions ?? [];
    const wordsOf = (m: Measurement) => SCENARIOS.find((s) => s.name === m.scenario)?.truth?.connection?.state;
    g('replay hermetic, live, a lost connection in words', mine, (m) => m.blocked_requests === 0 && m.snapshot_fetches === expectedFetches(actionsOf(m)) && statusProblems(m.status, m.states, actionsOf(m), wordsOf(m)).length === 0, `${mine.reduce((a, m) => a + m.blocked_requests, 0)} requests aborted; status after a lost connection: ${[...new Set(mine.filter((m) => actionsOf(m).includes('lose-connection')).map((m) => m.status[m.status.length - 1]))].join(', ') || 'no such scenario'}`);
  }
  return out;
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

  // The ratchet's basis: the gating replays, and the sideways scrollers at the measured-only
  // viewports (overflow.scrollers.<viewport>), ratcheted without a gate.
  const measuredOnly = measuredOnlyAggregate(all.filter((m) => !m.viewport.gating).map((m) => ({ scenario: m.scenario, viewport: m.viewport.name, scrollers: m.metrics['overflow.scrollers'] })));
  const gatingAgg = mergeAggregates(aggregate(all.filter((m) => m.viewport.gating)), measuredOnly);
  const allAgg = aggregate(all);
  const gates = hardGates(all);
  const pageDigest = createHash('sha256').update(readFileSync(BUILT)).digest('hex');
  const date = new Date().toISOString().slice(0, 10);
  const baselineFile = join(UX, 'baseline.json');
  const previous = existsSync(baselineFile) ? (JSON.parse(readFileSync(baselineFile, 'utf8')) as Baseline) : null;
  const problems = previous ? compare(previous, gatingAgg) : [];
  const ratchet = previous ? problems.map(describeProblem) : ['no baseline yet'];
  // Taken before anything is written: the round's own outputs are not the tree it measured.
  const stamp = treeStamp(REPO);
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

  // Full-page sheets: per theme, the whole page of every scenario after its last action at
  // FULL_PAGE_VIEWPORT, at half size side by side, so what lies below the fold is pictured.
  const fullViewport = VIEWPORTS.find((v) => v.name === FULL_PAGE_VIEWPORT)!;
  for (const theme of THEMES) {
    const shots = SCENARIOS.map((s) => ({ s: s.name, png: readFileSync(join(dir, 'shots', `full__${s.name}__${FULL_PAGE_VIEWPORT}__${theme}.png`)) }));
    const heights = shots.map(({ png }) => png.readUInt32BE(20));
    const w = fullViewport.width / 2;
    const h = Math.ceil(Math.max(...heights) / 2);
    const tiles = shots.map(({ s, png }, i) => `<figure><figcaption>${s}</figcaption><img src="data:image/png;base64,${png.toString('base64')}" style="height:${Math.ceil(heights[i]! / 2)}px"></figure>`).join('');
    await page.setViewportSize({ width: (w + 4) * shots.length + 8, height: h + 30 });
    await page.setContent(
      `<style>body{margin:0;padding:4px;display:grid;grid-template-columns:repeat(${shots.length},${w}px);gap:4px;align-items:start;background:#888;font:12px system-ui}figure{margin:0}img{width:${w}px;display:block}figcaption{height:18px;color:#fff}</style>${tiles}`,
    );
    const name = `full-${FULL_PAGE_VIEWPORT}-${theme}.png`;
    await page.screenshot({ path: join(roundDir, 'shots', name), fullPage: true });
    sheets.push(name);
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
      screen_only: m.screen_only,
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
  md.push('', '## Screen-only user (gate 4)', '');
  md.push('Deterministic, no model (e2e/screen-only.ts): it plays each task on the last state in the light theme, using only controls fully in view and uncovered, found by role and name; a click, a key press or a scroll is one action. Glance 1: the truth in the upper half of the first screen; 2: elsewhere on the first screen; 3: it needed a scroll.', '');
  md.push('| scenario | viewport | task | reached | actions | glance | steps | seen |', '|---|---|---|---|---|---|---|---|');
  for (const m of all.filter((x) => x.viewport.gating && x.screen_only.length > 0)) {
    for (const r of m.screen_only) md.push(`| ${m.scenario} | ${m.viewport.name} | ${r.task} | ${r.reached ? 'yes' : 'no'} | ${r.actions} | ${r.looks} | ${r.steps.filter((x) => x !== 'look').join(', ') || 'none'} | ${r.seen.replace(/\|/g, '/').slice(0, 60)} |`);
  }
  md.push('', `Ratcheted per scenario: ${SCREEN_ONLY_KEYS.map((k) => `\`${k}\``).join(', ')} (actions summed over the tasks, the worst glance, 1 when every task was reached).`);
  md.push('', '## Visible text (copy)', '', 'Distinct items of the visible-text probe (e2e/copy.ts) per scenario over the gating replays: words run together across elements, a raw identifier as a label, a control named by a glyph.', '');
  for (const s of SCENARIOS) {
    const items = new Map<string, string>();
    for (const m of all.filter((x) => x.scenario === s.name && x.viewport.gating)) for (const c of m.details.copy) if (!items.has(c.key)) items.set(c.key, c.label);
    md.push(`- ${s.name}: ${items.size ? [...items.entries()].map(([k, l]) => `\`${k}\` (${l.replace(/^\S+ /, '')})`).join('; ') : 'none'}`);
  }
  md.push('', '## Worst moments at the gating viewports', '', ...worstMoments(all.filter((m) => m.viewport.gating)));
  md.push('', '## Worst moments at the measured-only viewports (no screenshot)', '', ...worstMoments(all.filter((m) => !m.viewport.gating)));
  md.push('', '## Ratchet against `ux/baseline.json` (gating values)', '');
  md.push(...(ratchet.length ? ratchet.map((r) => `- ${r}`) : ['- every value equals the baseline within its tolerance']));
  md.push('', '## Contact sheets', '', `The last state of each scenario, 0.75 s after it arrived, at half size; then (\`full-…\`) the whole page of each scenario after its last action at ${FULL_PAGE_VIEWPORT}, at half size.`, '');
  for (const s of sheets) md.push(`![${s}](shots/${s})`);
  md.push('');
  writeFileSync(join(roundDir, 'report.md'), md.join('\n'));

  const trendFile = join(UX, 'rounds', 'trend.md');
  const header = ['# Trend', '', 'One row per round: hard-gate replays passing at the gating viewports (light / dark), and the worst values over every scenario at the gating viewports only (the ratchet\'s basis). "key font px" is the smallest key-data font there, n/a when no replay shows key data. Not like for like: R-01 to R-04 were measured with the host\'s fonts, R-05 on with the pinned fonts; R-05 also moved to the renderer and merged the Recent calls wrapping (R-05/notes.md). From R-06 the scenarios include `offline`.', '', '| round | date | commit | toggle | gate 1 | gate 2 | gate 5 axe | gate 5 keyboard | gate 5 motion | axe nodes | past box | key font px | layout shift |', '|---|---|---|---|---|---|---|---|---|---|---|---|---|'];
  const trendRows = existsSync(trendFile) ? readFileSync(trendFile, 'utf8').split('\n').filter((l) => /^\| R-/.test(l) && !l.startsWith(`| ${id} |`)).map(shortCommit) : [];
  const pass = (gate: string) => THEMES.map((t) => { const g = gates.find((x) => x.gate === gate && x.theme === t)!; return `${g.passed}/${g.of}`; }).join(' / ');
  const worst = (k: keyof Measurement['metrics']) => fmt(Math.max(...Object.values(gatingAgg).map((r) => r[k] ?? 0)));
  trendRows.push(`| ${id} | ${date} | ${round.commit} | ${round.toggle} | ${pass('gate 1 layout')} | ${pass('gate 2 always visible')} | ${pass('gate 5 axe')} | ${pass('gate 5 keyboard')} | ${pass('gate 5 reduced motion')} | ${worst('axe.serious_critical')} | ${worst('overflow.elements')} | ${keyFont(all.filter((m) => m.viewport.gating))} | ${worst('layout_shift.per_state_max')} |`);
  trendRows.sort();
  writeFileSync(trendFile, `${[...header, ...trendRows].join('\n')}\n`);

  if (process.env.UX_WRITE_BASELINE === '1') {
    const decision = process.env.UX_DECISION || undefined;
    const missing = missingGateReplays(all, { scenarios: SCENARIOS.map((s) => s.name), viewports: GATING.map((v) => v.name), themes: THEMES });
    const refused = baselineRefusal(missing, problems, decision, tools.fonts, stamp);
    if (refused) throw new Error(refused);
    // The commit is clean (refused above otherwise), so the round and the commit reproduce it.
    const baseline = baselineFrom(gatingAgg, baselineAbout(GATING.map((v) => v.name), id, stamp.page, pageDigest), previous ?? undefined, decision);
    writeFileSync(baselineFile, `${JSON.stringify(baseline, null, 2)}\n`);
  }
});
