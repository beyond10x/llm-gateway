// Where the renderer's reads are answered: from the gateway's snapshot through the binding (the
// live page, and the `?fixture` development replay), or from the document's fixture files (tests
// and `?fixtures`). A source is reactive: a component that asks it for rows re-renders when they
// change.
import { computed, ref, watch, type Ref } from 'vue';
import type { Link } from '../feed.ts';
import type { Snapshot } from '../extra.ts';
import { answer, type Params } from './bind.ts';
import type { Lifecycle, Reads, Row } from './types.ts';

export interface Source {
  // The rows a read answers, or null while nothing has arrived.
  rows(reads: Reads | undefined, params: Params): Row[] | null;
  // The state of a channel's connection.
  lifecycle(channel: string): Lifecycle;
  // While the page has nothing to show, what it says instead; null once it has.
  pending: Ref<string | null>;
  stop(): void;
}

// The feed's connection states in the channel lifecycle of ess-ui/1: polling while the stream is
// down is a reconnect; a gateway that does not answer is stale.
export const LINK_LIFECYCLE: Record<Link, Lifecycle> = {
  connecting: 'connecting',
  live: 'live',
  polling: 'reconnecting',
  offline: 'stale',
};

export function snapshotSource(snapshot: Readonly<Ref<Snapshot | null>>, link: Readonly<Ref<Link>>, error: Readonly<Ref<string | null>>): Source {
  // Countdowns tick every second on the gateway's clock: the offset is taken from each snapshot.
  const tick = ref(Date.now());
  const offset = ref(0);
  const stopWatch = watch(snapshot, (s) => (offset.value = s ? s.now_ms - Date.now() : 0), { immediate: true });
  const timer = setInterval(() => (tick.value = Date.now()), 1000);
  const now = computed(() => tick.value + offset.value);
  return {
    rows(reads, params) {
      const s = snapshot.value;
      if (!s || !reads?.placeholder) return null;
      return answer(reads.placeholder, s, now.value, params);
    },
    lifecycle: () => LINK_LIFECYCLE[link.value],
    pending: computed(() => (snapshot.value ? null : error.value ? `Cannot reach the gateway: ${error.value}` : 'Waiting for the first snapshot…')),
    stop() {
      clearInterval(timer);
      stopWatch();
    },
  };
}

export interface FixtureFile {
  view: string;
  rows: Row[];
}

// Answers each placeholder from its fixture file; a row that carries a field named like a read
// parameter answers only that parameter's value.
export function fixtureSource(files: FixtureFile[]): Source {
  const byView = new Map(files.map((f) => [f.view, f.rows]));
  return {
    rows(reads, params) {
      if (!reads?.placeholder) return null;
      const rows = byView.get(reads.placeholder);
      if (!rows) throw new Error(`no fixture answers the placeholder ${reads.placeholder}`);
      return rows.filter((r) => Object.entries(params).every(([k, v]) => !(k in r) || r[k] === v));
    },
    lifecycle: () => 'live',
    pending: ref(null),
    stop() {},
  };
}
