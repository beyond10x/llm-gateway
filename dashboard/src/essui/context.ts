// What every component of the renderer reads: the document, the page drawn, the source of rows,
// the shell state and the host components, and the helpers that turn a field's value into the
// words and tone it is drawn with.
import { inject, type Component, type InjectionKey } from 'vue';
import { DASH, count, pct, span } from '../format.ts';
import { value as evalExpr, type Scope } from './expr.ts';
import type { Source } from './source.ts';
import type { ShellState } from './state.ts';
import { isRich, type Action, type Document, type Field, type Page, type Reads, type Row, type Tone, type ToneMap, type Value } from './types.ts';

export interface Ctx {
  doc: Document;
  page: Page;
  source: Source;
  shell: ShellState;
  hosts: Record<string, Component>;
}

export const CTX: InjectionKey<Ctx> = Symbol('ess-ui');

export function useCtx(): Ctx {
  const ctx = inject(CTX);
  if (!ctx) throw new Error('an ess-ui component is drawn outside EssApp');
  return ctx;
}

export function scopeOf(ctx: Ctx, row?: unknown): Scope {
  return {
    row,
    shell: ctx.shell.values,
    channel: (name) => ({ lifecycle: ctx.source.lifecycle(name) }),
    section: (name) => {
      const s = ctx.page.sections.find((x) => x.name === name);
      return s ? readRows(ctx, (s as { reads?: Reads }).reads, { shell: ctx.shell.values }) : null;
    },
  };
}

export function readRows(ctx: Ctx, reads: Reads | undefined, scope: Scope): Row[] | null {
  if (!reads) return null;
  const params: Record<string, unknown> = {};
  for (const [k, src] of Object.entries(reads.params ?? {})) params[k] = evalExpr(src, scope);
  return ctx.source.rows(reads, params);
}

// The words a value is shown in: a Rich value's text, a number by the field's `as` (a duration in
// milliseconds, in hours, minutes and seconds), a value its label map names by the label.
export function wordsOf(v: Value | undefined, as: Field['as'] | undefined, labels?: Record<string, string>): string {
  if (v === null || v === undefined) return DASH;
  if (isRich(v)) return labels?.[v.text] ?? v.text;
  if (typeof v === 'number') return as === 'percent' ? pct(v) : as === 'duration' ? span(v) : count(v);
  if (typeof v === 'boolean') return labels?.[String(v)] ?? (v ? 'yes' : 'no');
  if (typeof v === 'string') return labels?.[v] ?? v;
  return DASH;
}

// The raw value a label map gives words to, shown on hover beside those words.
export function rawOf(v: Value | undefined, labels: Record<string, string> | undefined): string | undefined {
  if (!labels || v === null || v === undefined) return undefined;
  const key = isRich(v) ? v.text : typeof v === 'string' || typeof v === 'number' || typeof v === 'boolean' ? String(v) : undefined;
  return key !== undefined && Object.hasOwn(labels, key) ? key : undefined;
}

export function labelsOf(ctx: Ctx, labels: Field['labels']): Record<string, string> | undefined {
  if (labels === undefined) return undefined;
  return typeof labels === 'string' ? ctx.doc.label_maps?.[labels] : labels;
}

function mapTone(map: ToneMap | undefined, key: unknown): Tone | undefined {
  if (!map) return undefined;
  if ('bands' in map && Array.isArray(map.bands)) {
    if (typeof key !== 'number') return undefined;
    let tone: Tone | undefined;
    for (const b of map.bands as { from: number; tone: Tone }[]) if (key >= b.from) tone = b.tone;
    return tone;
  }
  return (map as Record<string, Tone>)[String(key)];
}

// The tone a field draws its value in: the tone map's tone for the value (a Rich value's key, when
// it has one), else the field's own `tone`, else neutral when the field is toned by a map at all.
export function toneOf(ctx: Ctx, field: Pick<Field, 'tone' | 'tone_by'>, v: Value | undefined): Tone | undefined {
  if (field.tone_by === undefined) return field.tone;
  const key = isRich(v) ? (v.key ?? v.text) : v;
  return mapTone(ctx.doc.tone_maps?.[field.tone_by], key) ?? field.tone ?? 'neutral';
}

// The classes of a value drawn in a tone outside a badge: none for neutral, which is the text's own
// colour.
export function toneClass(tone: Tone | undefined): string[] {
  return tone && tone !== 'neutral' ? ['toned', `tone-${tone}`] : [];
}

export const NUMERIC: Field['as'][] = ['number', 'duration', 'percent'];

// A count in words after the field's label: 2 under "Attempts" is "2 attempts", 1 is "1 attempt".
export function counted(n: number, field: Pick<Field, 'label' | 'field'>): string {
  const noun = (field.label ?? field.field).toLowerCase();
  return `${n} ${n === 1 ? noun.replace(/s$/, '') : noun}`;
}

// The words of a primitive's `text`: an expression's value, or the string itself when it is no
// expression (ess-ui/1 reads a string that does not parse as literal text).
export function textOf(ctx: Ctx, src: string, row?: unknown): string {
  try {
    const v = evalExpr(src, scopeOf(ctx, row));
    return v === null || v === undefined ? DASH : String(v);
  } catch {
    return src;
  }
}

// The address of a page: the home page at the root, any other under its name.
export function routeOf(ctx: Ctx, page: string): string {
  return ctx.doc.navigation.home === page ? '/' : `/${page}`;
}

// Runs an action: `sets` writes shell state (`toggle` flips it), `navigate` opens the page.
export function run(ctx: Ctx, action: Action, row?: unknown): void {
  if (action.navigate) {
    location.assign(routeOf(ctx, action.navigate.to));
    return;
  }
  for (const [target, src] of Object.entries(action.sets ?? {})) {
    const m = /^shell\.([A-Za-z0-9_-]+)$/.exec(target);
    if (!m) throw new Error(`an action sets shell state only: ${target}`);
    const next = src === 'toggle' ? !ctx.shell.values[m[1]!] : evalExpr(src, scopeOf(ctx, row));
    ctx.shell.set(m[1]!, next);
  }
}
