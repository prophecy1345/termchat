// One feed: releases and articles. No tracking, no third-party service.
import type { APIRoute } from "astro";
import { posts, releases } from "../releases";

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const cdata = (s: string) => `<![CDATA[${s.replace(/]]>/g, "]]]]><![CDATA[>")}]]>`;

export const GET: APIRoute = async ({ site }) => {
  const base = import.meta.env.BASE_URL.replace(/\/$/, "");
  const abs = (path: string) => new URL(base + path, site).href;
  const items = [
    ...(await releases()).map((r) => ({
      title: `termchat.app ${r.version}`,
      link: abs(`/changelog/#v${r.version}`),
      date: r.date,
      html: r.html,
    })),
    ...(await posts()).map((p) => ({
      title: p.title,
      link: abs(`${p.lang === "en" ? "" : "/ru"}/blog/${p.slug}/`),
      date: p.date,
      html: p.html,
    })),
  ].sort((a, b) => (b.date?.getTime() ?? 0) - (a.date?.getTime() ?? 0));
  const body = `<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:atom="http://www.w3.org/2005/Atom">
<channel>
<title>termchat.app</title>
<link>${abs("/")}</link>
<atom:link href="${abs("/rss.xml")}" rel="self" type="application/rss+xml"/>
<description>Releases and articles of termchat.app, an encrypted messenger without a phone number.</description>
${items
  .map(
    (i) => `<item>
<title>${esc(i.title)}</title>
<link>${i.link}</link>
<guid>${i.link}</guid>
${i.date ? `<pubDate>${i.date.toUTCString()}</pubDate>\n` : ""}<description>${cdata(i.html)}</description>
</item>`,
  )
  .join("\n")}
</channel>
</rss>
`;
  return new Response(body, { headers: { "Content-Type": "application/rss+xml; charset=utf-8" } });
};
