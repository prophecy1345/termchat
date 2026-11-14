// Release notes and articles, shared by the changelog page, the blog and the feed.
import { compiledContent } from "../../CHANGELOG.md";

export type Release = { version: string; html: string; date?: Date };
export type Post = {
  slug: string;
  lang: "en" | "ru";
  title: string;
  description: string;
  date: Date;
  html: string;
};

/** Publication dates from GitHub; without them (offline, rate limit) the notes still build. */
async function dates(): Promise<Record<string, Date>> {
  try {
    const r = await fetch("https://api.github.com/repos/prophecy1345/termchat/releases?per_page=100");
    if (!r.ok) return {};
    const list: { tag_name: string; published_at: string | null; draft: boolean }[] = await r.json();
    return Object.fromEntries(
      list.filter((x) => !x.draft && x.published_at).map((x) => [x.tag_name.replace(/^v/, ""), new Date(x.published_at!)]),
    );
  } catch {
    return {};
  }
}

let cached: Promise<Release[]> | undefined;

/** CHANGELOG.md split at its `## x.y.z` headings, newest first. */
export function releases(): Promise<Release[]> {
  cached ??= (async () => {
    const [html, when] = await Promise.all([compiledContent(), dates()]);
    const parts = html.split(/(?=<h2[^>]*>)/).slice(1);
    return parts.flatMap((part) => {
      const m = part.match(/^<h2[^>]*>([^<]+)<\/h2>/);
      if (!m) return [];
      const version = m[1].trim();
      return [{ version, html: part.slice(m[0].length).trim(), date: when[version] }];
    });
  })();
  return cached;
}

const files = import.meta.glob<{
  frontmatter: { title: string; description: string; date: string; lang: "en" | "ru"; draft?: boolean };
  compiledContent: () => Promise<string>;
}>("./posts/*.md", { eager: true });

/** Articles in `src/posts`, newest first. A draft builds nowhere. */
export async function posts(lang?: "en" | "ru"): Promise<Post[]> {
  const all = await Promise.all(
    Object.entries(files).map(async ([file, m]) => ({
      slug: file.replace(/^.*\//, "").replace(/(\.ru)?\.md$/, ""),
      lang: m.frontmatter.lang,
      title: m.frontmatter.title,
      description: m.frontmatter.description,
      date: new Date(m.frontmatter.date),
      draft: m.frontmatter.draft === true,
      html: await m.compiledContent(),
    })),
  );
  return all
    .filter((p) => !p.draft && (!lang || p.lang === lang))
    .sort((a, b) => b.date.getTime() - a.date.getTime());
}
