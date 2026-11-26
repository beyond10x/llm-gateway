// The metric keys and the pure arithmetic of a measurement, kept apart from the browser code so
// the unit tests can read them.
import { KEY_FONT_PX } from './profiles.ts';

export interface Metrics {
  'axe.serious_critical': number;
  'overflow.x_px': number;
  'overflow.elements': number;
  'overflow.scrollers_unreachable': number;
  'header.hidden_ratio': number;
  'keyboard.unreachable': number;
  'keyboard.focus_invisible': number;
  'key_font.deficit_px': number;
  'layout_shift.per_state_max': number;
  'reduced_motion.violations': number;
  'truncation.without_full_value': number;
}

export const METRIC_KEYS: (keyof Metrics)[] = [
  'axe.serious_critical',
  'overflow.x_px',
  'overflow.elements',
  'overflow.scrollers_unreachable',
  'header.hidden_ratio',
  'keyboard.unreachable',
  'keyboard.focus_invisible',
  'key_font.deficit_px',
  'layout_shift.per_state_max',
  'reduced_motion.violations',
  'truncation.without_full_value',
];

// The ratchet's per-rule axe keys: `axe.rule.<rule id>`, the number of distinct nodes.
export const AXE_RULE_PREFIX = 'axe.rule.';

export interface AxeFinding {
  id: string;
  impact: string;
  help: string;
  nodes: number;
  targets: string[];
}

// Adds one axe scan to the union: per rule, every distinct node target over every scan.
export function mergeAxe(into: Map<string, AxeFinding>, scan: AxeFinding[]): void {
  for (const f of scan) {
    const had = into.get(f.id);
    const targets = [...new Set([...(had?.targets ?? []), ...f.targets])].sort();
    into.set(f.id, { id: f.id, impact: had?.impact ?? f.impact, help: had?.help ?? f.help, nodes: targets.length, targets });
  }
}

// One known-failure item per node: the rule id and the node's target.
export function axeItems(list: AxeFinding[]): string[] {
  return list.flatMap((f) => f.targets.map((t) => `${f.id} ${t}`));
}

export function axeRules(list: AxeFinding[]): Record<string, number> {
  return Object.fromEntries(list.map((f) => [f.id, f.nodes]));
}

// The key-font deficit: KEY_FONT_PX less the smallest key-data font, 0 when the page shows no
// key data (nothing there is too small).
export function keyFontDeficit(minPx: number | null): number {
  if (minPx === null || !Number.isFinite(minPx) || minPx <= 0) return 0;
  return Math.round(Math.max(0, KEY_FONT_PX - minPx) * 1000) / 1000;
}

export function fmtKeyFont(minPx: number | null): string {
  return minPx === null ? 'n/a' : String(minPx);
}
