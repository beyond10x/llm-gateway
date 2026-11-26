<script setup lang="ts">
// One section: a host slot drawn by its host, or a composite in a frame of its own under its title
// (or inside a shared frame, where its title names its table). A child metric that counts the
// section's rows and has no label is drawn beside the title, as the number of rows. In a shared
// frame a heading text is drawn in the frame's heading (PageView), not again here.
import { computed } from 'vue';
import { isHeadingText, readRows, scopeOf, useCtx } from '../context.ts';
import type { MetricNode, Node, Section } from '../types.ts';
import HostView from './HostView.vue';
import NodeView from './NodeView.vue';

const props = defineProps<{ section: Section; inFrame?: boolean }>();
const ctx = useCtx();

const isCount = (n: Node): n is MetricNode => 'component' in n && n.component === 'metric' && n.aggregate === 'count' && !n.label;
const counts = computed(() => (props.section.children ?? []).filter(isCount).map((m) => ({ name: m.name, n: readRows(ctx, m.reads, scopeOf(ctx))?.length ?? 0 })));
const others = computed(() => (props.section.children ?? []).filter((n) => !isCount(n) && !(props.inFrame && isHeadingText(n))));
const empty = computed(() => props.section.states?.empty?.message);
const id = computed(() => `h-${props.section.name}`);
</script>

<template>
  <HostView v-if="'host' in section" :node="section" />
  <template v-else-if="inFrame">
    <NodeView :node="section" :title="section.title" :caption="section.title" :empty="empty" />
    <NodeView v-for="c in others" :key="(c as { name?: string }).name" :node="c" />
  </template>
  <section v-else :aria-labelledby="section.title ? id : undefined">
    <h2 v-if="section.title" :id="id">{{ section.title }}<template v-for="c in counts" :key="c.name">{{ " " }}<span class="badge">{{ c.n }}</span></template></h2>
    <NodeView :node="section" :title="section.title" :empty="empty" />
    <NodeView v-for="c in others" :key="(c as { name?: string }).name" :node="c" />
  </section>
</template>
