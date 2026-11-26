// The visible-text probe (the copy gate): reads the text the page shows, one run per text node,
// and reports what a reader cannot read as words. Runs inside the page (through page.evaluate),
// so it is self-contained: the browser gets its source text, not its closure.
//
// Kinds, each an item `<kind> <place>` (stable over states, rows, viewports and themes) and a label
// with the text it saw:
// - `run-together`: two consecutive runs that the eye reads as one word: the box of the first
//   run's last character and the box of the second run's first character sit on one line and
//   touch, with a gap under SPACE_EM of the first run's font size (a space is wider), where the
//   first ends in a letter, a digit or a dash and the second starts with a letter or a digit, or
//   the first ends in `%`, `)` or `]` and the second starts with a letter ("tok/s" + "last 60 s"
//   in a flex row without a gap, "claude-code" + "3f9a1c0e", "12%" + "errors"). It is the
//   rendered geometry that counts, not the markup: a margin, padding, generated content, a cell
//   or a line break between the runs sets them apart, whitespace in the DOM that renders as
//   nothing does not. A number and its unit ("12" + "ms") and a superscript or subscript
//   ("x" + "²" in `sup`) are allowed;
// - `hex-title`: a run of nothing but 8 or more hex digits in a heading or card title;
// - `header-name`: a raw `anthropic-ratelimit-…` header name outside a raw value;
// - `snake-label`: a snake_case token in a label (a heading, a term, a column head, a control, a
//   tile label); `snake-value`: one anywhere else outside a raw value;
// - `glyph-control`: a button or link whose visible text holds no word of two letters ("▸ 2").
// A raw value is code, preformatted text or sample output (`code`, `pre`, `kbd`, `samp`,
// `[data-raw]`), or a table body cell or definition whose value the document marks raw, which the
// page sets in a monospace face; never inside a label. A title attribute is never read.
export function copyProbe(): string[] {
  const CODE = 'code, pre, kbd, samp, [data-raw]';
  const CELL = 'tbody td, tbody th, [role="cell"], [role="gridcell"], [role="rowheader"], dd, [role="definition"]';
  const LABEL = 'h1, h2, h3, h4, h5, h6, [role="heading"], thead th, [role="columnheader"], dt, caption, legend, label, button, summary, [role="button"], .label';
  const HEADING = 'h1, h2, h3, h4, h5, h6, [role="heading"], caption, legend';
  const CONTROL = 'button, [role="button"], summary, a[href]';
  const SKIP = 'script, style, noscript, template, canvas, svg';
  const SPACE_EM = 0.2;
  const UNIT = /^(?:[kMGT]i?B|[kMGTµnm]?s|min|h|d|px|em|rem|tok(?:\/s)?|req(?:\/s)?|\/s|[kMGT]|x)(?![\p{L}\p{N}])/u;

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
  const mono = (el: Element) => /\bmono(?:space)?\b/i.test(style(el).fontFamily);
  const raw = (el: Element) => el.closest(LABEL) === null && (el.closest(CODE) !== null || (el.closest(CELL) !== null && mono(el)));

  // The box of one character of a text node, the last box when it spans a line break.
  const charBox = (node: Text, i: number): DOMRect | null => {
    if (i < 0 || i >= node.data.length) return null;
    const range = document.createRange();
    const end = i + 1 < node.data.length && /[\uDC00-\uDFFF]/.test(node.data[i + 1]!) ? i + 2 : i + 1;
    range.setStart(node, i);
    range.setEnd(node, end);
    const boxes = range.getClientRects();
    return boxes.length ? boxes[boxes.length - 1]! : null;
  };
  const lastBox = (r: Run) => charBox(r.last, r.last.data.search(/\S\s*$/));
  const firstBox = (r: Run) => charBox(r.first, r.first.data.search(/\S/));
  const raised = (el: Element) => el.closest('sup, sub') !== null || ['super', 'sub'].includes(style(el).verticalAlign);
  // Whether the eye reads the two runs as one word: one line, and the gap between the last
  // character of the first and the first character of the second under SPACE_EM.
  const touching = (a: Run, b: Run): boolean => {
    const ra = lastBox(a);
    const rb = firstBox(b);
    if (!ra || !rb || ra.height <= 0 || rb.height <= 0) return false;
    const overlap = Math.min(ra.bottom, rb.bottom) - Math.max(ra.top, rb.top);
    if (overlap < 0.5 * Math.min(ra.height, rb.height)) return false;
    const em = parseFloat(style(a.el).fontSize) || 16;
    const gap = rb.left - ra.right;
    return gap > -0.25 * em && gap < SPACE_EM * em;
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
    if (next) {
      const tail = run.text.trimEnd();
      const head = next.text.trimStart();
      const word = /[\p{L}\p{N}‐-―-]$/u.test(tail) && /^[\p{L}\p{N}]/u.test(head);
      const closer = /[%)\]]$/u.test(tail) && /^\p{L}/u.test(head);
      const unit = /\p{N}$/u.test(tail) && UNIT.test(head);
      if ((word || closer) && !unit && !raised(next.el) && !raised(el) && touching(run, next)) {
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
