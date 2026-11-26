import { ref } from 'vue';

// The page's colour theme. `system` follows the browser's `prefers-color-scheme`; `light` and
// `dark` force a palette. The choice is kept in localStorage and set as `data-theme` on <html>;
// index.html applies the stored choice in an inline script before the first paint.
export type Theme = 'light' | 'dark' | 'system';

export const THEMES: readonly Theme[] = ['light', 'dark', 'system'];
export const THEME_KEY = 'llm-gateway.theme';
export const DARK_QUERY = '(prefers-color-scheme: dark)';

export function parseTheme(value: string | null | undefined): Theme {
  return value === 'light' || value === 'dark' ? value : 'system';
}

export function storedTheme(): Theme {
  try {
    return parseTheme(localStorage.getItem(THEME_KEY));
  } catch {
    return 'system';
  }
}

export function applyTheme(theme: Theme, root: HTMLElement = document.documentElement) {
  root.dataset.theme = theme;
}

// The palette a theme shows: `system` asks the media query.
export function resolvedTheme(theme: Theme, matchMedia: ((query: string) => { matches: boolean }) | undefined = globalThis.matchMedia): 'light' | 'dark' {
  if (theme !== 'system') return theme;
  return matchMedia?.(DARK_QUERY).matches ? 'dark' : 'light';
}

export const theme = ref<Theme>('system');

export function initTheme() {
  theme.value = storedTheme();
  applyTheme(theme.value);
}

export function setTheme(next: Theme) {
  theme.value = next;
  applyTheme(next);
  try {
    localStorage.setItem(THEME_KEY, next);
  } catch {
    // A browser without storage keeps the choice for this page only.
  }
}
