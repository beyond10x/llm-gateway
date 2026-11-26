// Pure parts of the round report (e2e/round-report.e2e.ts), apart so the unit tests can feed them.
import type { Measurement } from './measure.ts';
import { fmtKeyFont, METRIC_KEYS } from './metrics.ts';

export const fmt = (v: number | undefined) => (v === undefined || !Number.isFinite(v) ? '–' : Number.isInteger(v) ? String(v) : v.toFixed(3));

// Where the worst replay shows one metric: its elements, its font and where it was found, its
// shifts. Every metric names a place, so a worst moment never reads "–".
export function worstSample(k: keyof Measurement['metrics'], worst: Measurement): string[] {
  const d = worst.details;
  switch (k) {
    case 'axe.serious_critical':
      return d.axe.map((a) => `${a.id}: ${a.targets.join('; ')}`);
    case 'overflow.x_px':
      return [`the document is ${worst.metrics[k]} px wider than the viewport`, ...d.overflowing.filter((o) => / past the viewport$/.test(o.label)).map((o) => o.label)];
    case 'overflow.elements':
      return d.overflowing.map((o) => o.label);
    case 'overflow.scrollers':
      return d.scrollers.map((x) => x.label);
    case 'overflow.scrollers_unreachable':
      return d.scrollers_unreachable;
    case 'header.hidden_ratio':
      return d.header.filter((h) => h.visible_ratio < 1).map((h) => `${h.name} visible ${h.visible_ratio}`);
    case 'keyboard.unreachable':
      return d.unreachable;
    case 'keyboard.focus_invisible':
      return d.focus_invisible;
    case 'reduced_motion.violations':
      return d.reduced_motion;
    case 'truncation.without_full_value':
      return d.truncated.map((t) => t.label);
    case 'key_font.deficit_px':
      return [`${fmtKeyFont(worst.key_font_min_px)} px at ${worst.key_font_at}`, ...d.key_font_below];
    case 'heading_font.deficit_px':
      return [`${fmtKeyFont(worst.heading_font_min_px)} px at ${worst.heading_font_at}`, ...d.heading_font_below];
    case 'secondary_font.deficit_px':
      return [`${fmtKeyFont(worst.secondary_font_min_px)} px at ${worst.secondary_font_at}`, ...d.secondary_font_below];
    case 'layout_shift.per_state_max':
      return [`per state: ${worst.shifts.join(', ')}`];
    case 'copy.defects':
      return d.copy.map((c) => c.label);
  }
}

// One line per metric above 0: its worst value, the replay it was worst in, and where.
export function worstMoments(ms: Measurement[]): string[] {
  const out: string[] = [];
  for (const k of METRIC_KEYS) {
    const worst = [...ms].sort((a, b) => b.metrics[k] - a.metrics[k])[0];
    if (!worst || worst.metrics[k] <= 0) continue;
    out.push(`- \`${k}\` = ${fmt(worst.metrics[k])} at ${worst.scenario} ${worst.viewport.name} ${worst.theme}: ${worstSample(k, worst).slice(0, 3).join(' / ') || '–'}`);
  }
  return out.length ? out : ['- none above 0'];
}
