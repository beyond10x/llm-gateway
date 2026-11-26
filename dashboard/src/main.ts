import { createApp } from 'vue';
import doc from 'virtual:ess-ui/document';
import 'virtual:ess-ui/tokens.css';
import Dashboard from './Dashboard.vue';
import EssApp from './essui/components/EssApp.vue';
import { HOSTS } from './hosts.ts';
import './style.css';
import { initTheme } from './theme.ts';

// index.html set data-theme before the first paint; this reads the same choice into the toggle.
initTheme();

// The gateway serves this one bundle for /dashboard and, with its /llms.txt inside a
// <pre id="llms">, for /: the page `setup` of the document renders that text, so the two cannot
// disagree.
const llms = document.getElementById('llms');
if (llms) {
  const markdown = llms.textContent ?? '';
  llms.hidden = true;
  createApp(EssApp, { doc, page: 'setup', hosts: HOSTS, hostProps: { markdown } }).mount('#app');
} else {
  createApp(Dashboard).mount('#app');
}
