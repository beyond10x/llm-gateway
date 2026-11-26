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

// One cited place: a file from the repository root, the lines it cites (none: the whole file), and
// the tokens the record names for it (the backticked text of its comment).
export interface EvidenceRef {
  path: string;
  from: number | null;
  to: number | null;
  tokens: string[];
}

// Reads `path:line`, `path:from-to` and a bare `path`; a bare number after a path belongs to that
// path. A comment in parentheses belongs to every reference since the previous comment, and its
// backticked text is the tokens the cited lines must hold.
export function evidenceRefs(value: string): EvidenceRef[] {
  const out: EvidenceRef[] = [];
  let pending: EvidenceRef[] = [];
  let path: string | null = null;
  let i = 0;
  while (i < value.length) {
    const ch = value[i]!;
    if (ch === '(') {
      let depth = 0;
      let j = i;
      let inTick = false;
      for (; j < value.length; j++) {
        const c = value[j]!;
        if (c === '`') inTick = !inTick;
        else if (!inTick && c === '(') depth += 1;
        else if (!inTick && c === ')' && --depth === 0) break;
      }
      const comment = value.slice(i + 1, j);
      const tokens = [...comment.matchAll(/`([^`]+)`/g)].map((m) => m[1]!);
      for (const r of pending) r.tokens = tokens;
      pending = [];
      i = j + 1;
      continue;
    }
    const atStart = i === 0 || /[\s,;]/.test(value[i - 1]!);
    const rest = atStart ? value.slice(i) : '';
    const withPath = rest.match(/^([\w./-]+\.\w+)(?::(\d+)(?:-(\d+))?)?(?![\w.])/);
    const bare = path ? rest.match(/^(\d+)(?:-(\d+))?(?![\w.])/) : null;
    if (withPath && /[/]/.test(withPath[1]!)) {
      path = withPath[1]!;
      const from = withPath[2] ? Number(withPath[2]) : null;
      const ref = { path, from, to: from === null ? null : Number(withPath[3] ?? withPath[2]), tokens: [] };
      out.push(ref);
      pending.push(ref);
      i += withPath[0].length;
    } else if (bare && path) {
      const ref = { path, from: Number(bare[1]), to: Number(bare[2] ?? bare[1]), tokens: [] };
      out.push(ref);
      pending.push(ref);
      i += bare[0].length;
    } else {
      i += 1;
    }
  }
  return out;
}

function evidenceProblems(value: string, repo: string): string[] {
  const out: string[] = [];
  for (const ref of evidenceRefs(value)) {
    const where = ref.from === null ? ref.path : `${ref.path}:${ref.from}${ref.to !== ref.from ? `-${ref.to}` : ''}`;
    const file = join(repo, ref.path);
    if (!existsSync(file)) {
      out.push(`evidence ${ref.path} does not exist`);
      continue;
    }
    const lines = readFileSync(file, 'utf8').split('\n');
    if (ref.to !== null && ref.to > lines.length) {
      out.push(`evidence ${ref.path}:${ref.to} is past the end (${lines.length} lines)`);
      continue;
    }
    if (ref.tokens.length === 0) {
      out.push(`evidence ${where} names no token in backticks to find there`);
      continue;
    }
    const cited = ref.from === null ? lines.join('\n') : lines.slice(ref.from - 1, ref.to!).join('\n');
    if (!ref.tokens.some((t) => cited.includes(t))) out.push(`evidence ${where} holds none of ${ref.tokens.map((t) => `\`${t}\``).join(', ')}`);
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
  parse('UXF-999.md', `---\n${Object.entries({ id: 'UXF-999', title: 't', class: 'layout', severity: 'S2', metric: 'm', status: 'closed', found_by: 'f', evidence_recording: 'none', evidence_screenshot: 's', evidence_lines: 'dashboard/src/essui/bind.ts:1 (`The binding`)', regression_asset: 'none', ...fields }).map(([k, v]) => `${k}: ${v}`).join('\n')}\n---\n`);

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
  const lines = readFileSync(join(REPO, 'dashboard/src/essui/bind.ts'), 'utf8').split('\n').length;
  expect(problems(record({ evidence_lines: `dashboard/src/essui/bind.ts:${lines + 1} (a line past the end, \`x\`)`, regression_asset: 'e2e/gates.e2e.ts "affordance: a theme toggle"' }))).toEqual([
    `UXF-999.md: evidence dashboard/src/essui/bind.ts:${lines + 1} is past the end (${lines} lines)`,
  ]);
  expect(problems(record({ evidence_lines: 'dashboard/src/essui/bind.ts:99999-99999 (\`x\`); dashboard/src/Gone.vue:1 (\`y\`)', regression_asset: 'e2e/gates.e2e.ts "affordance: a theme toggle"' }))).toEqual([
    `UXF-999.md: evidence dashboard/src/essui/bind.ts:99999 is past the end (${lines} lines)`,
    'UXF-999.md: evidence dashboard/src/Gone.vue does not exist',
  ]);
});

test('break: a cited line that does not hold a token the record names is refused, as is a citation without a token', () => {
  const source = readFileSync(join(REPO, 'dashboard/src/essui/bind.ts'), 'utf8').split('\n');
  const at = source.findIndex((l) => l.includes('WATCHDOG_STALE_MS =')) + 1;
  const asset = { regression_asset: 'e2e/gates.e2e.ts "affordance: a theme toggle"' };
  expect(problems(record({ evidence_lines: `dashboard/src/essui/bind.ts:${at} (\`WATCHDOG_STALE_MS\`)`, ...asset }))).toEqual([]);
  expect(problems(record({ evidence_lines: 'dashboard/src/essui/bind.ts (\`WATCHDOG_STALE_MS\`)', ...asset }))).toEqual([]);
  expect(problems(record({ evidence_lines: `dashboard/src/essui/bind.ts:${at + 1} (\`WATCHDOG_STALE_MS\`)`, ...asset }))).toEqual([
    `UXF-999.md: evidence dashboard/src/essui/bind.ts:${at + 1} holds none of \`WATCHDOG_STALE_MS\``,
  ]);
  expect(problems(record({ evidence_lines: `dashboard/src/essui/bind.ts:${at} (the stale limit)`, ...asset }))).toEqual([
    `UXF-999.md: evidence dashboard/src/essui/bind.ts:${at} names no token in backticks to find there`,
  ]);
});

test('evidence references are read with their ranges, bare line numbers, whole files and the tokens of their comment', () => {
  expect(evidenceRefs('dashboard/src/App.vue:75-78, 125-128 (`watchdogStale` sets class `alarm`, 9); dashboard/src/style.css:122 (`.x` (a) b), dashboard/src/flow.ts (`FONT_PX`)')).toEqual([
    { path: 'dashboard/src/App.vue', from: 75, to: 78, tokens: ['watchdogStale', 'alarm'] },
    { path: 'dashboard/src/App.vue', from: 125, to: 128, tokens: ['watchdogStale', 'alarm'] },
    { path: 'dashboard/src/style.css', from: 122, to: 122, tokens: ['.x'] },
    { path: 'dashboard/src/flow.ts', from: null, to: null, tokens: ['FONT_PX'] },
  ]);
});
test('every known hard-gate failure names an open finding', () => {
  const open = new Set(records.filter((r) => r.status === 'open').map((r) => r.id));
  for (const k of KNOWN_FAILURES) expect(open.has(k.finding), k.finding).toBe(true);
});
