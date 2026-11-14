// Every page in both languages; articles join as they are published.
import type { APIRoute } from "astro";
import { posts } from "../releases";

const pages = ["/", "/compare/", "/faq/", "/self-host/", "/threat-model/", "/changelog/", "/privacy/"];

export const GET: APIRoute = async ({ site }) => {
  const base = import.meta.env.BASE_URL.replace(/\/$/, "");
  const abs = (path: string) => new URL(base + path, site).href;
  const pairs: [string | null, string | null][] = pages.map((p) => [p, `/ru${p}`]);
  const en = await posts("en");
  const ru = await posts("ru");
  if (en.length || ru.length) pairs.push(["/blog/", "/ru/blog/"]);
  const slugs = new Set([...en, ...ru].map((p) => p.slug));
  for (const slug of slugs) {
    pairs.push([
      en.some((p) => p.slug === slug) ? `/blog/${slug}/` : null,
      ru.some((p) => p.slug === slug) ? `/ru/blog/${slug}/` : null,
    ]);
  }
  const url = ([e, r]: [string | null, string | null], loc: string) =>
    `  <url>\n    <loc>${abs(loc)}</loc>\n` +
    (e && r
      ? `    <xhtml:link rel="alternate" hreflang="en" href="${abs(e)}"/>\n    <xhtml:link rel="alternate" hreflang="ru" href="${abs(r)}"/>\n`
      : "") +
    "  </url>";
  const body = [
    '<?xml version="1.0" encoding="UTF-8"?>',
    '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:xhtml="http://www.w3.org/1999/xhtml">',
    ...pairs.flatMap((pair) => pair.filter((p): p is string => p !== null).map((loc) => url(pair, loc))),
    "</urlset>",
    "",
  ].join("\n");
  return new Response(body, { headers: { "Content-Type": "application/xml; charset=utf-8" } });
};
