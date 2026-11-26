<script setup lang="ts">
// The content of one field's value, inside the cell, heading or definition that holds it: a badge
// in its tone, a list as tags, or the value's words with a Rich value's note, tag and lines.
import { computed } from 'vue';
import { labelsOf, toneOf, useCtx, wordsOf } from '../context.ts';
import { isRich, type Field, type Value } from '../types.ts';

const props = defineProps<{ value: Value | undefined; field: Field }>();
const ctx = useCtx();

const rich = computed(() => (isRich(props.value) ? props.value : null));
const words = computed(() => wordsOf(props.value, props.field.as, labelsOf(ctx, props.field.labels)));
const tone = computed(() => toneOf(ctx, props.field, props.value) ?? 'neutral');
const tags = computed(() => (Array.isArray(props.value) ? props.value : []));
</script>

<template>
  <template v-if="field.as === 'tags'"><span v-for="t in tags" :key="t" class="flag">{{ t }}</span></template>
  <template v-else>
    <span v-if="field.as === 'badge'" class="pill" :class="`tone-${tone}`" :title="rich?.title">{{ words }}</span>
    <span v-else-if="rich?.mono" class="mono">{{ words }}</span>
    <template v-else>{{ words }}</template>
    <template v-if="rich?.note"> <span class="muted small" :class="{ mono: rich.note.mono }">{{ rich.note.text }}</span></template>
    <template v-if="rich?.tag"> <span class="badge" :title="rich.tag.title">{{ rich.tag.text }}</span></template>
    <div v-for="(s, i) in rich?.sub ?? []" :key="i" class="muted small" :class="{ mono: s.mono }" :title="s.title">{{ s.text }}</div>
  </template>
</template>
