<script setup lang="ts">
// A record: the first row of its read, its fields as a definition list (a field toned danger is an
// alarm), and its item nodes, each reading that row, as tiles.
import { computed } from 'vue';
import { holds } from '../expr.ts';
import { readRows, scopeOf, toneClass, toneOf, useCtx } from '../context.ts';
import { isRich, type Field, type RecordNode, type Row } from '../types.ts';
import CellValue from './CellValue.vue';
import NodeView from './NodeView.vue';

const props = defineProps<{ node: RecordNode; row?: Row }>();
const ctx = useCtx();

const record = computed(() => (props.node.reads ? (readRows(ctx, props.node.reads, scopeOf(ctx, props.row))?.[0] ?? null) : (props.row ?? null)));
const fields = computed(() => (record.value ? (props.node.fields ?? []).filter((f) => holds(f.visible, scopeOf(ctx, record.value))) : []));

function classes(f: Field): unknown[] {
  const tone = toneOf(ctx, f, record.value?.[f.field]);
  return [...toneClass(tone), { alarm: tone === 'danger' }];
}

const title = (f: Field) => {
  const v = record.value?.[f.field];
  return isRich(v) ? v.title : undefined;
};
</script>

<template>
  <template v-if="record">
    <dl v-if="fields.length" class="stats">
      <div v-for="f in fields" :key="f.name" :class="classes(f)">
        <dt>{{ f.label ?? f.field }}</dt>
        <dd :title="title(f)"><CellValue :value="record[f.field]" :field="f" /></dd>
      </div>
    </dl>
    <div v-if="node.item?.length" class="tiles">
      <NodeView v-for="n in node.item" :key="(n as { name?: string }).name" :node="n" :row="record" />
    </div>
  </template>
</template>
