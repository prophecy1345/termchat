import { defineConfig } from "astro/config";

export default defineConfig({
  // Scripts stay separate files, so the CSP needs no 'unsafe-inline' for them.
  vite: { build: { assetsInlineLimit: 0 } },
});
