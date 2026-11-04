import { renameSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { defineConfig, type Plugin } from 'vite';
import vue from '@vitejs/plugin-vue';
import { viteSingleFile } from 'vite-plugin-singlefile';

const outDir = fileURLToPath(new URL('../crates/llm-gateway/src/', import.meta.url));

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
  plugins: [vue(), viteSingleFile({ removeViteModuleLoader: true }), renameToDashboard()],
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
