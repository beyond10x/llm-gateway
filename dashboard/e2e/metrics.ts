// The metric keys and the pure arithmetic of a measurement, kept apart from the browser code so
// the unit tests can read them.
import type { FlowProbe } from './probe.ts';
import { HEADING_FONT_PX, KEY_FONT_PX, SECONDARY_FONT_PX } from './profiles.ts';

// Lower is better for every one of these.
export interface Metrics {
  'axe.serious_critical': number;
  'overflow.x_px': number;
  'overflow.elements': number;
  'overflow.scrollers': number;
  'overflow.scrollers_unreachable': number;
  'header.hidden_ratio': number;
  'keyboard.unreachable': number;
  'keyboard.focus_invisible': number;
  'key_font.deficit_px': number;
  'heading_font.deficit_px': number;
  'secondary_font.deficit_px': number;
  'layout_shift.per_state_max': number;
  'reduced_motion.violations': number;
  'truncation.without_full_value': number;
  // Distinct items of the visible-text probe (e2e/copy.ts) over every state.
  'copy.defects': number;
}

export const METRIC_KEYS: (keyof Metrics)[] = [
  'axe.serious_critical',
  'overflow.x_px',
  'overflow.elements',
  'overflow.scrollers',
  'overflow.scrollers_unreachable',
  'header.hidden_ratio',
  'keyboard.unreachable',
  'keyboard.focus_invisible',
  'key_font.deficit_px',
  'heading_font.deficit_px',
  'secondary_font.deficit_px',
  'layout_shift.per_state_max',
  'reduced_motion.violations',
  'truncation.without_full_value',
  'copy.defects',
];

// The token-flow keys, higher is better: the ratchet takes their smallest value over the replays.
// `flow.visible_ratio` is the share of the canvas in view and uncovered at the last state;
// `flow.drawn` is 1 when the canvas holds a drawing at the last state of the replay and of its
// reduced-motion replay, else 0.
export const FLOW_KEYS = ['flow.visible_ratio', 'flow.drawn'] as const;
export type FlowKey = (typeof FLOW_KEYS)[number];
export const HIGHER_IS_BETTER: ReadonlySet<string> = new Set([...FLOW_KEYS, 'screen_only.reached']);

// The screen-only user's keys (e2e/screen-only.ts), only for a scenario with tasks: the actions
// summed over its tasks, the worst glance (1 to 3), and 1 when every task reached the truth, else 0
// (higher is better).
export const SCREEN_ONLY_KEYS = ['screen_only.actions', 'screen_only.looks', 'screen_only.reached'] as const;

export interface ScreenOnlyResult {
  task: string;
  reached: boolean;
  actions: number;
  looks: number;
}

export function screenOnlyMetrics(results: ScreenOnlyResult[]): Record<string, number> {
  if (results.length === 0) return {};
  return {
    'screen_only.actions': results.reduce((a, r) => a + r.actions, 0),
    'screen_only.looks': Math.max(...results.map((r) => r.looks)),
    'screen_only.reached': results.every((r) => r.reached) ? 1 : 0,
  };
}

// What gate 4 refuses: a task not reached.
export function screenOnlyProblems(results: ScreenOnlyResult[]): string[] {
  return results.filter((r) => !r.reached).map((r) => `${r.task}: the truth was not reached in ${r.actions} actions`);
}

// The overflow.scrollers count at a measured-only viewport, ratcheted but not gated.
export const MEASURED_ONLY_SCROLLERS = (viewport: string) => `overflow.scrollers.${viewport}`;

// What the replay check refuses about the connection status: before a lose-connection action every
// status reads "live"; after it, the status names a state that is not live, in words (one of
// `words`, regular expressions).
export function statusProblems(status: string[], states: number, actions: readonly string[], words: string[] = []): string[] {
  const lostAt = actions.indexOf('lose-connection');
  const cut = lostAt < 0 ? Infinity : states + lostAt;
  const out: string[] = [];
  status.forEach((s, i) => {
    if (i < cut && s !== 'live') out.push(`status "${s}" at step ${i + 1}, not live`);
    if (i >= cut && (s === 'live' || !words.some((w) => new RegExp(w, 'i').test(s)))) out.push(`status "${s}" at step ${i + 1}, after the connection was lost`);
  });
  if (lostAt >= 0 && status.length <= cut) out.push('no status after the connection was lost');
  return out;
}

// The snapshot fetches a replay makes: the first one, and one failed poll after a lost connection.
export function expectedFetches(actions: readonly string[]): number {
  return actions.includes('lose-connection') ? 2 : 1;
}

// The bar of the flow gate.
export const FLOW_VISIBLE_MIN = 0.95;

export function flowMetrics(m: { flow: FlowProbe; reduced_flow: FlowProbe }): Record<FlowKey, number> {
  return { 'flow.visible_ratio': m.flow.visible_ratio, 'flow.drawn': m.flow.drawn && m.reduced_flow.drawn ? 1 : 0 };
}

// What the flow gate refuses in one replay of a scenario that shows active sessions: no canvas, a
// blank canvas (also under reduced motion), or less than FLOW_VISIBLE_MIN of it in view.
export function flowProblems(m: { flow: FlowProbe; reduced_flow: FlowProbe }): string[] {
  const out: string[] = [];
  if (!m.flow.present) out.push('no token-flow canvas');
  if (!m.flow.drawn) out.push(`the canvas is blank (share drawn ${m.flow.drawn_share})`);
  if (!m.reduced_flow.drawn) out.push(`the canvas is blank under reduced motion (share drawn ${m.reduced_flow.drawn_share})`);
  if (m.flow.visible_ratio < FLOW_VISIBLE_MIN) out.push(`${m.flow.visible_ratio} of the canvas in view and uncovered, under ${FLOW_VISIBLE_MIN}`);
  return out;
}

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

// A font deficit: the floor less the smallest font, 0 when the page shows no such text (nothing
// there is too small).
export function fontDeficit(minPx: number | null, floor: number): number {
  if (minPx === null || !Number.isFinite(minPx) || minPx <= 0) return 0;
  return Math.round(Math.max(0, floor - minPx) * 1000) / 1000;
}

export function keyFontDeficit(minPx: number | null): number {
  return fontDeficit(minPx, KEY_FONT_PX);
}

export function secondaryFontDeficit(minPx: number | null): number {
  return fontDeficit(minPx, SECONDARY_FONT_PX);
}

export function headingFontDeficit(minPx: number | null): number {
  return fontDeficit(minPx, HEADING_FONT_PX);
}

// The key-data gate's items of one replay: every element below its floor, and "no key text" when
// the probe found no key data at all (a renamed class, an empty page): a gate that read nothing
// must not pass.
export function keyDataItems(m: { key_font_min_px: number | null; details: { key_font_below: string[]; heading_font_below?: string[]; secondary_font_below: string[] } }): string[] {
  return [
    ...(m.key_font_min_px === null ? ['no key text'] : []),
    ...m.details.key_font_below.map((k) => `key ${k}`),
    ...(m.details.heading_font_below ?? []).map((k) => `heading ${k}`),
    ...m.details.secondary_font_below.map((k) => `secondary ${k}`),
  ];
}

export function fmtKeyFont(minPx: number | null): string {
  return minPx === null ? 'n/a' : String(minPx);
}
