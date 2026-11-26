<script setup lang="ts">
// The /dashboard page: the page `dashboard` of ui/llm-gateway.ui.yaml, drawn by the renderer from
// the gateway's live snapshot.
import doc from 'virtual:ess-ui/document';
import EssApp from './essui/components/EssApp.vue';
import { HOSTS } from './hosts.ts';
import { liveSource } from './live.ts';

const source = liveSource();
</script>

<template>
  <EssApp :doc="doc" page="dashboard" :source="source" :hosts="HOSTS" />
</template>
