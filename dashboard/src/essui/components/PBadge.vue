<script setup lang="ts">
// The badge primitive: its words in a pill of its tone, fixed or taken from a value by its map.
import { computed } from 'vue';
import { value as evalExpr } from '../expr.ts';
import { scopeOf, textOf, useCtx } from '../context.ts';
import type { BadgeNode, Row, Tone } from '../types.ts';

const props = defineProps<{ node: BadgeNode; row?: Row }>();
const ctx = useCtx();
const words = computed(() => textOf(ctx, props.node.text, props.row));
const tone = computed<Tone>(() => {
  const by = props.node.tone_by;
  if (!by) return props.node.tone ?? 'neutral';
  const key = evalExpr(by.value, scopeOf(ctx, props.row));
  const map = by.map as Record<string, Tone> | { bands: { from: number; tone: Tone }[] };
  if ('bands' in map && Array.isArray(map.bands)) {
    let t: Tone | undefined;
    if (typeof key === 'number') for (const b of map.bands) if (key >= b.from) t = b.tone;
    return t ?? props.node.tone ?? 'neutral';
  }
  return (map as Record<string, Tone>)[String(key)] ?? props.node.tone ?? 'neutral';
});
</script>

<template>
  <span class="pill" :class="`tone-${tone}`">{{ words }}</span>
</template>
