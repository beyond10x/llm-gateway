// @vitest-environment happy-dom
import { afterEach, expect, test, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import App from './App.vue';
import { fixture } from './fixture.ts';

type Listener = (event: Event) => void;

// The one event stream the page opens; the test sends its first snapshot event.
class FakeEventSource {
  static last: FakeEventSource | null = null;
  readonly listeners = new Map<string, Listener[]>();

  readonly url: string;

  constructor(url: string) {
    this.url = url;
    FakeEventSource.last = this;
  }

  addEventListener(type: string, listener: Listener) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }

  emit(type: string, data?: string) {
    const event = new MessageEvent(type, { data });
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }

  close() {}
}

afterEach(() => vi.unstubAllGlobals());

test('the first snapshot event renders the page, and the calls table shows the last 30', async () => {
  vi.stubGlobal('fetch', vi.fn(async () => new Response('', { status: 503 })));
  vi.stubGlobal('EventSource', FakeEventSource);
  const wrapper = mount(App);
  await flushPromises();
  expect(wrapper.find('main.waiting').exists()).toBe(true);

  FakeEventSource.last?.emit('open');
  FakeEventSource.last?.emit('snapshot', JSON.stringify(fixture(Date.now())));
  await flushPromises();

  const text = wrapper.text();
  expect(text).not.toContain('Waiting for the first snapshot');
  expect(text).toContain('Cancelled');
  expect(text).toContain('Qwen/Qwen3-Coder-30B-A3B-Instruct');
  expect(wrapper.find('table.calls').findAll('tbody > tr').length).toBe(30);
  expect(wrapper.text()).toContain('Show all 40');
  wrapper.unmount();
});
