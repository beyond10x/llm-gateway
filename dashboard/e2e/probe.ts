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
  // A stable key for an element, without its text, so one element counts once over every state.
  const pathOf = (el: Element): string => {
    const parts: string[] = [];
    for (let e: Element | null = el; e && e !== document.body && parts.length < 6; e = e.parentElement) {
      const nth = e.parentElement ? [...e.parentElement.children].indexOf(e) + 1 : 1;
      parts.unshift(`${e.tagName.toLowerCase()}${e.classList.length ? "." + [...e.classList].join(".") : ""}:nth-child(${nth})`);
    }
    return parts.join(" > ");
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
}

export function focusStep(): FocusStep {
  const el = document.activeElement;
  if (!el || el === document.body || el === document.documentElement) return { index: null, inside: [], visible: true, label: 'body' };
  const own = el.getAttribute('data-ux-i');
  const inside: number[] = [];
  for (let p = el.parentElement; p; p = p.parentElement) {
    const v = p.getAttribute('data-ux-i');
    if (v !== null) inside.push(Number(v));
  }
  const cs = getComputedStyle(el);
  const outline = cs.outlineStyle !== 'none' && parseFloat(cs.outlineWidth) > 0;
  const shadow = cs.boxShadow !== 'none' && cs.boxShadow !== '';
  const label = `${el.tagName.toLowerCase()} "${(el.textContent ?? '').trim().slice(0, 30)}"`;
  return { index: own === null ? null : Number(own), inside, visible: outline || shadow, label };
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
