import { readFileSync, renameSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { defineConfig, type Plugin } from 'vite';
import vue from '@vitejs/plugin-vue';
import { viteSingleFile } from 'vite-plugin-singlefile';
import { loadDocument, tokensCss } from './src/essui/load.ts';

const outDir = fileURLToPath(new URL('../crates/llm-gateway/src/', import.meta.url));
const UI_DOCUMENT = fileURLToPath(new URL('./ui/llm-gateway.ui.yaml', import.meta.url));

// The ess-ui document as two modules: `virtual:ess-ui/document`, the loaded document (a document
// the renderer cannot draw fails here), and `virtual:ess-ui/tokens.css`, its tokens and themes as
// custom properties. Both end up inside the one page.
function essUi(): Plugin {
  const DOCUMENT = 'virtual:ess-ui/document';
  const TOKENS = 'virtual:ess-ui/tokens.css';
  return {
    name: 'ess-ui-document',
    resolveId(id) {
      return id === DOCUMENT || id === TOKENS ? `\0${id}` : null;
    },
    load(id) {
      if (id !== `\0${DOCUMENT}` && id !== `\0${TOKENS}`) return null;
      this.addWatchFile(UI_DOCUMENT);
      const doc = loadDocument(readFileSync(UI_DOCUMENT, 'utf8'));
      return id === `\0${DOCUMENT}` ? `export default ${JSON.stringify(doc)};` : tokensCss(doc);
    },
  };
}

function renameToDashboard(): Plugin {
  return {
    name: 'rename-to-dashboard-html',
    apply: 'build',
    closeBundle() {
      renameSync(`${outDir}index.html`, `${outDir}dashboard.html`);
    },
  };
}

export default defineConfig({
  base: './',
  plugins: [essUi(), vue(), viteSingleFile({ removeViteModuleLoader: true }), renameToDashboard()],
  server: {
    host: '127.0.0.1',
    port: 5174,
    strictPort: true,
    proxy: {
      '/dashboard/': { target: 'http://127.0.0.1:8080' },
    },
  },
  preview: {
    host: '127.0.0.1',
    port: 4174,
    strictPort: true,
  },
  build: {
    outDir,
    emptyOutDir: false,
    copyPublicDir: false,
    reportCompressedSize: false,
  },
});
