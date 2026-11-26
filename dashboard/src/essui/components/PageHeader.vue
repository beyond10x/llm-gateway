<script setup lang="ts">
// The page header: the title, the actions that navigate as links, the headline metrics, the
// lifecycle of each live channel in words, and the actions drawn as a choice.
import { computed } from 'vue';
import { useCtx } from '../context.ts';
import type { Channel, Header, Lifecycle, Tone } from '../types.ts';
import ChoiceView from './ChoiceView.vue';
import NodeView from './NodeView.vue';
import PLink from './PLink.vue';

const props = defineProps<{ header: Header }>();
const ctx = useCtx();

const links = computed(() => (props.header.actions ?? []).filter((a) => a.navigate));
const choices = computed(() => (props.header.actions ?? []).filter((a) => a.as === 'choice' && a.choice));

const spaced = (d: string) => d.replace(/^(\d+)(ms|s|m|h)$/, '$1 $2');
const TONE: Partial<Record<Lifecycle, Tone>> = { live: 'success', reconnecting: 'warning', stale: 'danger', closed: 'danger' };

function words(state: Lifecycle, channel: Channel | undefined): string {
  switch (state) {
    case 'reconnecting':
      return channel?.reconnect ? `polling every ${spaced(channel.reconnect.backoff.from)}` : 'reconnecting';
    case 'stale':
      return 'offline';
    default:
      return state;
  }
}

const live = computed(() =>
  (props.header.live ?? []).map((name) => {
    const state = ctx.source.lifecycle(name);
    const tone = TONE[state];
    return { name, text: words(state, ctx.doc.channels?.[name]), classes: tone ? ['toned', `tone-${tone}`] : [] };
  }),
);
</script>

<template>
  <header class="top">
    <h1>{{ header.title }}</h1>
    <nav v-if="links.length" class="nav">
      <PLink v-for="a in links" :key="a.name" :node="{ primitive: 'link', name: a.name ?? '', text: a.label ?? '', to: { to: a.navigate!.to } }" />
    </nav>
    <NodeView v-for="m in header.metrics ?? []" :key="(m as { name?: string }).name" :node="m" inline />
    <span v-for="c in live" :key="c.name" class="link" :class="c.classes" role="status" aria-live="polite">
      <span class="dot" aria-hidden="true"></span>{{ c.text }}
    </span>
    <ChoiceView v-for="a in choices" :key="a.name" :node="a.choice!" :label="a.label" :name="a.name" />
  </header>
</template>
