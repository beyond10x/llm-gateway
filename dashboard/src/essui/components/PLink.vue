<script setup lang="ts">
// The link primitive: its words, opening a page of the document or an external address.
import { computed } from 'vue';
import { routeOf, textOf, useCtx } from '../context.ts';
import type { LinkNode, Row } from '../types.ts';

const props = defineProps<{ node: LinkNode; row?: Row }>();
const ctx = useCtx();
const words = computed(() => textOf(ctx, props.node.text, props.row));
const href = computed(() => (props.node.to ? routeOf(ctx, props.node.to.to) : textOf(ctx, props.node.href ?? '', props.row)));
</script>

<template>
  <a :href="href">{{ words }}</a>
</template>
