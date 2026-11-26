import { createApp } from 'vue';
import App from './App.vue';
import Index from './Index.vue';
import './style.css';
import { initTheme } from './theme.ts';

// index.html set data-theme before the first paint; this reads the same choice into the toggle.
initTheme();

// The gateway serves this one bundle for /dashboard and, with its /llms.txt inside a
// <pre id="llms">, for /: the index renders that text, so the two cannot disagree.
const llms = document.getElementById('llms');
if (llms) {
  const markdown = llms.textContent ?? '';
  llms.hidden = true;
  createApp(Index, { markdown }).mount('#app');
} else {
  createApp(App).mount('#app');
}
