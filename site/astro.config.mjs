import { defineConfig } from "astro/config";

export default defineConfig({
  site: "https://termchat.app",
  // GitHub Pages serves the site under /termchat/; locally it is /.
  base: process.env.SITE_BASE || "/",
  // The relay guide moved into the documentation.
  redirects: { "/self-host": "/docs/relay/", "/ru/self-host": "/ru/docs/relay/" },
  // Scripts stay separate files, so the CSP needs no 'unsafe-inline' for them.
  vite: { build: { assetsInlineLimit: 0 } },
});
