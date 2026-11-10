import { defineConfig } from 'vite';

export default defineConfig({
  clearScreen: false,
  esbuild: { jsx: 'automatic' },
  server: {
    port: 1420,
    strictPort: true,
    // TIQUET_API: a second dev server, to try two PCs of one shop side by side
    proxy: { '/api': process.env.TIQUET_API ?? 'http://127.0.0.1:1430' },
  },
  build: { target: 'es2021', outDir: 'dist', emptyOutDir: true },
});
