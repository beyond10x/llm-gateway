// Shell state: each state the shell declares, its value restored from its store, and the theme the
// document's ThemeChoice names applied on <html> whenever its state changes. A `local_storage`
// state is kept under `<app>.<name>` (the theme under `llm-gateway.theme`, which index.html reads
// before the first paint); any other store lives as long as the page.
import { reactive } from 'vue';
import { applyTheme, type Theme } from '../theme.ts';
import type { Document, StateDecl } from './types.ts';

export interface ShellState {
  values: Record<string, unknown>;
  set(name: string, value: unknown): void;
}

export function storageKey(doc: Document, name: string): string {
  return `${doc.app}.${name}`;
}

function allowed(decl: StateDecl, v: unknown): boolean {
  const t = decl.type as { enum?: unknown[] } | string;
  if (typeof t === 'object' && t !== null && Array.isArray(t.enum)) return t.enum.includes(v);
  if (t === 'boolean') return typeof v === 'boolean';
  if (t === 'string') return typeof v === 'string';
  return true;
}

function restore(doc: Document, name: string, decl: StateDecl): unknown {
  if (decl.store === 'local_storage') {
    try {
      const raw = localStorage.getItem(storageKey(doc, name));
      if (raw !== null) {
        const v = decl.type === 'boolean' ? raw === 'true' : raw;
        if (allowed(decl, v)) return v;
      }
    } catch {
      // A browser without storage starts from the default.
    }
  }
  return decl.default ?? null;
}

export function shellState(doc: Document, shell = 'app'): ShellState {
  const decls = doc.shells[shell]?.state ?? {};
  const values = reactive<Record<string, unknown>>({});
  for (const [name, decl] of Object.entries(decls)) values[name] = restore(doc, name, decl);
  const themeState = doc.theme?.chosen_by?.startsWith('shell.') ? doc.theme.chosen_by.slice('shell.'.length) : null;
  const show = () => {
    if (themeState !== null) applyTheme(String(values[themeState] ?? doc.theme!.default) as Theme);
  };
  show();
  return {
    values,
    set(name, value) {
      const decl = decls[name];
      if (!decl) throw new Error(`the shell declares no state ${name}`);
      if (!allowed(decl, value)) throw new Error(`${String(value)} is not a value of shell.${name}`);
      values[name] = value;
      if (decl.store === 'local_storage') {
        try {
          localStorage.setItem(storageKey(doc, name), String(value));
        } catch {
          // A browser without storage keeps the choice for this page only.
        }
      }
      if (name === themeState) show();
    },
  };
}
