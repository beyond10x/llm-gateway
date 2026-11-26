// The text a canvas draws, read on the built page. An init script, installed before the page's
// first script, records every fillText and strokeText call of every 2D context with the text, the
// font, the box the glyphs cover (measureText's actual bounding box, through the context's
// transform) and the canvas's size; a clearRect over the whole canvas starts a new frame, so the
// record holds what the canvas shows now. canvasLabels then reads, for the token-flow canvas, each
// label's size and box in CSS pixels and samples the canvas's pixels inside that box, so a label
// that is cut, drawn past the edge or not drawn at all is seen as the eye would see it. The page is
// not changed: the FlowCanvas debug hook (`window.__flow`, with `?flowdebug`) exposes counts only,
// not the labels.
import type { Page } from '@playwright/test';

export const CANVAS_TEXT = `
  (() => {
    const frames = new WeakMap();
    const proto = CanvasRenderingContext2D.prototype;
    const record = (ctx, text, x, y) => {
      const c = ctx.canvas;
      const m = ctx.getTransform();
      const t = ctx.measureText(String(text));
      const list = frames.get(c) || [];
      list.push({
        text: String(text),
        font: ctx.font,
        corners: [[x - t.actualBoundingBoxLeft, y - t.actualBoundingBoxAscent], [x + t.actualBoundingBoxRight, y + t.actualBoundingBoxDescent]]
          .map(([px, py]) => [m.a * px + m.c * py + m.e, m.b * px + m.d * py + m.f]),
        scale: Math.hypot(m.a, m.b),
      });
      frames.set(c, list);
    };
    for (const k of ['fillText', 'strokeText']) {
      const real = proto[k];
      proto[k] = function (text, x, y, ...rest) {
        record(this, text, x, y);
        return real.call(this, text, x, y, ...rest);
      };
    }
    const clear = proto.clearRect;
    proto.clearRect = function (x, y, w, h) {
      const m = this.getTransform();
      const x0 = m.a * x + m.e;
      const y0 = m.d * y + m.f;
      if (x0 <= 0.5 && y0 <= 0.5 && x0 + m.a * w >= this.canvas.width - 1 && y0 + m.d * h >= this.canvas.height - 1) frames.set(this.canvas, []);
      return clear.call(this, x, y, w, h);
    };
    window.__uxCanvasText = (c) => frames.get(c) || [];
  })();
`;

export async function recordCanvasText(page: Page): Promise<void> {
  await page.addInitScript(CANVAS_TEXT);
}

export interface CanvasLabel {
  text: string;
  // The font size in CSS pixels.
  px: number;
  // The glyphs' box in CSS pixels, relative to the canvas.
  box: { left: number; top: number; right: number; bottom: number };
  // The box lies inside the canvas.
  inside: boolean;
  // The share of the box's pixels the canvas painted (alpha over 0).
  ink: number;
}

export interface CanvasText {
  present: boolean;
  width: number;
  height: number;
  labels: CanvasLabel[];
}

// Runs in the page: the labels the token-flow canvas shows now (the canvas under a "Token flow"
// heading, else the first canvas).
export function canvasLabels(): CanvasText {
  const canvases = [...document.querySelectorAll('canvas')];
  const underHeading = (c: Element): boolean => {
    for (let p = c.parentElement; p && p !== document.body; p = p.parentElement) {
      const h = p.querySelector('h1, h2, h3, h4, [role="heading"]');
      if (h && /token flow/i.test(h.textContent ?? '')) return true;
    }
    return false;
  };
  const canvas = canvases.find(underHeading) ?? canvases[0];
  const read = (window as unknown as { __uxCanvasText?: (c: HTMLCanvasElement) => { text: string; font: string; corners: number[][]; scale: number }[] }).__uxCanvasText;
  if (!canvas || !read) return { present: false, width: 0, height: 0, labels: [] };
  const width = canvas.clientWidth;
  const height = canvas.clientHeight;
  const toCss = canvas.width > 0 ? width / canvas.width : 1;
  const ctx = canvas.getContext('2d');
  const labels: CanvasLabel[] = [];
  for (const d of read(canvas)) {
    if (!d.text.trim()) continue;
    const size = parseFloat(d.font.match(/([\d.]+)px/)?.[1] ?? 'NaN');
    const xs = d.corners.map((p) => p[0]! * toCss);
    const ys = d.corners.map((p) => p[1]! * toCss);
    const box = { left: Math.min(...xs), top: Math.min(...ys), right: Math.max(...xs), bottom: Math.max(...ys) };
    const inside = box.left >= -0.5 && box.top >= -0.5 && box.right <= width + 0.5 && box.bottom <= height + 0.5;
    let ink = 0;
    if (ctx) {
      const x0 = Math.max(0, Math.floor(box.left / toCss));
      const y0 = Math.max(0, Math.floor(box.top / toCss));
      const x1 = Math.min(canvas.width, Math.ceil(box.right / toCss));
      const y1 = Math.min(canvas.height, Math.ceil(box.bottom / toCss));
      if (x1 > x0 && y1 > y0) {
        const px = ctx.getImageData(x0, y0, x1 - x0, y1 - y0).data;
        let painted = 0;
        for (let i = 3; i < px.length; i += 4) if (px[i]! > 0) painted += 1;
        ink = painted / ((x1 - x0) * (y1 - y0));
      }
    }
    labels.push({ text: d.text, px: Math.round(size * d.scale * toCss * 100) / 100, box, inside, ink: Math.round(ink * 1000) / 1000 });
  }
  return { present: true, width, height, labels };
}

// What UXF-014 asks of the token flow's labels, from what the canvas shows: every label at
// `minPx` or more, inside the canvas and painted, and every route the snapshot names drawn as one
// whole label.
export function canvasLabelProblems(t: CanvasText, routes: string[], minPx: number): string[] {
  const out: string[] = [];
  if (!t.present) return ['no token-flow canvas'];
  for (const l of t.labels) {
    if (!(l.px >= minPx - 1e-6)) out.push(`"${l.text}" drawn at ${l.px} px, under ${minPx} px`);
    if (!l.inside) out.push(`"${l.text}" runs past the canvas (${Math.round(l.box.left)} to ${Math.round(l.box.right)} of ${t.width} px)`);
    if (l.ink <= 0) out.push(`"${l.text}" leaves no paint in its box`);
  }
  for (const r of routes) {
    if (!t.labels.some((l) => l.text === r)) {
      const cut = t.labels.find((l) => l.text.endsWith('…') && r.startsWith(l.text.slice(0, -1)));
      out.push(cut ? `route "${r}" is cut to "${cut.text}"` : `route "${r}" is not drawn`);
    }
  }
  return out;
}
