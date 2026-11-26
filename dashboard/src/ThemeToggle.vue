<script setup lang="ts">
import { THEMES, setTheme, theme, type Theme } from './theme.ts';

const LABEL: Record<Theme, string> = { light: 'Light', dark: 'Dark', system: 'System' };

function pick(event: Event) {
  setTheme((event.target as HTMLInputElement).value as Theme);
}
</script>

<template>
  <div class="theme" role="radiogroup" aria-label="Theme">
    <label v-for="t in THEMES" :key="t">
      <input type="radio" name="theme" :value="t" :checked="theme === t" @change="pick" />
      <span>{{ LABEL[t] }}</span>
    </label>
  </div>
</template>
