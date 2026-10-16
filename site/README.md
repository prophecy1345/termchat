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
  with the README and `docs/manual.ru.md`.
- Background animation: `src/scripts/cipher.ts`. It stops when the tab is
  hidden and shows one still frame with `prefers-reduced-motion`.
- Logo: see `LOGO_PROMPT.md`; `public/favicon.svg` is a placeholder until then.

## Publishing (once the repository is public)

The output in `dist/` is plain HTML, CSS and one small script, so any static
host works:

- **GitHub Pages:** a workflow that runs `npm ci && npm run build` in `site/`
  and uploads `site/dist` with `actions/upload-pages-artifact` and
  `actions/deploy-pages`.
- **nginx:** copy `dist/` to the server and serve it as a static root. Send the
  same CSP as an HTTP header too, plus `X-Content-Type-Options: nosniff` and
  `Referrer-Policy: no-referrer`.

The GitHub links on the page point to the repository and only work once it is
public.
