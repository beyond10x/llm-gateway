// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, test } from 'vitest';
import { mount } from '@vue/test-utils';
import indexHtml from '../index.html?raw';
import ThemeToggle from './ThemeToggle.vue';
import { THEME_KEY, initTheme, parseTheme, resolvedTheme, setTheme, theme } from './theme.ts';

beforeEach(() => {
  localStorage.clear();
  delete document.documentElement.dataset.theme;
  theme.value = 'system';
});
afterEach(() => localStorage.clear());

test('an unknown or missing stored value is system', () => {
  expect(parseTheme(null)).toBe('system');
  expect(parseTheme('sepia')).toBe('system');
  expect(parseTheme('dark')).toBe('dark');
  initTheme();
  expect(document.documentElement.dataset.theme).toBe('system');
});

test('a choice persists in localStorage and sets data-theme on <html>', () => {
  setTheme('dark');
  expect(localStorage.getItem(THEME_KEY)).toBe('dark');
  expect(document.documentElement.dataset.theme).toBe('dark');
  delete document.documentElement.dataset.theme;
  initTheme();
  expect(theme.value).toBe('dark');
  expect(document.documentElement.dataset.theme).toBe('dark');
  setTheme('system');
  expect(localStorage.getItem(THEME_KEY)).toBe('system');
});

test('system follows the media query; light and dark do not', () => {
  const dark = () => ({ matches: true });
  const light = () => ({ matches: false });
  expect(resolvedTheme('system', dark)).toBe('dark');
  expect(resolvedTheme('system', light)).toBe('light');
  expect(resolvedTheme('light', dark)).toBe('light');
  expect(resolvedTheme('dark', light)).toBe('dark');
});

test('the page applies the stored theme before its first paint, under the same key', () => {
  const head = indexHtml.slice(0, indexHtml.indexOf('</head>'));
  expect(head).toContain(`localStorage.getItem('${THEME_KEY}')`);
  expect(head).toContain('dataset.theme');
});

test('the toggle is a radio group named Theme whose choice applies and persists', async () => {
  initTheme();
  const wrapper = mount(ThemeToggle);
  const group = wrapper.get('[role="radiogroup"]');
  expect(group.attributes('aria-label')).toBe('Theme');
  const radios = wrapper.findAll('input[type="radio"]');
  expect(radios.map((r) => (r.element as HTMLInputElement).value)).toEqual(['light', 'dark', 'system']);
  expect((radios[2]!.element as HTMLInputElement).checked).toBe(true);
  await radios[1]!.setValue(true);
  expect(document.documentElement.dataset.theme).toBe('dark');
  expect(localStorage.getItem(THEME_KEY)).toBe('dark');
  await radios[0]!.setValue(true);
  expect(document.documentElement.dataset.theme).toBe('light');
  wrapper.unmount();
});
