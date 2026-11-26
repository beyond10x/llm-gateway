// The dashboard's source of rows: the gateway's snapshot from the feed (one fetch, then the event
// stream, polling while it is down), answered through the binding. In development, `?fixture`
// replays the development fixture instead, one new call a second.
import { computed, ref } from 'vue';
import { useFeed } from './feed.ts';
import type { Snapshot } from './extra.ts';
import { snapshotSource, type Source } from './essui/source.ts';

export function liveSource(): Source {
  const feed = useFeed();
  const replayed = ref<Snapshot | null>(null);
  const snapshot = computed(() => replayed.value ?? feed.snapshot.value);
  const link = computed(() => (replayed.value ? 'live' : feed.link.value));
  const source = snapshotSource(snapshot, link, feed.error);
  if (import.meta.env.DEV && new URLSearchParams(location.search).has('fixture')) {
    void import('./fixture.ts').then(({ fixture }) => {
      let replay = 0;
      replayed.value = fixture(Date.now(), replay);
      setInterval(() => (replayed.value = fixture(Date.now(), ++replay)), 1000);
    });
  } else {
    void feed.start();
  }
  return source;
}
