import { defineConfig } from 'vite';

export default defineConfig({
  clearScreen: false,
  esbuild: { jsx: 'automatic' },
  server: {
    port: 1420,
    strictPort: true,
    proxy: { '/api': 'http://127.0.0.1:1430' },
  },
  build: { target: 'es2021', outDir: 'dist', emptyOutDir: true },
});
