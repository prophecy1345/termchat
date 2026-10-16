# termchat website

One static page in English (`/`) and Russian (`/ru/`), built with Astro. No
backend, no cookies, no analytics, no third-party requests: the CSP in
`src/components/Page.astro` allows only the site's own files.

```sh
npm ci
npm run dev       # http://localhost:4321
npm run build     # static files in dist/
npm run preview   # serve dist/
```

- All text lives in `src/i18n.ts`, both languages side by side. Keep it in step
  with `README.md` and `README.ru.md` in the repository root.
- Background animation: `src/scripts/cipher.ts`. It stops when the tab is
  hidden and shows one still frame with `prefers-reduced-motion`.
- Logo: sources in `docs/logo/`; `public/` holds the favicon, `logo.svg` and `apple-touch-icon.png`.

## Publishing

`.github/workflows/pages.yml` builds the site and publishes it to GitHub Pages
on every push to `site/`: https://termchat.app/ (custom domain in Settings →
Pages; DNS at Cloudflare as CNAME records to `prophecy1345.github.io`, DNS only,
not proxied). The build takes its base path from GitHub; without a custom domain
it would be `/termchat/`, locally it is `/`.
