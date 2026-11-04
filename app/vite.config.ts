import { defineConfig } from 'vitest/config';

export default defineConfig({
  server: { port: 1420, strictPort: true },
  clearScreen: false,
  build: { assetsInlineLimit: 0 },
  test: { environment: 'jsdom' },
});
