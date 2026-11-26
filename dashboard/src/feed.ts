import { ref, shallowRef } from 'vue';
import type { Snapshot } from './extra.ts';

export type Link = 'connecting' | 'live' | 'polling' | 'offline';

const POLL_MS = 5000;

export function useFeed() {
  const snapshot = shallowRef<Snapshot | null>(null);
  const link = ref<Link>('connecting');
  const error = ref<string | null>(null);
  let source: EventSource | null = null;
  let timer: ReturnType<typeof setInterval> | null = null;

  function accept(s: Snapshot) {
    snapshot.value = s;
    error.value = null;
  }

  async function fetchOnce(): Promise<boolean> {
    try {
      const r = await fetch('dashboard/snapshot', { cache: 'no-store' });
      if (!r.ok) throw new Error(`HTTP ${r.status}`);
      accept((await r.json()) as Snapshot);
      return true;
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
      return false;
    }
  }

  function startPolling() {
    if (timer !== null) return;
    timer = setInterval(async () => {
      link.value = (await fetchOnce()) ? 'polling' : 'offline';
    }, POLL_MS);
  }

  function stopPolling() {
    if (timer !== null) clearInterval(timer);
    timer = null;
  }

  function openStream() {
    if (typeof EventSource === 'undefined') {
      link.value = 'polling';
      startPolling();
      return;
    }
    source = new EventSource('dashboard/events');
    source.addEventListener('open', () => {
      link.value = 'live';
      stopPolling();
    });
    source.addEventListener('snapshot', (ev) => {
      try {
        accept(JSON.parse((ev as MessageEvent<string>).data) as Snapshot);
        link.value = 'live';
        stopPolling();
      } catch {
        error.value = 'bad snapshot event';
      }
    });
    // A stream that drops is first a reconnect the browser makes on its own: the page reads
    // connecting until its first poll, one poll interval later, finds the stream still down.
    source.addEventListener('error', () => {
      if (link.value === 'live') link.value = snapshot.value ? 'connecting' : 'offline';
      else if (link.value === 'connecting' && !snapshot.value) link.value = 'offline';
      startPolling();
    });
  }

  // The first snapshot leaves the page connecting until the stream opens or fails: it does not poll
  // yet, so it is neither polling nor a problem.
  async function start() {
    link.value = (await fetchOnce()) ? 'connecting' : 'offline';
    openStream();
  }

  return { snapshot, link, error, start };
}
