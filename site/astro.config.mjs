import { defineConfig } from "astro/config";

export default defineConfig({
  site: "https://termchat.app",
  // GitHub Pages serves the site under /termchat/; locally it is /.
  base: process.env.SITE_BASE || "/",
  // Scripts stay separate files, so the CSP needs no 'unsafe-inline' for them.
  vite: { build: { assetsInlineLimit: 0 } },
});
