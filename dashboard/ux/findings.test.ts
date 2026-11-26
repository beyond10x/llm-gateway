// The findings directory: every record carries the header fields of the loop, and no S1 or S2
// finding is closed without a regression asset that exists.
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from 'vitest';
import { KNOWN_FAILURES } from '../e2e/known-failures.ts';

const DIR = fileURLToPath(new URL('./findings/', import.meta.url));
const ROOT = fileURLToPath(new URL('../', import.meta.url));
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

// The asset's file, before its quoted test title, must exist and hold the title.
function assetHolds(asset: string): boolean {
  const m = asset.match(/^(\S+)\s+"([^"]+)"/);
  if (!m) return false;
  const file = join(ROOT, m[1]!);
  return existsSync(file) && readFileSync(file, 'utf8').includes(m[2]!);
}

export function problems(f: Finding): string[] {
  const out: string[] = [];
  for (const k of FIELDS) if (!f[k]) out.push(`${f.file}: no ${k}`);
  if (f.id && `${f.id}.md` !== f.file) out.push(`${f.file}: id ${f.id} does not match the file name`);
  if (f.severity && !/^S[123]$/.test(f.severity)) out.push(`${f.file}: severity ${f.severity}`);
  if (f.class && !CLASSES.includes(f.class)) out.push(`${f.file}: class ${f.class}`);
  if (f.status && !['open', 'closed'].includes(f.status)) out.push(`${f.file}: status ${f.status}`);
  const hasAsset = !!f.regression_asset && !/^none\b/.test(f.regression_asset);
  if (hasAsset && !assetHolds(f.regression_asset!)) out.push(`${f.file}: regression asset not found: ${f.regression_asset}`);
  if (f.status === 'closed' && /^S[12]$/.test(f.severity ?? '') && !hasAsset) out.push(`${f.file}: closed ${f.severity} without a regression asset`);
  return out;
}

const records = readdirSync(DIR)
  .filter((f) => /^UXF-\d{3}\.md$/.test(f))
  .sort()
  .map((f) => parse(f, readFileSync(join(DIR, f), 'utf8')));

test('every finding record is complete, and no closed S1 or S2 finding lacks a regression asset', () => {
  expect(records.length).toBeGreaterThan(0);
  expect(records.flatMap(problems)).toEqual([]);
});

test('a closed S2 record without a regression asset is refused', () => {
  const closed = parse('UXF-999.md', '---\nid: UXF-999\ntitle: t\nclass: layout\nseverity: S2\nmetric: m\nstatus: closed\nfound_by: f\nevidence_recording: none\nevidence_screenshot: s\nevidence_lines: l\nregression_asset: none\n---\n');
  expect(problems(closed)).toEqual(['UXF-999.md: closed S2 without a regression asset']);
  expect(problems({ ...closed, regression_asset: 'e2e/gates.e2e.ts "no such test"' })).toEqual(['UXF-999.md: regression asset not found: e2e/gates.e2e.ts "no such test"']);
});

test('every known hard-gate failure names an open finding', () => {
  const open = new Set(records.filter((r) => r.status === 'open').map((r) => r.id));
  for (const k of KNOWN_FAILURES) expect(open.has(k.finding), k.finding).toBe(true);
});
