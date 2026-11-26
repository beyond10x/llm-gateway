// The expression subset of ess-ui/1 that the document uses: paths under row, shell, state, params,
// channel and section.<name>.row, literals (numbers, quoted strings, true, false, null), == and !=,
// not, and, or, and parentheses. A path that reads nothing is undefined; a condition is true when
// its value is truthy.

export type Expr =
  | { kind: 'path'; root: Root; path: string[] }
  | { kind: 'literal'; value: string | number | boolean | null }
  | { kind: 'not'; of: Expr }
  | { kind: 'and' | 'or' | '==' | '!='; left: Expr; right: Expr };

export const ROOTS = ['row', 'shell', 'state', 'params', 'channel', 'section'] as const;
export type Root = (typeof ROOTS)[number];

export interface Scope {
  row?: unknown;
  shell?: Record<string, unknown>;
  state?: Record<string, unknown>;
  params?: Record<string, unknown>;
  channel?: (name: string) => Record<string, unknown> | undefined;
  section?: (name: string) => unknown[] | null;
}

const TOKEN = /\s*(==|!=|\(|\)|"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|[A-Za-z0-9_.$-]+)/y;

function tokenize(src: string): string[] {
  const out: string[] = [];
  TOKEN.lastIndex = 0;
  let at = 0;
  while (at < src.length) {
    if (/^\s*$/.test(src.slice(at))) break;
    TOKEN.lastIndex = at;
    const m = TOKEN.exec(src);
    if (!m) throw new Error(`cannot read '${src.slice(at).trim()}' in '${src}'`);
    out.push(m[1]!);
    at = TOKEN.lastIndex;
  }
  return out;
}

export function parse(src: string): Expr {
  const tokens = tokenize(src);
  let i = 0;
  const peek = () => tokens[i];
  const take = () => tokens[i++];

  function atom(): Expr {
    const t = take();
    if (t === undefined) throw new Error(`'${src}' ends too early`);
    if (t === '(') {
      const e = or();
      if (take() !== ')') throw new Error(`'${src}' misses a ')'`);
      return e;
    }
    if (t === 'true' || t === 'false') return { kind: 'literal', value: t === 'true' };
    if (t === 'null') return { kind: 'literal', value: null };
    if (/^-?\d+(\.\d+)?$/.test(t)) return { kind: 'literal', value: Number(t) };
    if (t.startsWith('"') || t.startsWith("'")) return { kind: 'literal', value: t.slice(1, -1).replace(/\\(.)/g, '$1') };
    const parts = t.split('.');
    const root = parts[0] as Root;
    if (!ROOTS.includes(root) || parts.length < 2 || parts.some((p) => p === '')) {
      throw new Error(`'${t}' in '${src}' is no path this renderer reads (roots: ${ROOTS.join(', ')})`);
    }
    if (root === 'section' && parts[2] !== 'row') throw new Error(`'${t}' in '${src}': a section is read only as section.<name>.row`);
    return { kind: 'path', root, path: parts.slice(1) };
  }

  function cmp(): Expr {
    const left = atom();
    const op = peek();
    if (op === '==' || op === '!=') {
      take();
      return { kind: op, left, right: atom() };
    }
    return left;
  }

  function not(): Expr {
    if (peek() === 'not') {
      take();
      return { kind: 'not', of: not() };
    }
    return cmp();
  }

  function and(): Expr {
    let left = not();
    while (peek() === 'and') {
      take();
      left = { kind: 'and', left, right: not() };
    }
    return left;
  }

  function or(): Expr {
    let left = and();
    while (peek() === 'or') {
      take();
      left = { kind: 'or', left, right: and() };
    }
    return left;
  }

  const e = or();
  if (i !== tokens.length) throw new Error(`'${src}' has more after '${tokens[i - 1]}'`);
  return e;
}

function dig(value: unknown, path: string[]): unknown {
  let v = value;
  for (const p of path) {
    if (v === null || typeof v !== 'object') return undefined;
    v = (v as Record<string, unknown>)[p];
  }
  return v;
}

export function evaluate(e: Expr, scope: Scope): unknown {
  switch (e.kind) {
    case 'literal':
      return e.value;
    case 'not':
      return !evaluate(e.of, scope);
    case 'and':
      return !!evaluate(e.left, scope) && !!evaluate(e.right, scope);
    case 'or':
      return !!evaluate(e.left, scope) || !!evaluate(e.right, scope);
    case '==':
      return (evaluate(e.left, scope) ?? null) === (evaluate(e.right, scope) ?? null);
    case '!=':
      return (evaluate(e.left, scope) ?? null) !== (evaluate(e.right, scope) ?? null);
    case 'path': {
      const [first, ...rest] = e.path;
      switch (e.root) {
        case 'row':
          return dig(scope.row, e.path);
        case 'shell':
          return dig(scope.shell, e.path);
        case 'state':
          return dig(scope.state, e.path);
        case 'params':
          return dig(scope.params, e.path);
        case 'channel':
          return dig(scope.channel?.(first!), rest);
        case 'section': {
          const rows = scope.section?.(first!);
          return rows && rows.length > 0 ? dig(rows[0], rest.slice(1)) : undefined;
        }
      }
    }
  }
}

const cache = new Map<string, Expr>();

// The value of `src` in `scope`; a document's expressions were parsed once at build time, so a
// parse error here means the document changed under a running page.
export function value(src: string, scope: Scope): unknown {
  let e = cache.get(src);
  if (!e) {
    e = parse(src);
    cache.set(src, e);
  }
  return evaluate(e, scope);
}

export function holds(src: string | undefined, scope: Scope): boolean {
  return src === undefined || !!value(src, scope);
}
