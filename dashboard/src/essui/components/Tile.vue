<script setup lang="ts">
// One value as a tile: its label (with its help on hover), the value, its lines, and a Rich value's
// small table, in the tone of the field. A value and its lines wrap only between words or after an
// underscore, never inside a word. A tile with a table spans two columns.
import { computed } from 'vue';
import { labelsOf, rawOf, toneClass, toneOf, useCtx, wordsOf } from '../context.ts';
import { isRich, type Field, type Value } from '../types.ts';

const props = defineProps<{ field: Pick<Field, 'label' | 'help' | 'as' | 'labels' | 'tone' | 'tone_by'>; value: Value | undefined; window?: string }>();
const ctx = useCtx();

const rich = computed(() => (isRich(props.value) ? props.value : null));
const words = computed(() => wordsOf(props.value, props.field.as, labelsOf(ctx, props.field.labels)));
const help = computed(() => props.field.help?.text ?? rich.value?.about);
const classes = computed(() => [...toneClass(toneOf(ctx, { tone: props.field.tone, tone_by: props.field.tone_by }, props.value)), { wide: rich.value?.table !== undefined }]);
const windowWords = computed(() => props.window?.replace(/^(\d+)(ms|s|m|h)$/, '$1 $2'));

const broken = (v: string) => v.split(/(?<=_)/);
</script>

<template>
  <div class="tile" :class="classes">
    <div class="label"><abbr v-if="help" :title="help">{{ field.label }}</abbr><template v-else>{{ field.label }}</template></div>
    <div v-if="words !== ''" class="value" :title="rich?.title ?? rawOf(value, labelsOf(ctx, field.labels))"><template v-for="(p, i) in broken(words)" :key="i"><wbr v-if="i > 0" />{{ p }}</template></div>
    <div v-for="(s, j) in rich?.sub ?? []" :key="j" class="sub" :title="s.title">
      <template v-for="(p, i) in broken(s.text)" :key="i"><wbr v-if="i > 0" />{{ p }}</template><template v-if="windowWords && j === 0">{{ ' ' }}<span class="muted">last {{ windowWords }}</span></template>
    </div>
    <table v-if="rich?.table && (rich.table.length > 0 || rich.empty)" class="mini">
      <tbody>
        <tr v-for="(r, i) in rich.table" :key="i">
          <th scope="row">{{ r[0]?.text }}<template v-if="r[0]?.tag">{{ ' ' }}<span class="badge" :title="r[0].tag.title">{{ r[0].tag.text }}</span></template></th>
          <td v-for="(c, k) in r.slice(1)" :key="k" class="n">{{ c.text }}</td>
        </tr>
        <tr v-if="rich.table.length === 0"><td class="muted">{{ rich.empty }}</td></tr>
      </tbody>
    </table>
  </div>
</template>
