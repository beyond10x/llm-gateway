<script setup lang="ts">
// A metric: one value, from an expression over the row it stands in or an aggregate over the rows
// of its read, drawn as a tile, or inline in a header as its label and value. A metric with no
// value yet draws nothing inline.
import { computed } from 'vue';
import { value as evalExpr } from '../expr.ts';
import { readRows, scopeOf, useCtx, wordsOf } from '../context.ts';
import type { Field, MetricNode, Row, Value } from '../types.ts';
import Tile from './Tile.vue';

const props = defineProps<{ node: MetricNode; row?: Row; inline?: boolean }>();
const ctx = useCtx();

const AS: Record<NonNullable<MetricNode['format']>, Field['as']> = { number: 'number', percent: 'percent', duration: 'duration' };
const UNIT_MS = { ms: 1, s: 1000, m: 60_000, h: 3_600_000 } as const;

const value = computed<Value | undefined>(() => {
  const n = props.node;
  if (n.from !== undefined) return evalExpr(n.from, scopeOf(ctx, props.row)) as Value | undefined;
  const rows = readRows(ctx, n.reads, scopeOf(ctx, props.row));
  if (!rows) return undefined;
  if (n.aggregate === 'count') return rows.length;
  const nums = rows.map((r) => r[n.field!]).filter((v): v is number => typeof v === 'number');
  if (nums.length === 0) return null;
  let v: number;
  switch (n.aggregate) {
    case 'sum':
      v = nums.reduce((a, b) => a + b, 0);
      break;
    case 'avg':
      v = nums.reduce((a, b) => a + b, 0) / nums.length;
      break;
    case 'min':
      v = Math.min(...nums);
      break;
    default:
      v = Math.max(...nums);
  }
  return n.format === 'duration' ? v * UNIT_MS[n.unit ?? 's'] : v;
});

const field = computed(() => ({ label: props.node.label, as: AS[props.node.format ?? 'number'] }));
const words = computed(() => wordsOf(value.value, field.value.as));
</script>

<template>
  <span v-if="inline && value !== undefined && value !== null" class="muted">{{ node.label }} {{ words }}</span>
  <Tile v-else-if="!inline" :field="field" :value="value ?? null" :window="node.window" />
</template>
