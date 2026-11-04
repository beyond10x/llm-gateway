import { createApp } from 'vue';
import App from './App.vue';
import Index from './Index.vue';
import './style.css';

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
