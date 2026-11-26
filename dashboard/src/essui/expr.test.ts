import { expect, test } from 'vitest';
import { holds, parse, value, type Scope } from './expr.ts';

const scope: Scope = {
  row: { status: 'ok', n: 2, gone: null, nested: { a: 'x' } },
  shell: { theme: 'dark' },
  params: { call: 'c-1' },
  channel: (name) => (name === 'events' ? { lifecycle: 'live' } : undefined),
  section: (name) => (name === 'provider' ? [{ name: 'runpod' }] : name === 'empty' ? [] : null),
};

test('paths read their root and give undefined for what is not there', () => {
  expect(value('row.status', scope)).toBe('ok');
  expect(value('row.nested.a', scope)).toBe('x');
  expect(value('row.missing.deeper', scope)).toBeUndefined();
  expect(value('shell.theme', scope)).toBe('dark');
  expect(value('params.call', scope)).toBe('c-1');
  expect(value('channel.events.lifecycle', scope)).toBe('live');
  expect(value('section.provider.row', scope)).toEqual({ name: 'runpod' });
  expect(value('section.provider.row.name', scope)).toBe('runpod');
  expect(value('section.empty.row', scope)).toBeUndefined();
});

test('comparisons, not, and, or and parentheses', () => {
  expect(value("row.status == 'ok'", scope)).toBe(true);
  expect(value('row.status != "ok"', scope)).toBe(false);
  expect(value('row.n == 2', scope)).toBe(true);
  expect(value('row.gone == null and row.missing == null', scope)).toBe(true);
  expect(value('not row.gone', scope)).toBe(true);
  expect(value("row.status == 'refused' or shell.theme == 'dark'", scope)).toBe(true);
  expect(value("not (row.status == 'ok' and row.n == 3)", scope)).toBe(true);
  expect(value('true and false or true', scope)).toBe(true);
  expect(holds(undefined, scope)).toBe(true);
  expect(holds('row.gone', scope)).toBe(false);
});

test('an expression outside the subset is refused', () => {
  expect(() => parse('row.n > 1')).toThrow("cannot read '> 1'");
  expect(() => parse('window.location')).toThrow('no path this renderer reads');
  expect(() => parse('row')).toThrow('no path this renderer reads');
  expect(() => parse('section.provider.name')).toThrow('section.<name>.row');
  expect(() => parse('(row.n == 1')).toThrow("misses a ')'");
  expect(() => parse('row.n row.m')).toThrow('has more after');
  expect(() => parse('not')).toThrow('ends too early');
});
