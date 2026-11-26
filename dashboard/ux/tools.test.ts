// Break tests of the measuring tools that need no browser: each feeds a tool a bad input and
// asserts that it reports it.
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { afterAll, expect, test } from 'vitest';
import { fontFileProblems, fontMode, fontsLabel, MAX_FONT_BYTES, MONO_FAMILY, PINNED_FONTS, PINNED_LABEL, SANS_FAMILY } from '../e2e/fonts.ts';
import { KNOWN_FAILURES,stale, unexpected, wholeCheck, type KnownFailure } from '../e2e/known-failures.ts';
import {
  expectedFetches,
  flowProblems,
  fmtKeyFont,
  headingFontDeficit,
  keyDataItems,
  keyFontDeficit,
  mergeAxe,
  METRIC_KEYS,
  screenOnlyMetrics,
  screenOnlyProblems,
  secondaryFontDeficit,
  statusProblems,
  type AxeFinding,
  type Metrics,
} from '../e2e/metrics.ts';
import type { FlowProbe } from '../e2e/probe.ts';
import { METRICS_ROOT, metricsDir, readMeasurements } from '../e2e/metrics-dir.ts';
import { aggregate, baselineFrom, compare, describeProblem, measuredOnlyAggregate, mergeAggregates, toleranceOf, type Baseline } from './ratchet.ts';
import { aboutRound, baselineAbout, baselineRefusal, missingGateReplays, roundDirRefusal, shortCommit, toolsOf, treeStamp } from './round-guard.ts';

mkdirSync(METRICS_ROOT, { recursive: true });
const scratch = mkdtempSync(join(METRICS_ROOT, 'unit-'));
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

const zeros = Object.fromEntries(METRIC_KEYS.map((k) => [k, 0])) as unknown as Metrics;

test('metrics of two runs go to two directories, outside the Playwright output directory', () => {
  const a = metricsDir('round', 'R-01-1');
  const b = metricsDir('round', 'R-01-2');
  expect(a).not.toEqual(b);
  expect(a).not.toMatch(/test-results/);
  expect(a.startsWith(METRICS_ROOT)).toBe(true);
});

test('break: a measurement another run wrote is refused', () => {
  const dir = join(scratch, 'gate');
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, 'idle__1366x768__light.json'), JSON.stringify({ run: 'other', scenario: 'idle' }));
  expect(() => readMeasurements(dir, 'mine')).toThrow(/belongs to run other/);
  expect(readMeasurements(dir, 'other')).toHaveLength(1);
});

test('break: a round directory that holds a report is refused without --force', () => {
  const dir = join(scratch, 'R-99');
  mkdirSync(dir, { recursive: true });
  expect(roundDirRefusal(dir, false)).toBeNull();
  writeFileSync(join(dir, 'review-report.md'), 'a review');
  expect(roundDirRefusal(dir, false)).toBeNull();
  writeFileSync(join(dir, 'round.json'), '{}');
  expect(roundDirRefusal(dir, false)).toMatch(/already holds round.json/);
  expect(roundDirRefusal(dir, true)).toBeNull();
});

test('break: --baseline is refused when a gate replay is missing or incomplete, or a value got worse without a decision', () => {
  const gate = { scenarios: ['idle', 'busy'], viewports: ['1366x768'], themes: ['light'] };
  const m = (scenario: string, metrics: Metrics = zeros) => ({ scenario, viewport: { name: '1366x768', width: 1366, height: 768, gating: true }, theme: 'light' as const, metrics });
  expect(missingGateReplays([m('idle'), m('busy')], gate)).toEqual([]);
  expect(missingGateReplays([m('idle')], gate)).toEqual(['busy 1366x768 light']);
  expect(missingGateReplays([m('idle'), m('busy', { ...zeros, 'overflow.x_px': NaN })], gate)).toEqual(['busy 1366x768 light']);
  expect(baselineRefusal(['busy 1366x768 light'], [], 'D1')).toMatch(/1 gate replay\(s\) not measured/);
  const worse = [{ scenario: 'idle', metric: 'overflow.x_px', kind: 'worse' as const, baseline: 0, measured: 3 }];
  expect(baselineRefusal([], worse, undefined)).toMatch(/--decision/);
  expect(baselineRefusal([], worse, 'D7')).toBeNull();
  const previous: Baseline = { about: '', scenarios: { idle: { 'overflow.x_px': { value: 0, tolerance: 0 } } }, tolerances: {} };
  expect(baselineFrom({ idle: { 'overflow.x_px': 3 } }, '', previous, 'D7').scenarios.idle!['overflow.x_px']).toEqual({ value: 3, tolerance: 0, decision: 'D7' });
});

test('break: a round measured on two browser or axe versions is refused', () => {
  expect(toolsOf([{ tools: { chromium: '1', axe_core: '4', fonts: 'f' } }, { tools: { chromium: '1', axe_core: '4', fonts: 'f' } }])).toEqual({ chromium: '1', axe_core: '4', fonts: 'f' });
  expect(() => toolsOf([{ tools: { chromium: '1', axe_core: '4', fonts: 'f' } }, { tools: { chromium: '2', axe_core: '4', fonts: 'f' } }])).toThrow(/different tools/);
  expect(() => toolsOf([{ tools: { chromium: '', axe_core: '4', fonts: 'f' } }])).toThrow(/no Chromium/);
  expect(() => toolsOf([{ tools: { chromium: '1', axe_core: '4', fonts: 'f' } }, { tools: { chromium: '1', axe_core: '4', fonts: 'host' } }])).toThrow(/different tools/);
  expect(() => toolsOf([{ tools: { chromium: '1', axe_core: '4', fonts: '' } }])).toThrow(/no fonts/);
});

test('the pinned fonts are the committed files with their declared sha256, under 200 KB, with their licence', () => {
  expect(fontFileProblems()).toEqual([]);
  expect(PINNED_FONTS.map((f) => `${f.family} ${f.weight}`).sort()).toEqual([`${SANS_FAMILY} 400`, `${SANS_FAMILY} 600`, `${MONO_FAMILY} 400`, `${MONO_FAMILY} 600`].sort());
});

test('break: a font file that is not the declared one, too many bytes, or no licence is reported', () => {
  const dir = join(scratch, 'fonts/');
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, 'a.woff2'), 'not the font');
  const declared = [{ file: 'a.woff2', family: SANS_FAMILY, weight: 400, sha256: '0'.repeat(64) }, { file: 'gone.woff2', family: SANS_FAMILY, weight: 600, sha256: '0'.repeat(64) }];
  const problems = fontFileProblems(declared, dir);
  expect(problems).toHaveLength(3);
  expect(problems[0]).toMatch(/^a\.woff2: sha256 [0-9a-f]{64}, declared 0{64}$/);
  expect(problems[1]).toBe('gone.woff2: missing');
  expect(problems[2]).toMatch(/^OFL\.txt: no SIL Open Font License/);
  writeFileSync(join(dir, 'big.woff2'), Buffer.alloc(MAX_FONT_BYTES + 1));
  writeFileSync(join(dir, 'OFL.txt'), 'SIL Open Font License, Version 1.1');
  expect(fontFileProblems([{ file: 'big.woff2', family: SANS_FAMILY, weight: 400, sha256: '0'.repeat(64) }], dir)).toEqual([
    expect.stringMatching(/^big\.woff2: sha256/),
    `the font files hold ${MAX_FONT_BYTES + 1} bytes, more than ${MAX_FONT_BYTES}`,
  ]);
});

test('break: UX_FONTS takes pinned, host or host:<family>, and a baseline only from the pinned fonts', () => {
  expect(fontMode('')).toEqual({ kind: 'pinned' });
  expect(fontMode('host')).toEqual({ kind: 'host', sans: null });
  expect(fontMode('host:DejaVu Sans')).toEqual({ kind: 'host', sans: 'DejaVu Sans' });
  expect(() => fontMode('hosted')).toThrow(/UX_FONTS=hosted/);
  expect(() => fontMode('host:')).toThrow(/UX_FONTS=host:/);
  expect(baselineRefusal([], [], undefined, PINNED_LABEL)).toBeNull();
  expect(baselineRefusal([], [], undefined, fontsLabel({ kind: 'host', sans: null }))).toMatch(/--baseline refused: the round was measured with the fonts "host"/);
});

test('break: axe nodes are the union over every scan, not the largest scan', () => {
  const union = new Map<string, AxeFinding>();
  const f = (targets: string[]): AxeFinding => ({ id: 'rule', impact: 'serious', help: '', nodes: targets.length, targets });
  mergeAxe(union, [f(['#a'])]);
  mergeAxe(union, [f(['#b'])]);
  mergeAxe(union, [f(['#a'])]);
  expect(union.get('rule')).toMatchObject({ nodes: 2, targets: ['#a', '#b'] });
});

test('break: a known failure holds its own items only; a new item in the same gate fails, and a fixed item goes stale', () => {
  const list: KnownFailure[] = [{ finding: 'UXF-900', check: 'axe', scenarios: ['busy'], items: ['rule-a #x'] }, { finding: 'UXF-901', check: 'flow' }];
  expect(unexpected('axe', 'busy', ['rule-a #x'], list)).toEqual([]);
  expect(unexpected('axe', 'busy', ['rule-a #x', 'rule-a #y', 'rule-b #x'], list)).toEqual(['rule-a #y', 'rule-b #x']);
  expect(unexpected('axe', 'idle', ['rule-a #x'], list)).toEqual(['rule-a #x']);
  expect(stale('axe', 'busy', new Set(), list)).toEqual(['UXF-900: rule-a #x']);
  expect(stale('axe', 'busy', new Set(['rule-a #x']), list)).toEqual([]);
  expect(wholeCheck('flow', 'busy', list)).toBe('UXF-901');
  expect(wholeCheck('axe', 'busy', list)).toBeNull();
});

test('every known failure names its items, except a whole single-valued check', () => {
  for (const k of KNOWN_FAILURES) expect(k.check === 'flow' || (k.items?.length ?? 0) > 0, k.finding).toBe(true);
});

test('break: the ratchet counts per axe rule, so a violation swapped for another of the same count fails', () => {
  const m = (rules: Record<string, number>) => ({ scenario: 's', metrics: { ...zeros, 'axe.serious_critical': 2 }, axe_rules: rules });
  const measured = aggregate([m({ 'rule-a': 2 })]);
  const baseline = baselineFrom(measured, 'test');
  expect(compare(baseline, measured)).toEqual([]);
  const swapped = compare(baseline, aggregate([m({ 'rule-b': 2 })]));
  expect(swapped.map((p) => `${p.metric} ${p.kind}`)).toEqual(['axe.rule.rule-a improved-not-taken', 'axe.rule.rule-b worse']);
});

test('break: a page without key data reads n/a and no deficit, not a deficit of 14 px', () => {
  expect(keyFontDeficit(null)).toBe(0);
  expect(fmtKeyFont(null)).toBe('n/a');
  expect(keyFontDeficit(10.4)).toBe(3.6);
  expect(keyFontDeficit(16)).toBe(0);
  expect(secondaryFontDeficit(10.4)).toBe(1.6);
  expect(secondaryFontDeficit(12.25)).toBe(0);
});

const flow = (drawn: boolean, visible = 1): FlowProbe => ({ present: true, at: 'canvas', visible_ratio: visible, drawn, drawn_share: drawn ? 0.05 : 0 });

test('break: a blank token-flow canvas fails the flow gate, also when only the reduced-motion replay is blank', () => {
  expect(flowProblems({ flow: flow(true), reduced_flow: flow(true) })).toEqual([]);
  expect(flowProblems({ flow: flow(false), reduced_flow: flow(true) })).toEqual(['the canvas is blank (share drawn 0)']);
  expect(flowProblems({ flow: flow(true), reduced_flow: flow(false) })).toEqual(['the canvas is blank under reduced motion (share drawn 0)']);
  expect(flowProblems({ flow: flow(true, 0.5), reduced_flow: flow(true) })).toEqual(['0.5 of the canvas in view and uncovered, under 0.95']);
});

test('break: the ratchet holds the flow keys, higher is better: a blank or hidden canvas is worse, a better one must be taken', () => {
  const m = (drawn: boolean, reducedDrawn: boolean, visible: number) => ({ scenario: 's', metrics: zeros, axe_rules: {}, flow: flow(drawn, visible), reduced_flow: flow(reducedDrawn) });
  const measured = aggregate([m(true, true, 1), m(true, true, 0.98)]);
  expect(measured.s).toMatchObject({ 'flow.visible_ratio': 0.98, 'flow.drawn': 1 });
  const baseline = baselineFrom(measured, 'test');
  expect(compare(baseline, measured)).toEqual([]);
  const kinds = (ms: ReturnType<typeof m>[]) => compare(baseline, aggregate(ms)).map((p) => `${p.metric} ${p.kind}`);
  expect(kinds([m(true, false, 0.98)])).toEqual(['flow.drawn worse']);
  expect(kinds([m(true, true, 0.5)])).toEqual(['flow.visible_ratio worse']);
  expect(kinds([m(true, true, 1)])).toEqual(['flow.visible_ratio improved-not-taken']);
});

test('break: the layout-shift tolerance is relative and on the worse side only: 0.007 to 0.017 fails, 0.008 passes, any improvement must be taken', () => {
  expect(toleranceOf('layout_shift.per_state_max', 0.007)).toBe(0.002);
  expect(toleranceOf('layout_shift.per_state_max', 0.059)).toBe(0.0059);
  expect(toleranceOf('overflow.elements', 3)).toBe(0);
  const baseline: Baseline = { about: 'test', scenarios: { s: { 'layout_shift.per_state_max': { value: 0.007, tolerance: 0.002 } } }, tolerances: {} };
  const kind = (v: number) => compare(baseline, { s: { 'layout_shift.per_state_max': v } }).map((p) => p.kind);
  expect(kind(0.017)).toEqual(['worse']);
  expect(kind(0.01)).toEqual(['worse']);
  expect(kind(0.008)).toEqual([]);
  expect(kind(0.007)).toEqual([]);
  expect(kind(0.006)).toEqual(['improved-not-taken']);
  expect(kind(0.004)).toEqual(['improved-not-taken']);
  // The R-06 review's input: errors' baseline 0.003, measured 0.001, inside the 0.002 band.
  const errors: Baseline = { about: 'test', scenarios: { errors: { 'layout_shift.per_state_max': { value: 0.003, tolerance: 0.002 } } }, tolerances: {} };
  expect(compare(errors, { errors: { 'layout_shift.per_state_max': 0.001 } }).map(describeProblem)).toEqual([
    'errors layout_shift.per_state_max: better than the baseline: take it into ux/baseline.json in this change (baseline 0.003, measured 0.001)',
  ]);
});

test('trend rows carry 8 characters of the commit, also a row written with 12', () => {
  expect(shortCommit('| R-03 | 2026-11-26 | 6bd91d70289c | present | 8/8 |')).toBe('| R-03 | 2026-11-26 | 6bd91d70 | present | 8/8 |');
  expect(shortCommit('| R-04 | 2026-11-26 | 6bd91d70+changes | present |')).toBe('| R-04 | 2026-11-26 | 6bd91d70+changes | present |');
  expect(shortCommit('| R-05 | 2026-11-26 | 0123456789ab+changes | present |')).toBe('| R-05 | 2026-11-26 | 01234567+changes | present |');
});

test('break: the key-data gate refuses a replay that found no key text, and names headings apart', () => {
  const details = { key_font_below: [], heading_font_below: ['13.3px h2'], secondary_font_below: [] };
  expect(keyDataItems({ key_font_min_px: null, details })).toEqual(['no key text', 'heading 13.3px h2']);
  expect(keyDataItems({ key_font_min_px: 14, details: { ...details, heading_font_below: [] } })).toEqual([]);
  expect(headingFontDeficit(13.3)).toBe(0.7);
  expect(headingFontDeficit(15.4)).toBe(0);
});

test('break: the replay status is live until the connection is lost and a non-live word after it', () => {
  const words = ['\\boffline\\b'];
  expect(statusProblems(['live', 'live', 'live'], 3, ['show-all-calls'])).toEqual([]);
  expect(statusProblems(['live', 'polling every 5 s'], 2, [])).toEqual(['status "polling every 5 s" at step 2, not live']);
  expect(statusProblems(['live', 'live', 'offline'], 2, ['lose-connection'], words)).toEqual([]);
  expect(statusProblems(['live', 'live', 'live'], 2, ['lose-connection'], words)).toEqual(['status "live" at step 3, after the connection was lost']);
  expect(statusProblems(['live', 'live', 'polling every 5 s'], 2, ['lose-connection'], words)).toEqual(['status "polling every 5 s" at step 3, after the connection was lost']);
  expect(statusProblems(['live', 'live'], 2, ['lose-connection'], words)).toEqual(['no status after the connection was lost']);
  expect(expectedFetches(['lose-connection'])).toBe(2);
  expect(expectedFetches([])).toBe(1);
});

const task = (t: string, reached: boolean, actions: number, looks: number) => ({ task: t, reached, actions, looks });

test('break: gate 4 refuses a task not reached; the ratchet sums actions, keeps the worst glance and the worst reach', () => {
  expect(screenOnlyProblems([task('a', true, 3, 3)])).toEqual([]);
  expect(screenOnlyProblems([task('a', true, 3, 3), task('b', false, 12, 3)])).toEqual(['b: the truth was not reached in 12 actions']);
  expect(screenOnlyMetrics([])).toEqual({});
  expect(screenOnlyMetrics([task('a', true, 3, 3), task('b', true, 0, 1)])).toEqual({ 'screen_only.actions': 3, 'screen_only.looks': 3, 'screen_only.reached': 1 });
  const m = (tasks: ReturnType<typeof task>[]) => ({ scenario: 's', metrics: zeros, axe_rules: {}, screen_only: tasks });
  const measured = aggregate([m([task('a', true, 3, 3)]), m([]), m([task('a', true, 1, 1)])]);
  expect(measured.s).toMatchObject({ 'screen_only.actions': 3, 'screen_only.looks': 3, 'screen_only.reached': 1 });
  const baseline = baselineFrom(measured, 'test');
  const kinds = (tasks: ReturnType<typeof task>[]) => compare(baseline, aggregate([m(tasks)])).map((p) => `${p.metric} ${p.kind}`);
  expect(kinds([task('a', false, 12, 3)])).toEqual(['screen_only.actions worse', 'screen_only.reached worse']);
  expect(kinds([task('a', true, 0, 1)])).toEqual(['screen_only.actions improved-not-taken', 'screen_only.looks improved-not-taken']);
});

test('break: the scrollers at a measured-only viewport are ratcheted per viewport, the worst over the themes', () => {
  const only = measuredOnlyAggregate([
    { scenario: 's', viewport: '390x844', scrollers: 3 },
    { scenario: 's', viewport: '390x844', scrollers: 5 },
    { scenario: 's', viewport: '960x1080', scrollers: 0 },
  ]);
  expect(only).toEqual({ s: { 'overflow.scrollers.390x844': 5, 'overflow.scrollers.960x1080': 0 } });
  const baseline = baselineFrom(mergeAggregates({ s: { 'overflow.x_px': 0 } }, only), 'test');
  const kinds = (n: number) => compare(baseline, mergeAggregates({ s: { 'overflow.x_px': 0 } }, { s: { ...only.s, 'overflow.scrollers.390x844': n } })).map((p) => `${p.metric} ${p.kind}`);
  expect(kinds(5)).toEqual([]);
  expect(kinds(6)).toEqual(['overflow.scrollers.390x844 worse']);
  expect(kinds(4)).toEqual(['overflow.scrollers.390x844 improved-not-taken']);
});

test('break: a new baseline takes an improvement inside the tolerance and does not creep up inside it', () => {
  const previous: Baseline = { about: '', scenarios: { s: { 'layout_shift.per_state_max': { value: 0.043, tolerance: 0.0043 } } }, tolerances: {} };
  const take = (v: number, decision?: string) => baselineFrom({ s: { 'layout_shift.per_state_max': v } }, '', previous, decision).scenarios.s!['layout_shift.per_state_max'];
  expect(take(0.039)).toEqual({ value: 0.039, tolerance: 0.0039 });
  expect(take(0.046)).toEqual({ value: 0.043, tolerance: 0.0043 });
  expect(take(0.06, 'D9')).toEqual({ value: 0.06, tolerance: 0.006, decision: 'D9' });
});

test('break: --baseline is refused on a page or harness that differs from a commit', () => {
  expect(baselineRefusal([], [], undefined, PINNED_LABEL, { page: 'abcdef12', harness: 'abcdef12' })).toBeNull();
  expect(baselineRefusal([], [], undefined, PINNED_LABEL, { page: 'abcdef12+changes', harness: 'abcdef12' })).toMatch(/--baseline refused: the page \(abcdef12\+changes\) differ/);
  expect(baselineRefusal([], [], undefined, PINNED_LABEL, { page: 'unknown', harness: 'unknown' })).toMatch(/refused/);
});

// A repository of its own under the metrics scratch directory, with the page's sources committed.
function gitRepo(): string {
  const repo = mkdtempSync(join(scratch, 'git-'));
  const git = (...args: string[]) => execFileSync('git', ['-c', 'user.name=t', '-c', 'user.email=t@example.invalid', '-c', 'commit.gpgsign=false', ...args], { cwd: repo, encoding: 'utf8' });
  git('init', '-q');
  for (const f of ['dashboard/ui/page.ui.yaml', 'dashboard/index.html', 'dashboard/vite.config.ts', 'dashboard/src/main.ts', 'dashboard/e2e/a.ts', 'dashboard/ux/rounds/R-01/report.md']) {
    mkdirSync(join(repo, f, '..'), { recursive: true });
    writeFileSync(join(repo, f), 'v1\n');
  }
  git('add', '-A');
  git('commit', '-q', '-m', 'base');
  return repo;
}

test('break: the page is marked +changes for an edit of the document, index.html or vite.config.ts, not for a round directory', () => {
  const repo = gitRepo();
  const clean = treeStamp(repo);
  expect(clean.page).toMatch(/^[0-9a-f]{8}$/);
  expect(clean.harness).toBe(clean.page);
  for (const f of ['dashboard/ui/page.ui.yaml', 'dashboard/index.html', 'dashboard/vite.config.ts']) {
    writeFileSync(join(repo, f), 'v2\n');
    expect(treeStamp(repo).page, f).toBe(`${clean.page}+changes`);
    writeFileSync(join(repo, f), 'v1\n');
  }
  writeFileSync(join(repo, 'dashboard/ux/rounds/R-01/report.md'), 'v2\n');
  writeFileSync(join(repo, 'dashboard/ux/baseline.json'), '{}\n');
  expect(treeStamp(repo)).toEqual(clean);
  writeFileSync(join(repo, 'dashboard/e2e/a.ts'), 'v2\n');
  expect(treeStamp(repo)).toEqual({ page: clean.page, harness: `${clean.page}+changes` });
});

test('the baseline names the round and the commit it was taken on, and a kept round agrees on the page', () => {
  const file = new URL('./baseline.json', import.meta.url);
  const about = (JSON.parse(readFileSync(file, 'utf8')) as Baseline).about;
  const named = aboutRound(about);
  expect(named, about).not.toBeNull();
  const round = new URL(`./rounds/${named!.round}/round.json`, import.meta.url);
  if (existsSync(round)) expect((JSON.parse(readFileSync(round, 'utf8')) as { page_sha256: string }).page_sha256.startsWith(named!.page)).toBe(true);
  expect(aboutRound(baselineAbout(['1366x768'], 'R-07', 'abcdef12', 'f'.repeat(64)))).toEqual({ round: 'R-07', commit: 'abcdef12', page: 'f'.repeat(12) });
  expect(aboutRound('Re-taken by the harness at commit abcdef12 on the built page of round R-05 with sha256 ffffffffffff.')).toEqual({ round: 'R-05', commit: 'abcdef12', page: 'f'.repeat(12) });
  expect(aboutRound('measured on the built page with sha256 951512ba2fae (page commit 076c9fb9+changes)')).toBeNull();
});
