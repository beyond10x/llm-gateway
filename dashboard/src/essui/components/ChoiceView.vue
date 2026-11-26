<script setup lang="ts">
// A choice of fixed options bound to shell state, as a radio group named by its label: a
// segmented control (each option a labelled radio, one Tab stop, arrow keys inside).
import { computed } from 'vue';
import { value as evalExpr } from '../expr.ts';
import { scopeOf, useCtx } from '../context.ts';
import type { ChoiceNode } from '../types.ts';

const props = defineProps<{ node: ChoiceNode; label?: string; name?: string }>();
const ctx = useCtx();

const state = computed(() => {
  const m = /^shell\.([A-Za-z0-9_-]+)$/.exec(props.node.binds);
  if (!m) throw new Error(`a choice binds shell state only: ${props.node.binds}`);
  return m[1]!;
});
const current = computed(() => evalExpr(props.node.binds, scopeOf(ctx)));
const group = computed(() => props.name ?? props.node.name ?? state.value);

function pick(event: Event) {
  ctx.shell.set(state.value, (event.target as HTMLInputElement).value);
}
</script>

<template>
  <div class="segmented" role="radiogroup" :aria-label="label ?? node.name">
    <label v-for="o in node.options" :key="o.value">
      <input type="radio" :name="group" :value="o.value" :checked="current === o.value" @change="pick" />
      <span>{{ o.label }}</span>
    </label>
  </div>
</template>
