<script setup lang="ts">
import { ref } from 'vue';
import { parse, type Inline } from './markdown.ts';

const props = defineProps<{ markdown: string }>();
const blocks = parse(props.markdown);
const copied = ref<number | null>(null);

async function copy(index: number, code: string) {
  try {
    await navigator.clipboard.writeText(code);
  } catch {
    const area = document.createElement('textarea');
    area.value = code;
    document.body.append(area);
    area.select();
    document.execCommand('copy');
    area.remove();
  }
  copied.value = index;
  setTimeout(() => {
    if (copied.value === index) copied.value = null;
  }, 1500);
}

function key(parts: Inline[]): string {
  return parts.map((part) => part.text).join('');
}
</script>

<template>
  <header class="top">
    <h1>llm-gateway</h1>
    <nav class="nav"><a href="/dashboard">Dashboard</a><a href="/llms.txt">llms.txt</a></nav>
  </header>
  <main class="guide">
    <template v-for="(block, index) in blocks" :key="index">
      <component :is="`h${Math.min(block.level + 1, 6)}`" v-if="block.kind === 'heading' && block.level > 1">
        <template v-for="(part, p) in block.text" :key="p">
          <code v-if="part.kind === 'code'">{{ part.text }}</code>
          <a v-else-if="part.kind === 'link'" :href="part.href">{{ part.text }}</a>
          <template v-else>{{ part.text }}</template>
        </template>
      </component>
      <p v-else-if="block.kind === 'paragraph'">
        <template v-for="(part, p) in block.text" :key="p">
          <code v-if="part.kind === 'code'">{{ part.text }}</code>
          <a v-else-if="part.kind === 'link'" :href="part.href">{{ part.text }}</a>
          <template v-else>{{ part.text }}</template>
        </template>
      </p>
      <div v-else-if="block.kind === 'table'" class="scroll">
        <table>
          <thead>
            <tr>
              <th v-for="(cell, c) in block.head" :key="c" scope="col">{{ key(cell) }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="(row, r) in block.rows" :key="r">
              <td v-for="(cell, c) in row" :key="c">
                <template v-for="(part, p) in cell" :key="p">
                  <code v-if="part.kind === 'code'">{{ part.text }}</code>
                  <template v-else>{{ part.text }}</template>
                </template>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <div v-else-if="block.kind === 'code'" class="command">
        <pre><code>{{ block.code }}</code></pre>
        <button type="button" class="copy" :aria-label="`Copy ${block.lang || 'text'} block`" @click="copy(index, block.code)">
          {{ copied === index ? 'Copied' : 'Copy' }}
        </button>
      </div>
    </template>
  </main>
</template>
