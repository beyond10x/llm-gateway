// axe-core, run in the page itself. The source is injected once per document and `axe.run` is
// called on it directly: @axe-core/playwright's default mode opens a blank page for every scan to
// finish the run, and injects the source again each time, which made the scans most of the
// suite's time. The page has no frames, so the in-page run sees what the default mode sees.
// Only violations are processed in full (`resultTypes`); passes and incomplete results are not used.
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import type { Page } from '@playwright/test';

const require = createRequire(import.meta.url);
const AXE_SOURCE = readFileSync(createRequire(require.resolve('@axe-core/playwright')).resolve('axe-core/axe.min.js'), 'utf8');

export interface AxeViolation {
  id: string;
  impact: string | null;
  help: string;
  nodes: { target: string[] }[];
}

export interface AxeRun {
  violations: AxeViolation[];
  version: string;
}

// One scan of the page: the rules of `tags`, or only the rules named in `rules`.
export async function runAxe(page: Page, select: { tags?: string[]; rules?: string[] }): Promise<AxeRun> {
  if (!(await page.evaluate(() => 'axe' in window))) await page.evaluate(AXE_SOURCE);
  return page.evaluate(async (sel) => {
    const axe = (window as unknown as { axe: { run(ctx: Document, opts: object): Promise<{ violations: AxeViolation[]; testEngine: { version: string } }> } }).axe;
    const runOnly = sel.rules ? { type: 'rule', values: sel.rules } : { type: 'tag', values: sel.tags ?? [] };
    const r = await axe.run(document, { runOnly, resultTypes: ['violations'] });
    return {
      violations: r.violations.map((v) => ({ id: v.id, impact: v.impact, help: v.help, nodes: v.nodes.map((n) => ({ target: n.target.map(String) })) })),
      version: r.testEngine.version,
    };
  }, select);
}
