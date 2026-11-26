// Functions that run inside the page (through page.evaluate). Each is self-contained: the browser
// gets its source text, not its closure.

export interface HeaderItem {
  name: string;
  visible_ratio: number;
}

export interface LayoutProbe {
  overflow_x_px: number;
  overflowing: string[];
  scrollers: string[];
  truncated: string[];
  header: HeaderItem[];
  key_font_min_px: number;
  key_font_at: string;
}

// Marks the always-visible elements with data-ux-always before a probe.
export const ALWAYS_ATTR = 'data-ux-always';

export function layoutProbe(): LayoutProbe {
  const describe = (el: Element): string => {
    const id = el.id ? `#${el.id}` : '';
    const cls = el.classList.length ? `.${[...el.classList].join('.')}` : '';
    const text = (el.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 40);
    return `${el.tagName.toLowerCase()}${id}${cls}${text ? ` "${text}"` : ''}`;
  };
  // A stable key for an element, without its text, so one element counts once over every state:
  // a CSS selector from the nearest ancestor with an id or aria-labelledby, each step the tag, its
  // first class, and its place among siblings of the same tag when there are several.
  const pathOf = (el: Element): string => {
    const parts: string[] = [];
    for (let e: Element | null = el; e && e !== document.body; e = e.parentElement) {
      const tag = e.tagName.toLowerCase();
      if (e.id) {
        parts.unshift(`${tag}#${CSS.escape(e.id)}`);
        break;
      }
      const labelled = e.getAttribute('aria-labelledby');
      if (labelled) {
        parts.unshift(`${tag}[aria-labelledby="${labelled}"]`);
        break;
      }
      let step = tag + (e.classList.length ? `.${CSS.escape(e.classList[0]!)}` : '');
      const sameTag = e.parentElement ? [...e.parentElement.children].filter((s) => s.tagName === e!.tagName) : [e];
      if (sameTag.length > 1) step += `:nth-of-type(${sameTag.indexOf(e) + 1})`;
      parts.unshift(step);
    }
    return parts.join(' > ');
  };
  const keyed = (el: Element, what: string): string => `${pathOf(el)} | ${what}`;
  const shown = (el: Element, cs: CSSStyleDeclaration): boolean => {
    if (cs.display === 'none' || cs.visibility === 'hidden' || cs.visibility === 'collapse') return false;
    const r = el.getBoundingClientRect();
    return r.width > 0 || r.height > 0;
  };
  const scrollsX = (cs: CSSStyleDeclaration) => cs.overflowX === 'auto' || cs.overflowX === 'scroll';
  const clipsX = (cs: CSSStyleDeclaration) => cs.overflowX === 'hidden' || cs.overflowX === 'clip';

  const root = document.documentElement;
  const vw = root.clientWidth;
  const overflow_x_px = Math.max(0, root.scrollWidth - vw);
  const overflowing: string[] = [];
  const scrollers: string[] = [];
  const truncated: string[] = [];

  const all = [...document.body.querySelectorAll('*')];
  const styles = new Map<Element, CSSStyleDeclaration>();
  for (const el of all) styles.set(el, getComputedStyle(el));
  const inScroller = (el: Element): boolean => {
    for (let p = el.parentElement; p && p !== document.body; p = p.parentElement) {
      const cs = styles.get(p);
      if (cs && scrollsX(cs)) return true;
    }
    return false;
  };

  for (const el of all) {
    const cs = styles.get(el)!;
    if (!shown(el, cs)) continue;
    if (el.closest('svg') && el.tagName.toLowerCase() !== 'svg') continue;
    const r = el.getBoundingClientRect();
    const box = el as HTMLElement;
    if (scrollsX(cs) && box.scrollWidth > box.clientWidth + 1) scrollers.push(keyed(el, describe(el)));
    const fullText = (el.textContent ?? '').trim();
    if ((cs.textOverflow === 'ellipsis' || clipsX(cs)) && fullText && (box.scrollWidth > box.clientWidth + 1 || box.scrollHeight > box.clientHeight + 1)) {
      const carried = [el.getAttribute('title'), el.getAttribute('aria-label')].some((v) => v !== null && v.includes(fullText));
      if (!carried) truncated.push(keyed(el, describe(el)));
    }
    if (inScroller(el)) continue;
    if (cs.position === 'fixed' && el.tagName.toLowerCase() === 'canvas') continue;
    if (r.right > vw + 0.5 || r.left < -0.5) {
      overflowing.push(keyed(el, `${describe(el)} past the viewport`));
      continue;
    }
    const parent = el.parentElement;
    const pcs = parent ? styles.get(parent) : undefined;
    if (parent && pcs && parent !== document.body && cs.position !== 'absolute' && cs.position !== 'fixed' && pcs.overflowX === 'visible' && cs.display !== 'inline' && cs.display !== 'contents') {
      const pr = parent.getBoundingClientRect();
      if (r.right > pr.right + 1 || r.left < pr.left - 1) {
        overflowing.push(keyed(el, `${describe(el)} past ${describe(parent)}`));
        continue;
      }
    }
    if (!clipsX(cs) && !scrollsX(cs) && cs.display !== 'inline' && !['table', 'tr', 'tbody', 'thead', 'td', 'th'].includes(el.tagName.toLowerCase())) {
      for (const n of el.childNodes) {
        if (n.nodeType !== Node.TEXT_NODE || !(n.textContent ?? '').trim()) continue;
        const range = document.createRange();
        range.selectNodeContents(n);
        const tr = range.getBoundingClientRect();
        if (tr.right > r.right + 1 || tr.left < r.left - 1) {
          overflowing.push(keyed(el, `${describe(el)} text past its box`));
          break;
        }
      }
    }
  }

  const header: HeaderItem[] = [];
  const vh = root.clientHeight;
  for (const el of document.querySelectorAll(`[${'data-ux-always'}]`)) {
    const r = el.getBoundingClientRect();
    const area = r.width * r.height;
    const ix = Math.max(0, Math.min(r.right, vw) - Math.max(r.left, 0));
    const iy = Math.max(0, Math.min(r.bottom, vh) - Math.max(r.top, 0));
    const inView = area > 0 ? (ix * iy) / area : 0;
    const points: [number, number][] = [
      [r.left + r.width / 2, r.top + r.height / 2],
      [r.left + 2, r.top + 2],
      [r.right - 2, r.top + 2],
      [r.left + 2, r.bottom - 2],
      [r.right - 2, r.bottom - 2],
    ];
    let uncovered = 0;
    for (const [x, y] of points) {
      const hit = x >= 0 && y >= 0 && x < vw && y < vh ? document.elementFromPoint(x, y) : null;
      if (hit && (hit === el || el.contains(hit))) uncovered += 1;
    }
    header.push({ name: el.getAttribute('data-ux-always') ?? describe(el), visible_ratio: Math.min(inView, uncovered / points.length) });
  }

  let key_font_min_px = Infinity;
  let key_font_at = '';
  const keyData = document.querySelectorAll('main tbody td, main tbody th, main dd, .tile .value, [role="status"]');
  for (const container of keyData) {
    for (const el of [container, ...container.querySelectorAll('*')]) {
      const cs = styles.get(el) ?? getComputedStyle(el);
      if (!shown(el, cs)) continue;
      const direct = [...el.childNodes].some((n) => n.nodeType === Node.TEXT_NODE && (n.textContent ?? '').trim());
      if (!direct) continue;
      const px = parseFloat(cs.fontSize);
      if (px < key_font_min_px) {
        key_font_min_px = px;
        key_font_at = describe(el);
      }
    }
  }
  if (!Number.isFinite(key_font_min_px)) key_font_min_px = 0;
  return { overflow_x_px, overflowing, scrollers, truncated, header, key_font_min_px, key_font_at };
}

export interface Candidate {
  index: number;
  label: string;
  scroller: boolean;
  // Radios of one group take one Tab stop; the arrow keys move inside it.
  group: string | null;
}

// Numbers every keyboard-operable control and every horizontally scrolling container.
export function markCandidates(): Candidate[] {
  const describe = (el: Element): string => {
    const name = el.getAttribute('aria-label') ?? (el.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 30);
    return `${el.tagName.toLowerCase()}${el.className && typeof el.className === 'string' ? `.${el.className.split(' ').join('.')}` : ''} "${name}"`;
  };
  const selector = 'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]:not([tabindex="-1"]), [contenteditable="true"]';
  const out: Candidate[] = [];
  let i = 0;
  for (const el of document.body.querySelectorAll('*')) {
    el.removeAttribute('data-ux-i');
    const cs = getComputedStyle(el);
    const r = el.getBoundingClientRect();
    if (cs.display === 'none' || cs.visibility !== 'visible' || (r.width === 0 && r.height === 0)) continue;
    if (el.closest('[inert], [aria-hidden="true"]')) continue;
    const control = el.matches(selector);
    const box = el as HTMLElement;
    const scroller = (cs.overflowX === 'auto' || cs.overflowX === 'scroll') && box.scrollWidth > box.clientWidth + 1;
    if (!control && !scroller) continue;
    el.setAttribute('data-ux-i', String(i));
    const radioName = el.matches("input[type=\"radio\"]") ? el.getAttribute("name") : null;
    const radioGroup = el.getAttribute("role") === "radio" ? el.closest("[role=\"radiogroup\"]") : null;
    const group = radioName ? `radio:${radioName}` : radioGroup ? `radiogroup:${(radioGroup.getAttribute("aria-label") ?? radioGroup.id) || "unnamed"}` : null;
    out.push({ index: i, label: describe(el), scroller: scroller && !control, group });
    i += 1;
  }
  return out;
}

export interface FocusStep {
  index: number | null;
  inside: number[];
  visible: boolean;
  label: string;
  // Why the focus counts as invisible; empty when visible.
  why: string;
}

// The focused element and whether its focus can be seen: an outline or a box-shadow whose colour,
// after the opacity of the element and its ancestors, has alpha > 0 and a contrast of at least
// 3:1 against the background it is drawn on (the element's own background for an inset ring,
// else its parent's). A control that is itself invisible (opacity 0, as a styled radio input)
// may show its focus on its label or the label's content instead.
export function focusStep(): FocusStep {
  const el = document.activeElement;
  if (!el || el === document.body || el === document.documentElement) return { index: null, inside: [], visible: true, label: 'body', why: '' };
  const own = el.getAttribute('data-ux-i');
  const inside: number[] = [];
  for (let p = el.parentElement; p; p = p.parentElement) {
    const v = p.getAttribute('data-ux-i');
    if (v !== null) inside.push(Number(v));
  }
  const label = `${el.tagName.toLowerCase()} "${(el.textContent ?? '').trim().slice(0, 30)}"`;

  type Rgba = [number, number, number, number];
  const norm = document.createElement('canvas').getContext('2d')!;
  const rgba = (c: string): Rgba => {
    norm.fillStyle = '#000';
    norm.fillStyle = c;
    const v = String(norm.fillStyle);
    if (v.startsWith('#')) return [parseInt(v.slice(1, 3), 16), parseInt(v.slice(3, 5), 16), parseInt(v.slice(5, 7), 16), 1];
    const p = (v.match(/[\d.]+/g) ?? []).map(Number);
    return [p[0] ?? 0, p[1] ?? 0, p[2] ?? 0, p[3] ?? 1];
  };
  const over = (top: Rgba, under: Rgba): Rgba => {
    const a = top[3];
    return [top[0] * a + under[0] * (1 - a), top[1] * a + under[1] * (1 - a), top[2] * a + under[2] * (1 - a), 1];
  };
  const opacity = (e: Element | null): number => {
    let o = 1;
    for (let p = e; p; p = p.parentElement) o *= parseFloat(getComputedStyle(p).opacity);
    return o;
  };
  // The colour behind `e`: every background from the root down to `e`, composited over white.
  const backdrop = (e: Element | null): Rgba => {
    const chain: Element[] = [];
    for (let p = e; p; p = p.parentElement) chain.unshift(p);
    let c: Rgba = [255, 255, 255, 1];
    for (const p of chain) {
      const b = rgba(getComputedStyle(p).backgroundColor);
      if (b[3] > 0) c = over(b, c);
    }
    return c;
  };
  const lum = (c: Rgba): number => {
    const ch = (v: number) => {
      const s = v / 255;
      return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
    };
    return 0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2]);
  };
  const contrast = (a: Rgba, b: Rgba): number => {
    const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x) as [number, number];
    return (hi + 0.05) / (lo + 0.05);
  };
  // Splits a computed box-shadow into its layers, each with its colour and whether it is inset.
  const shadows = (v: string): { color: string; inset: boolean; extent: number }[] => {
    if (!v || v === 'none') return [];
    const layers = v.split(/,(?![^(]*\))/);
    return layers.map((layer) => {
      const color = layer.match(/(rgba?|hsla?|color|oklch|oklab|lab|lch)\([^)]*\)|#[0-9a-f]{3,8}\b/i)?.[0] ?? 'currentcolor';
      const lengths = (layer.replace(color, '').match(/-?[\d.]+px/g) ?? []).map((x) => Math.abs(parseFloat(x)));
      return { color, inset: /\binset\b/.test(layer), extent: Math.max(0, ...lengths) };
    });
  };

  const candidates: Element[] = [];
  const self = getComputedStyle(el);
  if (opacity(el) > 0.05 && self.visibility === 'visible') candidates.push(el);
  for (const l of (el as HTMLInputElement).labels ?? []) candidates.push(l, ...l.querySelectorAll('*'));

  let best = 0;
  let seen = false;
  for (const c of candidates) {
    const cs = getComputedStyle(c);
    const rings: { color: string; inset: boolean }[] = [];
    if (cs.outlineStyle !== 'none' && parseFloat(cs.outlineWidth) > 0) rings.push({ color: cs.outlineColor, inset: parseFloat(cs.outlineOffset) < 0 });
    for (const s of shadows(cs.boxShadow)) if (s.extent > 0) rings.push({ color: s.color === 'currentcolor' ? cs.color : s.color, inset: s.inset });
    const alphaOf = opacity(c);
    for (const r of rings) {
      const col = rgba(r.color);
      const a = col[3] * alphaOf;
      if (a <= 0) continue;
      seen = true;
      const behind = backdrop(r.inset ? c : c.parentElement);
      const ratio = contrast(over([col[0], col[1], col[2], a], behind), behind);
      best = Math.max(best, ratio);
    }
  }
  const visible = best >= 3;
  const why = visible ? '' : seen ? `focus ring contrast ${best.toFixed(2)}:1` : 'no outline or box-shadow that can be seen';
  return { index: own === null ? null : Number(own), inside, visible, label, why };
}

export interface FlowProbe {
  present: boolean;
  at: string;
  // Share of the canvas's box that is inside the viewport and not under an element that paints
  // over it (a background, an image, a control), sampled on a 32 x 16 grid.
  visible_ratio: number;
  // The canvas holds pixels other than its background colour.
  drawn: boolean;
  drawn_share: number;
}

// The token-flow canvas: the one under a "Token flow" heading, else the one with class `flow`,
// else the first canvas.
export function flowProbe(): FlowProbe {
  const canvases = [...document.querySelectorAll('canvas')];
  const underHeading = (c: Element): boolean => {
    for (let p = c.parentElement; p && p !== document.body; p = p.parentElement) {
      const h = p.querySelector('h1, h2, h3, h4, [role="heading"]');
      if (h && /token flow/i.test(h.textContent ?? '')) return true;
    }
    return false;
  };
  const canvas = canvases.find(underHeading) ?? canvases.find((c) => c.classList.contains('flow')) ?? canvases[0];
  if (!canvas) return { present: false, at: '', visible_ratio: 0, drawn: false, drawn_share: 0 };
  const at = `canvas${canvas.className ? `.${String(canvas.className).split(' ').join('.')}` : ''}`;

  let hidden = false;
  let opacity = 1;
  for (let p: Element | null = canvas; p; p = p.parentElement) {
    const cs = getComputedStyle(p);
    opacity *= parseFloat(cs.opacity);
    if (cs.display === 'none') hidden = true;
  }
  if (getComputedStyle(canvas).visibility !== 'visible' || opacity <= 0.05) hidden = true;

  const paints = (e: Element): boolean => {
    const cs = getComputedStyle(e);
    const bg = cs.backgroundColor.match(/[\d.]+/g)?.map(Number) ?? [];
    const alpha = bg.length >= 4 ? bg[3]! : bg.length === 3 ? 1 : 0;
    if (alpha > 0.5 || cs.backgroundImage !== 'none') return true;
    return ['img', 'svg', 'video', 'canvas', 'input', 'button', 'select', 'textarea', 'iframe'].includes(e.tagName.toLowerCase());
  };

  const r = canvas.getBoundingClientRect();
  const vw = document.documentElement.clientWidth;
  const vh = document.documentElement.clientHeight;
  const cols = 32;
  const rows = 16;
  let seen = 0;
  const before = canvas.style.pointerEvents;
  canvas.style.pointerEvents = 'auto';
  try {
    for (let i = 0; i < cols && !hidden; i++) {
      for (let j = 0; j < rows; j++) {
        const x = r.left + ((i + 0.5) * r.width) / cols;
        const y = r.top + ((j + 0.5) * r.height) / rows;
        if (x < 0 || y < 0 || x >= vw || y >= vh) continue;
        const stack = document.elementsFromPoint(x, y);
        const k = stack.indexOf(canvas);
        if (k < 0) continue;
        if (!stack.slice(0, k).some(paints)) seen += 1;
      }
    }
  } finally {
    canvas.style.pointerEvents = before;
  }
  const visible_ratio = r.width > 0 && r.height > 0 ? seen / (cols * rows) : 0;

  let drawn_share = 0;
  try {
    const w = canvas.width;
    const h = canvas.height;
    if (w > 0 && h > 0) {
      const copy = document.createElement('canvas');
      copy.width = w;
      copy.height = h;
      const c2 = copy.getContext('2d')!;
      c2.drawImage(canvas, 0, 0);
      const px = c2.getImageData(0, 0, w, h).data;
      // The background is the most common colour of a coarse sample.
      const counts = new Map<number, number>();
      for (let p = 0; p < px.length; p += 4 * 97) {
        const key = ((px[p]! << 24) | (px[p + 1]! << 16) | (px[p + 2]! << 8) | px[p + 3]!) >>> 0;
        counts.set(key, (counts.get(key) ?? 0) + 1);
      }
      const bgKey = [...counts.entries()].sort((a, b) => b[1] - a[1])[0]?.[0] ?? 0;
      let differ = 0;
      let total = 0;
      for (let p = 0; p < px.length; p += 8) {
        total += 1;
        const key = ((px[p]! << 24) | (px[p + 1]! << 16) | (px[p + 2]! << 8) | px[p + 3]!) >>> 0;
        if (key !== bgKey) differ += 1;
      }
      drawn_share = total ? differ / total : 0;
    }
  } catch {
    drawn_share = 0;
  }
  return { present: true, at, visible_ratio: Math.round(visible_ratio * 1000) / 1000, drawn: drawn_share >= 0.0005, drawn_share: Math.round(drawn_share * 10000) / 10000 };
}

export interface MotionProbe {
  running: string[];
}

export function motionProbe(): MotionProbe {
  const running: string[] = [];
  for (const a of document.getAnimations()) {
    const t = a.effect?.getComputedTiming();
    const duration = typeof t?.duration === 'number' ? t.duration : 0;
    if (a.playState === 'running' && duration > 0) {
      const target = (a.effect as KeyframeEffect | null)?.target;
      const name = a instanceof CSSTransition ? `transition ${a.transitionProperty}` : a instanceof CSSAnimation ? `animation ${a.animationName}` : 'script animation';
      running.push(`${name} on ${target ? target.tagName.toLowerCase() : 'unknown'} (${Math.round(duration)} ms)`);
    }
  }
  return { running };
}

export function canvasPixels(): string[] {
  return [...document.querySelectorAll('canvas')].map((c) => {
    try {
      return c.toDataURL();
    } catch {
      return 'unreadable';
    }
  });
}
