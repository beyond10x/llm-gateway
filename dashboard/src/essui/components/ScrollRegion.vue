<script setup lang="ts">
// A keyboard-reachable region that scrolls its table sideways, named after it. While columns are
// hidden past an edge, that edge fades; while some are hidden to the right, a cue above the table
// says to scroll and is the region's accessible description. Scrolled to its end, the cue's words
// and the right fade go; its line stays while the table can scroll, so scrolling moves nothing.
import { onBeforeUnmount, onMounted, reactive, ref, useId } from 'vue';

defineProps<{ label: string }>();
const region = ref<HTMLElement | null>(null);
const edge = reactive({ scrolls: false, left: false, right: false });
const cueId = useId();

function measure() {
  const el = region.value;
  if (!el) return;
  const max = el.scrollWidth - el.clientWidth;
  edge.scrolls = max > 1;
  edge.left = max > 1 && el.scrollLeft > 1;
  edge.right = max > 1 && el.scrollLeft < max - 1;
}

let observer: ResizeObserver | null = null;
onMounted(() => {
  measure();
  if (typeof ResizeObserver === 'undefined' || !region.value) return;
  observer = new ResizeObserver(measure);
  observer.observe(region.value);
  for (const child of region.value.children) observer.observe(child);
});
onBeforeUnmount(() => observer?.disconnect());
</script>

<template>
  <p v-if="edge.scrolls" class="scroll-cue small"><span v-if="edge.right" :id="cueId">scroll for more{{ ' ' }}<span aria-hidden="true">→</span></span></p>
  <div class="scroller" :class="{ 'more-left': edge.left, 'more-right': edge.right }">
    <div ref="region" class="scroll" tabindex="0" role="region" :aria-label="label" :aria-describedby="edge.right ? cueId : undefined" @scroll.passive="measure"><slot /></div>
  </div>
</template>
