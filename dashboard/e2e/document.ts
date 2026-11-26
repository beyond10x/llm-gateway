// What the page's ess-ui document (ui/llm-gateway.ui.yaml) declares, read by the probes: the
// labels it gives its metrics, fields and columns. A text on the page that is one of these words is
// a label, whatever class the renderer gives it; the probes use it to tell a metric's label from
// the value the metric shows (e2e/probe.ts keyDataProbe).
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { parse } from 'yaml';

const DOCUMENT = fileURLToPath(new URL('../ui/llm-gateway.ui.yaml', import.meta.url));

// Every `label` the document declares, anywhere, once each.
export function labelsOf(document: unknown): string[] {
  const out = new Set<string>();
  const walk = (v: unknown) => {
    if (Array.isArray(v)) v.forEach(walk);
    else if (v && typeof v === 'object') {
      for (const [k, x] of Object.entries(v)) {
        if (k === 'label' && typeof x === 'string' && x.trim()) out.add(x.trim());
        else walk(x);
      }
    }
  };
  walk(document);
  return [...out].sort();
}

let labels: string[] | null = null;
export function documentLabels(): string[] {
  labels ??= labelsOf(parse(readFileSync(DOCUMENT, 'utf8')));
  return labels;
}
