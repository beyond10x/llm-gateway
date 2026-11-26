<script setup lang="ts">
// The toggle primitive: a switch named by its label, flipping the shell state it binds.
import { computed } from 'vue';
import { value as evalExpr } from '../expr.ts';
import { run, scopeOf, useCtx } from '../context.ts';
import type { ToggleNode } from '../types.ts';

const props = defineProps<{ node: ToggleNode }>();
const ctx = useCtx();
const on = computed(() => !!evalExpr(props.node.binds, scopeOf(ctx)));
</script>

<template>
  <button type="button" class="action" role="switch" :aria-checked="on" @click="run(ctx, { sets: { [node.binds]: 'toggle' } })">{{ node.label }}</button>
</template>
