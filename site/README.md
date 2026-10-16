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
at https://prophecy1345.github.io/termchat/ on every push to `site/`. Pages
needs a public repository on the free plan, so the workflow skips while the
repository is private. When it goes public:

1. Settings → Pages → Source: **GitHub Actions**.
2. Run the **Pages** workflow once (Actions → Pages → Run workflow).

The build takes the sub-path from GitHub (`SITE_BASE=/termchat`); locally the
site lives at `/`. With a custom domain later, add it in Settings → Pages and
the sub-path goes away by itself.

The GitHub links on the page point to the repository and only work once it is
public.
