<script setup lang="ts">
// Draws one node by its kind. `row` is the row the node stands in (a record's row for its items, a
// collection's row for its expanded detail), which its expressions read as row.<field>.
import { computed } from 'vue';
import { holds } from '../expr.ts';
import { scopeOf, useCtx } from '../context.ts';
import type { Node, Row } from '../types.ts';
import CollectionView from './CollectionView.vue';
import ChoiceView from './ChoiceView.vue';
import HostView from './HostView.vue';
import MetricView from './MetricView.vue';
import PBadge from './PBadge.vue';
import PButton from './PButton.vue';
import PLink from './PLink.vue';
import PText from './PText.vue';
import PToggle from './PToggle.vue';
import RecordView from './RecordView.vue';

const props = defineProps<{ node: Node; row?: Row; title?: string; caption?: string; empty?: string; inline?: boolean; nested?: boolean }>();
const ctx = useCtx();

const shown = computed(() => holds((props.node as { visible?: string }).visible, scopeOf(ctx, props.row)));
</script>

<template>
  <template v-if="shown">
    <HostView v-if="'host' in node" :node="node" />
    <template v-else-if="'primitive' in node">
      <PText v-if="node.primitive === 'text'" :node="node" :row="row" />
      <PBadge v-else-if="node.primitive === 'badge'" :node="node" :row="row" />
      <PLink v-else-if="node.primitive === 'link'" :node="node" :row="row" />
      <PButton v-else-if="node.primitive === 'button'" :node="node" />
      <PToggle v-else-if="node.primitive === 'toggle'" :node="node" />
    </template>
    <CollectionView v-else-if="node.component === 'collection'" :node="node" :row="row" :title="title" :caption="caption" :empty="empty" :nested="nested" />
    <RecordView v-else-if="node.component === 'record'" :node="node" :row="row" />
    <MetricView v-else-if="node.component === 'metric'" :node="node" :row="row" :inline="inline" />
    <ChoiceView v-else-if="node.component === 'choice'" :node="node" />
  </template>
</template>
