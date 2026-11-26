<script setup lang="ts">
// A collection: a table in a scrolling region named after its title (ScrollRegion), a mini table
// when it is another collection's expanded detail, or cards (`style: cards`), one per row, headed by
// the row's key and holding a tile per field and a definition list per map, or a list (`style:
// list`), one item per row: its columns, then its `item` nodes reading the row. In a table the key
// column is each row's header. A column named like the collection's `expand` node holds the button
// that opens that detail under the row. A client pager draws the first page and a button for all.
import { computed, reactive, ref } from 'vue';
import { holds } from '../expr.ts';
import { NUMERIC, counted, readRows, scopeOf, toneClass, toneOf, useCtx } from '../context.ts';
import { isRich, type CollectionNode, type Field, type Part, type Row } from '../types.ts';
import CellValue from './CellValue.vue';
import NodeView from './NodeView.vue';
import ScrollRegion from './ScrollRegion.vue';
import Tile from './Tile.vue';

const props = defineProps<{ node: CollectionNode; row?: Row; title?: string; caption?: string; empty?: string; nested?: boolean }>();
const ctx = useCtx();

const rows = computed(() => readRows(ctx, props.node.reads, scopeOf(ctx, props.row)) ?? []);
const key = computed(() => props.node.reads?.key ?? 'id');
const keyOf = (r: Row, i: number) => {
  const v = r[key.value];
  return typeof v === 'string' || typeof v === 'number' ? v : isRich(v) ? v.text : i;
};

const pageSize = computed(() => (props.node.reads?.paging === 'client' ? props.node.reads.pager?.sizes?.[0] : undefined));
const showAll = ref(false);
const shown = computed(() => (pageSize.value !== undefined && !showAll.value ? rows.value.slice(0, pageSize.value) : rows.value));

const expandName = computed(() => (props.node.expand as { name?: string } | undefined)?.name);
const open = reactive(new Set<string | number>());
function toggle(k: string | number) {
  if (open.has(k)) open.delete(k);
  else open.add(k);
}

// A condition that reads the row is judged per row; any other shows or hides the whole column.
const perRow = (f: Field) => f.visible !== undefined && /\brow\./.test(f.visible);
const columns = computed(() => props.node.columns.filter((f) => perRow(f) || holds(f.visible, scopeOf(ctx, props.row))));
const cellShown = (f: Field, r: Row) => !perRow(f) || holds(f.visible, scopeOf(ctx, r));

function cellClass(f: Field, r: Row): unknown[] {
  const v = r[f.field];
  const rich = isRich(v) ? v : null;
  return [
    `col-${f.name}`,
    { n: NUMERIC.includes(f.as), wrap: rich?.wrap || f.as === 'json', mono: f.as === 'json' || rich?.mono, muted: rich?.muted },
    ...(f.as === 'badge' ? [] : toneClass(toneOf(ctx, f, v))),
  ];
}

const cellTitle = (f: Field, r: Row) => {
  const v = r[f.field];
  return f.as !== 'badge' && isRich(v) ? v.title : undefined;
};

const keyColumn = computed(() => props.node.columns.find((f) => f.field === key.value) ?? props.node.columns[0]!);
const tileColumns = computed(() => props.node.columns.filter((f) => f !== keyColumn.value && f.as !== 'map'));
const mapColumns = computed(() => props.node.columns.filter((f) => f.as === 'map'));
const entries = (v: unknown): [Part, Part][] => (isRich(v) ? (v.table ?? []).map((r) => [r[0] ?? { text: '' }, r[1] ?? { text: '' }]) : v && typeof v === 'object' ? Object.entries(v as Record<string, string>).map(([k, x]) => [{ text: k, mono: true }, { text: x }]) : []);
</script>

<template>
  <table v-if="nested" class="mini">
    <thead>
      <tr><th v-for="c in columns" :key="c.name" scope="col" :class="{ n: NUMERIC.includes(c.as) }">{{ c.label ?? c.field }}</th></tr>
    </thead>
    <tbody>
      <tr v-for="(r, i) in shown" :key="keyOf(r, i)">
        <td v-for="c in columns" :key="c.name" :class="cellClass(c, r)" :title="cellTitle(c, r)"><CellValue v-if="cellShown(c, r)" :value="r[c.field]" :field="c" /></td>
      </tr>
    </tbody>
  </table>

  <template v-else-if="node.style === 'cards'">
    <div v-for="(r, i) in shown" :key="keyOf(r, i)" class="card">
      <h3>{{ keyColumn.label ? `${keyColumn.label} · ` : '' }}<CellValue :value="r[keyColumn.field]" :field="keyColumn" /></h3>
      <div class="tiles">
        <template v-for="c in tileColumns" :key="c.name">
          <Tile v-if="cellShown(c, r)" :field="c" :value="r[c.field]" />
        </template>
      </div>
      <template v-for="c in mapColumns" :key="c.name">
        <dl v-if="cellShown(c, r) && entries(r[c.field]).length" class="headers">
          <template v-for="[k, v] in entries(r[c.field])" :key="k.title ?? k.text">
            <dt :class="{ mono: k.mono }" :title="k.title">{{ k.text }}</dt>
            <dd class="mono" :title="v.title">{{ v.text }}</dd>
          </template>
        </dl>
      </template>
    </div>
  </template>

  <ul v-else-if="node.style === 'list'" class="list" :class="node.name">
    <li v-for="(r, i) in shown" :key="keyOf(r, i)">
      <template v-for="c in columns" :key="c.name"><CellValue v-if="cellShown(c, r)" :value="r[c.field]" :field="c" />{{ ' ' }}</template>
      <NodeView v-for="n in node.item ?? []" :key="(n as { name?: string }).name" :node="n" :row="r" />
    </li>
  </ul>

  <template v-else>
    <ScrollRegion :label="`${title ?? node.name} table`">
      <table :class="node.name">
        <caption v-if="caption">{{ caption }}</caption>
        <thead>
          <tr>
            <th v-for="c in columns" :key="c.name" scope="col" :class="{ n: NUMERIC.includes(c.as) }">
              <abbr v-if="c.help?.text" :title="c.help.text">{{ c.label ?? c.field }}</abbr><template v-else>{{ c.label ?? c.field }}</template>
            </th>
          </tr>
        </thead>
        <tbody>
          <template v-for="(r, i) in shown" :key="keyOf(r, i)">
            <tr>
              <component :is="c.field === key ? 'th' : 'td'" v-for="c in columns" :key="c.name" :scope="c.field === key ? 'row' : undefined" :class="cellClass(c, r)" :title="cellTitle(c, r)">
                <template v-if="c.name === expandName">
                  <button v-if="Number(r[c.field]) > 0" type="button" class="expand" :aria-expanded="open.has(keyOf(r, i))" @click="toggle(keyOf(r, i))">
                    <span aria-hidden="true">{{ open.has(keyOf(r, i)) ? '▾' : '▸' }}</span>{{ ` ${counted(Number(r[c.field]), c)}` }}
                  </button>
                  <span v-else class="muted">–</span>
                </template>
                <CellValue v-else-if="cellShown(c, r)" :value="r[c.field]" :field="c" />
              </component>
            </tr>
            <tr v-if="node.expand && open.has(keyOf(r, i))" :class="expandName">
              <td :colspan="columns.length"><NodeView :node="node.expand" :row="r" nested /></td>
            </tr>
          </template>
          <tr v-if="rows.length === 0 && empty"><td :colspan="columns.length" class="empty">{{ empty }}</td></tr>
        </tbody>
      </table>
    </ScrollRegion>
    <p v-if="pageSize !== undefined && rows.length > pageSize" class="more">
      <button type="button" @click="showAll = !showAll">{{ showAll ? `Show the last ${pageSize}` : `Show all ${rows.length}` }}</button>
    </p>
  </template>
</template>
