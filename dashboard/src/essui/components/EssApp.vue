<script setup lang="ts">
// The shell: a skip link, the page's header and the page outlet. A page that is a host slot is the
// host component alone, which draws its own frame.
import { computed, onUnmounted, provide, type Component } from 'vue';
import { CTX, type Ctx } from '../context.ts';
import { fixtureSource, type Source } from '../source.ts';
import { shellState } from '../state.ts';
import type { Document } from '../types.ts';
import PageHeader from './PageHeader.vue';
import PageView from './PageView.vue';

const props = defineProps<{ doc: Document; page: string; source?: Source; hosts: Record<string, Component>; hostProps?: Record<string, unknown> }>();

const page = props.doc.pages[props.page];
if (!page) throw new Error(`the document has no page ${props.page}`);
// A page that is a host slot reads nothing.
const source = props.source ?? fixtureSource([]);
const ctx: Ctx = { doc: props.doc, page, source, shell: shellState(props.doc), hosts: props.hosts };
provide(CTX, ctx);
onUnmounted(() => source.stop());

const host = computed(() => (page.host ? props.hosts[page.host] : undefined));
const pending = computed(() => source.pending.value);
</script>

<template>
  <component :is="host" v-if="page.host" v-bind="hostProps ?? {}" />
  <template v-else>
    <a class="skip" href="#content">Skip to content</a>
    <PageHeader v-if="page.header" :header="page.header" />
    <main v-if="pending === null" id="content" tabindex="-1">
      <PageView />
    </main>
    <main v-else id="content" class="waiting" tabindex="-1">
      <p>{{ pending }}</p>
    </main>
  </template>
</template>
