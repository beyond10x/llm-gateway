// The findings directory: every record carries the header fields of the loop, every evidence line
// it cites exists, and no S1 or S2 finding is closed without a regression asset that names a test
// declared in dashboard/e2e.
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from 'vitest';
import { KNOWN_FAILURES } from '../e2e/known-failures.ts';

const DIR = fileURLToPath(new URL('./findings/', import.meta.url));
const ROOT = fileURLToPath(new URL('../', import.meta.url));
const REPO = join(ROOT, '..');
const FIELDS = ['id', 'title', 'class', 'severity', 'metric', 'status', 'found_by', 'evidence_recording', 'evidence_screenshot', 'evidence_lines', 'regression_asset'];
const CLASSES = ['layout', 'cue', 'affordance', 'copy', 'noise', 'engine', 'input'];

type Finding = Record<string, string> & { file: string };

export function parse(file: string, text: string): Finding {
  const head = text.match(/^---\n([\s\S]*?)\n---/);
  const out: Finding = { file };
  for (const line of head?.[1]?.split('\n') ?? []) {
    const m = line.match(/^([a-z_]+):\s*(.*)$/);
    if (m) out[m[1]!] = m[2]!.trim();
  }
  return out;
}

// The static part of every test title declared in a spec file: the literal text of test('…'),
// test("…") or test(`…`) up to the first `${`.
export function declaredTests(source: string): string[] {
  const out: string[] = [];
  for (const m of source.matchAll(/\btest(?:\.describe)?\(\s*(['"`])((?:\\.|(?!\1)[^\\])*?)\1/g)) {
    out.push(m[2]!.split('${')[0]!);
  }
  return out;
}

// A closed record's asset: a spec file under e2e/ and the quoted start of a test title it declares.
function assetProblem(asset: string, closed: boolean, root: string): string | null {
  const m = asset.match(/^(\S+)\s+"([^"]+)"/);
  if (!m) return 'names no file and quoted test title';
  const file = join(root, m[1]!);
  if (!existsSync(file)) return `${m[1]} does not exist`;
  const source = readFileSync(file, 'utf8');
  if (closed) {
    if (!/^e2e\/[^/]+\.e2e\.ts$/.test(m[1]!)) return `${m[1]} is not a spec file in dashboard/e2e`;
    const title = m[2]!;
    if (!declaredTests(source).some((t) => t.startsWith(title) || (t.length > 0 && title.startsWith(t)))) return `${m[1]} declares no test "${title}"`;
  } else if (!source.includes(m[2]!)) {
    return `${m[1]} does not hold "${m[2]}"`;
  }
  return null;
}

// Every `path:line` and `path:from-to` of the evidence lines, path from the repository root; a bare
// number after a path belongs to that path. Text in parentheses is a comment.
export function evidenceRefs(value: string): { path: string; line: number }[] {
  const out: { path: string; line: number }[] = [];
  const bare = value.replace(/\([^)]*\)/g, ' ').replace(/`[^`]*`/g, ' ');
  let path: string | null = null;
  for (const token of bare.split(/[\s,;]+/).filter(Boolean)) {
    const withPath = token.match(/^([\w./-]+\.\w+):(\d+)(?:-(\d+))?$/);
    const bareLine = token.match(/^(\d+)(?:-(\d+))?$/);
    if (withPath) {
      path = withPath[1]!;
      out.push({ path, line: Number(withPath[3] ?? withPath[2]) });
    } else if (bareLine && path) {
      out.push({ path, line: Number(bareLine[2] ?? bareLine[1]) });
    }
  }
  return out;
}

function evidenceProblems(value: string, repo: string): string[] {
  const out: string[] = [];
  for (const ref of evidenceRefs(value)) {
    const file = join(repo, ref.path);
    if (!existsSync(file)) {
      out.push(`evidence ${ref.path} does not exist`);
      continue;
    }
    const lines = readFileSync(file, 'utf8').split('\n').length;
    if (ref.line > lines) out.push(`evidence ${ref.path}:${ref.line} is past the end (${lines} lines)`);
  }
  return out;
}

export function problems(f: Finding, root: string = ROOT, repo: string = REPO): string[] {
  const out: string[] = [];
  for (const k of FIELDS) if (!f[k]) out.push(`${f.file}: no ${k}`);
  if (f.id && `${f.id}.md` !== f.file) out.push(`${f.file}: id ${f.id} does not match the file name`);
  if (f.severity && !/^S[123]$/.test(f.severity)) out.push(`${f.file}: severity ${f.severity}`);
  if (f.class && !CLASSES.includes(f.class)) out.push(`${f.file}: class ${f.class}`);
  if (f.status && !['open', 'closed'].includes(f.status)) out.push(`${f.file}: status ${f.status}`);
  if (f.evidence_lines) for (const p of evidenceProblems(f.evidence_lines, repo)) out.push(`${f.file}: ${p}`);
  const hasAsset = !!f.regression_asset && !/^none\b/.test(f.regression_asset);
  if (hasAsset) {
    const p = assetProblem(f.regression_asset!, f.status === 'closed', root);
    if (p) out.push(`${f.file}: regression asset: ${p}`);
  }
  if (f.status === 'closed' && /^S[12]$/.test(f.severity ?? '') && !hasAsset) out.push(`${f.file}: closed ${f.severity} without a regression asset`);
  return out;
}

const records = readdirSync(DIR)
  .filter((f) => /^UXF-\d{3}\.md$/.test(f))
  .sort()
  .map((f) => parse(f, readFileSync(join(DIR, f), 'utf8')));

const record = (fields: Record<string, string>) =>
  parse('UXF-999.md', `---\n${Object.entries({ id: 'UXF-999', title: 't', class: 'layout', severity: 'S2', metric: 'm', status: 'closed', found_by: 'f', evidence_recording: 'none', evidence_screenshot: 's', evidence_lines: 'dashboard/src/App.vue:1', regression_asset: 'none', ...fields }).map(([k, v]) => `${k}: ${v}`).join('\n')}\n---\n`);

test('every finding record is complete, and no closed S1 or S2 finding lacks a regression asset', () => {
  expect(records.length).toBeGreaterThan(0);
  expect(records.flatMap((r) => problems(r))).toEqual([]);
});

test('break: a closed S2 record without a regression asset is refused', () => {
  expect(problems(record({}))).toEqual(['UXF-999.md: closed S2 without a regression asset']);
});

test('break: a closed record whose asset names no declared e2e test is refused', () => {
  expect(problems(record({ regression_asset: 'e2e/gates.e2e.ts "no such test"' }))).toEqual(['UXF-999.md: regression asset: e2e/gates.e2e.ts declares no test "no such test"']);
  // The words appear in gates.e2e.ts, but only in a comment.
  expect(problems(record({ regression_asset: 'e2e/gates.e2e.ts "the theme toggle drives a page itself"' }))).toEqual([
    'UXF-999.md: regression asset: e2e/gates.e2e.ts declares no test "the theme toggle drives a page itself"',
  ]);
  expect(problems(record({ regression_asset: 'ux/findings.test.ts "every finding record is complete"' }))).toEqual([
    'UXF-999.md: regression asset: ux/findings.test.ts is not a spec file in dashboard/e2e',
  ]);
  expect(problems(record({ regression_asset: 'e2e/gates.e2e.ts "affordance: a theme toggle in the header sets the theme"' }))).toEqual([]);
});

test('break: evidence lines past the end of their file, or in a file that does not exist, are refused', () => {
  const lines = readFileSync(join(REPO, 'dashboard/src/App.vue'), 'utf8').split('\n').length;
  expect(problems(record({ evidence_lines: `dashboard/src/App.vue:${lines + 1} (a line past the end), 3`, regression_asset: 'e2e/gates.e2e.ts "affordance: a theme toggle"' }))).toEqual([
    `UXF-999.md: evidence dashboard/src/App.vue:${lines + 1} is past the end (${lines} lines)`,
  ]);
  expect(problems(record({ evidence_lines: 'dashboard/src/App.vue:2, 99999-99999; dashboard/src/Gone.vue:1', regression_asset: 'e2e/gates.e2e.ts "affordance: a theme toggle"' }))).toEqual([
    `UXF-999.md: evidence dashboard/src/App.vue:99999 is past the end (${lines} lines)`,
    'UXF-999.md: evidence dashboard/src/Gone.vue does not exist',
  ]);
});

test('evidence references are read with their ranges and bare line numbers', () => {
  expect(evidenceRefs('dashboard/src/App.vue:75-78, 125-128 (`watchdogStale` sets class `alarm`, 9); dashboard/src/style.css:122')).toEqual([
    { path: 'dashboard/src/App.vue', line: 78 },
    { path: 'dashboard/src/App.vue', line: 128 },
    { path: 'dashboard/src/style.css', line: 122 },
  ]);
});

test('every known hard-gate failure names an open finding', () => {
  const open = new Set(records.filter((r) => r.status === 'open').map((r) => r.id));
  for (const k of KNOWN_FAILURES) expect(open.has(k.finding), k.finding).toBe(true);
});
