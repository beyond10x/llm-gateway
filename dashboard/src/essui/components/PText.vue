<script setup lang="ts">
// The text primitive: its words in its style (a heading, a caption, monospace, or body text).
import { computed } from 'vue';
import { textOf, useCtx } from '../context.ts';
import type { Row, TextNode } from '../types.ts';

const props = defineProps<{ node: TextNode; row?: Row }>();
const ctx = useCtx();
const words = computed(() => textOf(ctx, props.node.text, props.row));
const STYLE = { body: '', caption: 'muted small', heading: 'heading', mono: 'mono' } as const;
</script>

<template>
  <span :class="STYLE[node.style ?? 'body']">{{ words }}</span>
</template>
