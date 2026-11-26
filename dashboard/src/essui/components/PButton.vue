<script setup lang="ts">
// The button primitive: its label, running its action.
import { run, useCtx } from '../context.ts';
import type { ButtonNode } from '../types.ts';

defineProps<{ node: ButtonNode }>();
const ctx = useCtx();
</script>

<template>
  <button type="button" class="action" :class="`emphasis-${node.tone ?? 'secondary'}`" @click="run(ctx, node.action)">{{ node.label }}</button>
</template>
