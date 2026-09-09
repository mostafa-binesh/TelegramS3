import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte()],
  base: '/_admin/',
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'esnext',
    rollupOptions: {
      output: {
        // Avoid shipping tiny Svelte runtime helper chunks for lazy views.
        experimentalMinChunkSize: 2000
      }
    }
  }
});
