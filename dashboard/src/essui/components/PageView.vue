<script setup lang="ts">
// The page's sections in the order of its layout's areas, one area under the other. An area of
// frame `each` draws each section in a frame of its own; a `shared` area draws its sections in one
// frame under its title, and is left out when none of them is visible.
import { computed } from 'vue';
import { holds } from '../expr.ts';
import { scopeOf, useCtx } from '../context.ts';
import type { Frame, Section } from '../types.ts';
import SectionView from './SectionView.vue';

const ctx = useCtx();

interface Area {
  name: string;
  shared: boolean;
  title?: string;
  sections: Section[];
}

const byName = (name: string) => {
  const s = ctx.page.sections.find((x) => x.name === name);
  if (!s) throw new Error(`the layout places a section the page does not have: ${name}`);
  return s;
};

const areas = computed<Area[]>(() => {
  const layout = ctx.page.layout;
  if (layout === undefined || layout === 'stack') return ctx.page.sections.map((s) => ({ name: s.name, shared: false, sections: [s] }));
  return layout.areas.grid.map(([name]) => {
    const place = layout.areas.place[name!]!;
    const frame: Frame = Array.isArray(place) ? { sections: place } : place;
    return { name: name!, shared: frame.frame === 'shared', title: frame.title, sections: frame.sections.map(byName) };
  });
});

const shown = (s: Section) => holds(s.visible, scopeOf(ctx));
</script>

<template>
  <template v-for="a in areas" :key="a.name">
    <template v-if="!a.shared">
      <SectionView v-for="s in a.sections.filter(shown)" :key="s.name" :section="s" />
    </template>
    <section v-else-if="a.sections.some(shown)" :aria-labelledby="`h-${a.name}`">
      <h2 :id="`h-${a.name}`">{{ a.title }}</h2>
      <SectionView v-for="s in a.sections.filter(shown)" :key="s.name" :section="s" in-frame />
    </section>
  </template>
</template>
