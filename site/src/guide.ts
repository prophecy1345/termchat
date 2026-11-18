// The user guide (docs/guide/): its sections in order, for the menu and the
// pages. A section shows up once its file exists; "relay" is docs/self-host.md.
import type { Lang } from "./i18n";

export interface Section {
  slug: string;
  title: Record<Lang, string>;
  /** For files without front matter (docs/self-host.md is read on GitHub too). */
  description?: Record<Lang, string>;
}

export const sections: Section[] = [
  { slug: "install", title: { en: "Install", ru: "Установка" } },
  { slug: "first-run", title: { en: "First run", ru: "Первый запуск" } },
  { slug: "contacts", title: { en: "Contacts", ru: "Контакты" } },
  { slug: "messages", title: { en: "Messages", ru: "Сообщения" } },
  { slug: "devices", title: { en: "Devices", ru: "Устройства" } },
  { slug: "terminal", title: { en: "Terminal", ru: "Терминал" } },
  { slug: "desktop", title: { en: "Desktop app", ru: "Приложение для компьютера" } },
  { slug: "android", title: { en: "Android", ru: "Android" } },
  { slug: "tor", title: { en: "Tor and bridges", ru: "Tor и мосты" } },
  { slug: "safety", title: { en: "Backups, locking, wiping", ru: "Копии, блокировка, стирание" } },
  { slug: "settings", title: { en: "Settings and data", ru: "Настройки и данные" } },
  { slug: "scripts", title: { en: "Scripts and servers", ru: "Скрипты и серверы" } },
  {
    slug: "relay",
    title: { en: "Run a relay", ru: "Свой relay" },
    description: {
      en: "How to run your own relay for termchat.app: one binary or Docker, SQLite, TLS, an optional onion address over Tor, rate limits and updates. Your friends use your server.",
      ru: "Как поднять свой relay для termchat.app: один бинарный файл или Docker, SQLite, TLS, по желанию onion-адрес в Tor, лимиты и обновления. Ваши друзья пользуются вашим сервером.",
    },
  },
  { slug: "trouble", title: { en: "If something goes wrong", ru: "Если что-то не так" } },
];

/** Markdown modules of the guide, as Astro compiles them. */
interface Md {
  compiledContent: () => Promise<string>;
  frontmatter: { title?: string; description?: string };
}
const guide = import.meta.glob<Md>("../../docs/guide/*.md", { eager: true });
const relay = import.meta.glob<Md>("../../docs/self-host*.md", { eager: true });

/** The file behind a section in one language, if it is written yet. */
export function file(slug: string, lang: Lang): Md | undefined {
  const suffix = lang === "en" ? ".md" : ".ru.md";
  const all = slug === "relay" ? relay : guide;
  const name = slug === "relay" ? `self-host${suffix}` : `${slug}${suffix}`;
  return Object.entries(all).find(([path]) => path.endsWith(`/${name}`))?.[1];
}

/** The sections written in `lang`, in order. */
export const written = (lang: Lang) => sections.filter((s) => file(s.slug, lang));
