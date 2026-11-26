<script setup lang="ts">
// A host slot: the component the page registers under the slot's `host`, given the rows of the
// slot's read. The host draws its own frame.
import { computed } from 'vue';
import { readRows, scopeOf, useCtx } from '../context.ts';
import type { HostSlot } from '../types.ts';

const props = defineProps<{ node: HostSlot }>();
const ctx = useCtx();

const host = computed(() => {
  const c = ctx.hosts[props.node.host];
  if (!c) throw new Error(`no host component is registered as ${props.node.host}`);
  return c;
});
const rows = computed(() => readRows(ctx, props.node.reads, scopeOf(ctx)) ?? []);
</script>

<template>
  <component :is="host" :rows="rows" />
</template>
