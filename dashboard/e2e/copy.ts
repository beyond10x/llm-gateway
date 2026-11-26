// The visible-text probe (the copy gate): reads the text the page shows, one run per text node,
// and reports what a reader cannot read as words. Runs inside the page (through page.evaluate),
// so it is self-contained: the browser gets its source text, not its closure.
//
// Kinds, each an item `<kind> <place>` (stable over states, rows, viewports and themes) and a label
// with the text it saw:
// - `run-together`: two runs in different elements meet with no whitespace, no line or cell break
//   and no generated content between them, where the first ends in a letter, a digit or a dash and
//   the second starts with a letter or a digit ("tok/s" + "last 60 s", "claude-code" + "3f9a1c0e");
// - `hex-title`: a run of nothing but 8 or more hex digits in a heading or card title;
// - `header-name`: a raw `anthropic-ratelimit-…` header name outside a raw-value cell;
// - `snake-label`: a snake_case token in a label (a heading, a term, a column head, a control, a
//   tile label); `snake-value`: one anywhere else outside a raw-value cell;
// - `glyph-control`: a button or link whose visible text holds no word of two letters ("▸ 2").
// A raw-value cell is code, preformatted text or a `.mono` cell or definition (`td.mono`,
// `dd.mono`) that is not itself a label; a title attribute is never read.
export function copyProbe(): string[] {
  const RAW = 'code, pre, kbd, samp, td.mono, dd.mono, [data-raw]';
  const LABEL = 'h1, h2, h3, h4, h5, h6, [role="heading"], th, dt, caption, legend, label, button, summary, [role="button"], .label';
  const HEADING = 'h1, h2, h3, h4, h5, h6, [role="heading"], caption, legend';
  const CONTROL = 'button, [role="button"], summary, a[href]';
  const SKIP = 'script, style, noscript, template, canvas, svg';
  const INLINE = new Set(['inline', 'inline-block', 'inline-flex', 'inline-grid', 'contents']);

  const styles = new Map<Element, CSSStyleDeclaration>();
  const style = (el: Element) => {
    let cs = styles.get(el);
    if (!cs) styles.set(el, (cs = getComputedStyle(el)));
    return cs;
  };
  const shape = (el: Element) => `${el.tagName.toLowerCase()}${el.classList.length ? `.${el.classList[0]}` : ''}`;
  // Where an element sits, without row or column positions: the section it is in (by the id of
  // its heading), the cell, term, definition, heading, tile or control holding it, and its shape.
  const place = (el: Element): string => {
    const section = el.closest('[aria-labelledby]')?.getAttribute('aria-labelledby') ?? el.closest('[id]')?.id ?? 'page';
    const holder = el.closest('td, th, dt, dd, li, h1, h2, h3, h4, h5, h6, button, a, summary, .tile, header');
    // A cell by its column class, so every row of a column is one place.
    const cell = (e: Element) => {
      const col = [...e.classList].find((k) => k.startsWith('col-'));
      return col ? `${e.tagName.toLowerCase()}.${col}` : shape(e);
    };
    const parts = [`#${section}`];
    if (holder && holder !== el) parts.push(cell(holder));
    parts.push(cell(el));
    return parts.join(' ');
  };
  const hidden = new Map<Element, boolean>();
  const isHidden = (el: Element): boolean => {
    const known = hidden.get(el);
    if (known !== undefined) return known;
    let out = false;
    const cs = style(el);
    if (el.closest(SKIP) || cs.display === 'none' || cs.visibility !== 'visible' || parseFloat(cs.opacity) === 0) out = true;
    else if (cs.clip && cs.clip !== 'auto') out = true;
    else if (/inset\(50%\)/.test(cs.clipPath)) out = true;
    else {
      const r = el.getBoundingClientRect();
      if ((cs.overflow === 'hidden' || cs.overflow === 'clip') && (r.width <= 2 || r.height <= 2)) out = true;
      else if (el.parentElement && el.parentElement !== document.body) out = isHidden(el.parentElement);
    }
    hidden.set(el, out);
    return out;
  };

  // A run: the text nodes of one element with nothing but <wbr>, comments and empty text between
  // them ("five_<wbr>hour" is one run).
  interface Run {
    first: Text;
    last: Text;
    el: Element;
    text: string;
  }
  const runs: Run[] = [];
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    const node = n as Text;
    const text = node.data;
    if (!text.trim()) continue;
    const el = node.parentElement;
    if (!el || isHidden(el)) continue;
    const range = document.createRange();
    range.selectNodeContents(node);
    const r = range.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) continue;
    const prev = runs[runs.length - 1];
    let gap = node.previousSibling;
    while (gap && gap !== prev?.last && (gap.nodeName === 'WBR' || gap.nodeType === Node.COMMENT_NODE || (gap.nodeType === Node.TEXT_NODE && !gap.textContent))) gap = gap.previousSibling;
    if (prev && prev.el === el && gap === prev.last) {
      prev.last = node;
      prev.text += text;
      continue;
    }
    runs.push({ first: node, last: node, el, text });
  }

  const items: string[] = [];
  const add = (kind: string, el: Element, label: string) => items.push(`${kind} ${place(el)} | ${kind} "${label}"`);
  const raw = (el: Element) => el.closest(RAW) !== null && el.closest(LABEL) === null;

  // A break between two runs: an element on either path to their common ancestor that is not
  // inline (a block, a flex or grid item, a cell), a <br>, generated content, or whitespace.
  const pathUp = (from: Element, to: Node): Element[] => {
    const out: Element[] = [];
    for (let e: Element | null = from; e && e !== to; e = e.parentElement) out.push(e);
    return out;
  };
  const pseudo = (el: Element, which: '::before' | '::after') => {
    const c = getComputedStyle(el, which).content;
    return c !== 'none' && c !== 'normal' && c !== '""' && c !== "''";
  };
  const broken = (a: Run, b: Run): boolean => {
    if (/\s$/.test(a.text) || /^\s/.test(b.text)) return true;
    const range = document.createRange();
    range.setStart(a.last, a.last.data.length);
    range.setEnd(b.first, 0);
    const common = range.commonAncestorContainer;
    const up = pathUp(a.el, common);
    const down = pathUp(b.el, common);
    if ([...up, ...down].some((e) => !INLINE.has(style(e).display))) return true;
    const between = range.toString();
    if (/\s/.test(between)) return true;
    if (range.cloneContents().querySelector('br')) return true;
    if (up.some((e) => pseudo(e, '::after')) || down.some((e) => pseudo(e, '::before'))) return true;
    return false;
  };

  for (let i = 0; i < runs.length; i++) {
    const run = runs[i]!;
    const t = run.text.trim();
    const el = run.el;
    if (/^[0-9a-f]{8,}$/i.test(t) && /\d/.test(t) && /[a-f]/i.test(t) && el.closest(HEADING)) add('hex-title', el, t);
    if (/anthropic-ratelimit-/i.test(t) && !raw(el)) add('header-name', el, t.slice(0, 60));
    const snake = t.match(/(?<![\w-])[a-z][a-z0-9]*(?:_[a-z0-9]+)+(?![\w-])/);
    if (snake && !raw(el)) add(el.closest(LABEL) ? 'snake-label' : 'snake-value', el, snake[0]);
    const next = runs[i + 1];
    if (next && next.el !== el) {
      const tail = run.text.trimEnd();
      const head = next.text.trimStart();
      if (/[\p{L}\p{N}‐-―-]$/u.test(tail) && /^[\p{L}\p{N}]/u.test(head) && !broken(run, next)) {
        add('run-together', el, `${tail.slice(-16)}|${head.slice(0, 16)}`);
      }
    }
  }
  for (const control of document.querySelectorAll(CONTROL)) {
    if (isHidden(control)) continue;
    const text = ((control as HTMLElement).innerText ?? '').trim();
    if (text && !/\p{L}{2}/u.test(text)) add('glyph-control', control, text);
  }
  return items;
}
