// Every word on the page, in both languages. Keep in step with README.md and
// README.ru.md: the site must not promise more than they do.

export const REPO = "https://github.com/prophecy1345/termchat";
const RAW = "https://raw.githubusercontent.com/prophecy1345/termchat/main";

export const commands = {
  brew: "brew install prophecy1345/tap/termchat",
  sh: `curl -fsSL ${RAW}/scripts/install.sh | sh`,
  ps: `irm ${RAW}/scripts/install.ps1 | iex`,
};

export type Lang = "en" | "ru";
type Item = [title: string, text: string];

const en = {
  lang: "en",
  title: "termchat: private messages in your terminal and on Android",
  description:
    "An end-to-end encrypted, text-only messenger for the terminal and Android. No accounts, no phone numbers, optional Tor.",
  other: { href: "/ru/", label: "RU" },
  // Documents in this language, relative to the repository root.
  docs: {
    threatModel: "docs/threat-model.md",
    selfHost: "docs/self-host.md",
    verify: "README.md#verify-a-release",
    use: "README.md#acceptable-use",
  },
  nav: { features: "Features", protection: "Protection", install: "Install" },
  tagline: "Private messages, in your terminal and on your phone.",
  lead: "End-to-end encrypted, text only, no accounts. You are a key pair and a nickname you choose; the server only ever sees ciphertext.",
  status: "not audited · experimental",
  cta: { install: "Install", github: "GitHub", download: "Download" },
  whatTitle: "What it is",
  what: [
    ["Text, in a terminal and on Android", "A chat where you already work, and an app for your phone on the same engine. No images, no link previews, no formatting: less to leak, less to exploit."],
    ["No one to sign up with", "No phone number, email or name. Your identity is a key pair made on your device, plus a nickname only your contacts see."],
    ["A server that knows nothing", "The relay passes along envelopes it cannot open. Seizing it gives ciphertext and public keys, nothing more."],
  ] as Item[],
  featuresTitle: "What it can do",
  features: [
    ["End-to-end encryption", "Olm from vodozemac: an X3DH-style handshake and the Double Ratchet, the design behind Signal and Matrix."],
    ["Tor built in", "One switch hides your IP address from the relay and your provider. Where Tor is blocked, obfs4, webtunnel or snowflake bridges get through. No separate Tor install."],
    ["Offline delivery", "Messages wait on the relay, encrypted, for up to 14 days until your contact comes online."],
    ["Disappearing messages", "From 5 minutes to a week, per chat. Or keep a chat in memory only, never on disk."],
    ["Duress password", "A second password that silently erases everything and shows a fresh install."],
    ["Auto-lock", "Keys leave memory after 5 idle minutes, or at once with Ctrl+K."],
    ["Backups and moving", "Back up your identity and restore it on a new device; the old one is wiped without telling anyone."],
    ["Safety numbers", "Compare 60 digits with a friend once and rule out anyone in the middle."],
    ["Requests, not inboxes", "Strangers wait until you accept. Block, reject or report to the relay operator."],
    ["One-time invites", "Each invite works for one person. Revoke the unused ones, or renew a public invite to stop the old link."],
    ["Incognito contacts", "Talk to someone from a separate identity of your own: other keys, another name. Contacts cannot link you, and over Tor neither can the relay."],
    ["Search, on your device", "Find anything in a chat. Nothing is indexed or sent anywhere."],
    ["Android app", "The same engine and protection on your phone: Tor, the duress password, no Google services. Wake-ups through UnifiedPush if you want them."],
    ["Mouse and themes", "Click, scroll, right-click for actions. The same light and dark colour themes in both apps."],
    ["Your own relay", "One small binary with SQLite. Run it on any server, with an onion address if you like."],
  ] as Item[],
  protectionTitle: "What it protects, and what it does not",
  protectionLead: "The goal is not “impossible to break” but “breaking the server is pointless”.",
  protected: {
    title: "Protected",
    items: [
      "Message contents, from your provider, Wi-Fi, the relay operator and anyone who seizes the relay.",
      "Old messages if a key leaks later, and new ones once the attacker loses access.",
      "Tampering, replays and reordering; dropped messages are shown.",
      "Impersonation, once you compare the safety number.",
      "Your IP address, from the relay and your provider, with Tor on.",
      "Your data at rest: encrypted with your password (Argon2id).",
    ],
  },
  partly: {
    title: "Partly",
    items: [
      "Metadata: the relay sees which key writes to which and when, and the size rounded up to 512 / 4096 / 16384 bytes. Incognito contacts over Tor give it pairs of keys it cannot link to you.",
      "Someone at your unlocked computer: auto-lock, disappearing messages, chats without history.",
      "Being forced to give a password: the duress password does not help if the disk was copied first.",
      "A lost device: only with a backup of your identity.",
    ],
  },
  notProtected: {
    title: "Not protected",
    items: [
      "Malware, a keylogger, a compromised terminal or OS.",
      "A global observer correlating traffic timing.",
      "Keys in swap or a hibernation file: use disk encryption.",
    ],
  },
  threatModel: "Full threat model",
  howTitle: "How it works",
  how: [
    ["Exchange invites", "Send your invite, a line starting with termchat:, by any channel. It holds public keys and a code that works once."],
    ["Verify once", "Compare the safety number in person or by phone. Same digits: nobody is in the middle."],
    ["Talk", "Messages travel encrypted through the relay, with a new key for every message."],
  ] as Item[],
  installTitle: "Install",
  installTabs: { unix: "macOS · Linux", windows: "Windows", android: "Android", manual: "Manual" },
  install: {
    brew: "With Homebrew",
    sh: "Or with one command",
    ps: "In PowerShell",
    zip: "Or download the .zip, open it and double-click termchat.exe.",
    android: "Download termchat-…-android-arm64.apk from the releases page and open it; Android asks once to allow installs from your browser. No Google services needed. For messages while the app is closed, install ntfy and turn on wake-ups in the app's settings.",
    manual: "Download the archive for your system from the releases page, check it against SHA256SUMS and run termchat.",
    then: "Then type termchat. The setup screen is already filled in with the public relay.",
    verify: "Verify a release",
    releases: "Releases",
  },
  copy: "copy",
  copied: "copied",
  relayTitle: "Run your own relay",
  relay: "The relay is one binary next to termchat in every release: SQLite, rate limits, an optional onion address. Your friends use your server; nobody else needs to know.",
  relayLink: "Self-hosting guide",
  footer: {
    licence: "AGPL-3.0",
    use: "Acceptable use",
    help: "Need help now? findahelpline.com",
    note: "No cookies, no analytics, no third-party requests.",
    about:
      "termchat is a free, non-commercial, open-source project created and maintained as a personal hobby. It has no advertising, subscriptions, paid features or commercial monetization. Anyone can run their own relay server.",
  },
};

const ru: typeof en = {
  lang: "ru",
  title: "termchat: приватные сообщения в терминале и на Android",
  description:
    "Мессенджер для терминала и Android со сквозным шифрованием и только текстом. Без аккаунтов и номеров телефона, Tor по желанию.",
  other: { href: "/", label: "EN" },
  docs: {
    threatModel: "docs/threat-model.ru.md",
    selfHost: "docs/self-host.ru.md",
    verify: "README.ru.md#проверка-релиза",
    use: "README.ru.md#правила-использования",
  },
  nav: { features: "Возможности", protection: "Защита", install: "Установка" },
  tagline: "Приватная переписка в терминале и в телефоне.",
  lead: "Сквозное шифрование, только текст, никаких аккаунтов. Вы — это пара ключей и ник, который выбираете сами; сервер видит только шифртекст.",
  status: "без аудита · экспериментально",
  cta: { install: "Установить", github: "GitHub", download: "Скачать" },
  whatTitle: "Что это",
  what: [
    ["Текст в терминале и на Android", "Чат там, где вы и так работаете, и приложение для телефона на том же движке. Без картинок, превью ссылок и разметки: меньше утечек и меньше поводов для атак."],
    ["Не нужно регистрироваться", "Ни телефона, ни email, ни имени. Ваша личность — пара ключей, созданная на устройстве, и ник, который видят только ваши контакты."],
    ["Сервер ничего не знает", "Relay передаёт конверты, которые не может открыть. Изъятие сервера даст шифртекст и публичные ключи, не больше."],
  ],
  featuresTitle: "Что умеет",
  features: [
    ["Сквозное шифрование", "Olm из vodozemac: рукопожатие в стиле X3DH и Double Ratchet, та же схема, что в Signal и Matrix."],
    ["Встроенный Tor", "Один переключатель скрывает ваш IP от relay и провайдера. Где Tor заблокирован, помогут мосты obfs4, webtunnel или snowflake. Ставить Tor отдельно не нужно."],
    ["Офлайн-доставка", "Сообщения ждут на relay в зашифрованном виде до 14 дней, пока собеседник не появится в сети."],
    ["Исчезающие сообщения", "От 5 минут до недели, для каждого чата. Или чат только в памяти, без записи на диск."],
    ["Пароль под принуждением", "Второй пароль, который молча стирает всё и показывает чистую установку."],
    ["Автоблокировка", "Ключи уходят из памяти после 5 минут бездействия или сразу по Ctrl+K."],
    ["Копия и переезд", "Сохраните личность и восстановите её на новом устройстве; старое стирается, никого не оповещая."],
    ["Номер безопасности", "Один раз сверьте 60 цифр с другом и исключите посредника."],
    ["Запросы, а не входящие", "Незнакомцы ждут, пока вы их примете. Блокировка, отказ, жалоба оператору relay."],
    ["Одноразовые приглашения", "Каждое приглашение срабатывает для одного человека. Неиспользованные можно отозвать, а публичное — обновить, чтобы старая ссылка перестала работать."],
    ["Инкогнито-контакты", "Общайтесь с человеком от отдельной личности: другие ключи, другое имя. Контакты не смогут вас связать, а через Tor — и relay."],
    ["Поиск на устройстве", "Найдите что угодно в чате. Ничего не индексируется и никуда не отправляется."],
    ["Приложение для Android", "Тот же движок и та же защита в телефоне: Tor, пароль под принуждением, без сервисов Google. Пробуждения через UnifiedPush — по желанию."],
    ["Мышь и темы", "Клики, прокрутка, действия по правой кнопке. Одни и те же светлые и тёмные темы в обоих приложениях."],
    ["Свой relay", "Один небольшой бинарник с SQLite. Запускается на любом сервере, можно с onion-адресом."],
  ],
  protectionTitle: "От чего защищает, а от чего нет",
  protectionLead: "Цель не «невозможно взломать», а «взламывать сервер бессмысленно».",
  protected: {
    title: "Защищено",
    items: [
      "Содержимое сообщений — от провайдера, Wi-Fi, оператора relay и того, кто изымет сервер.",
      "Старые сообщения, если ключ утечёт позже, и новые, когда атакующий потеряет доступ.",
      "Подмена, повтор и перестановка сообщений; пропавшие сообщения видны.",
      "Выдача себя за другого, если вы сверили номер безопасности.",
      "Ваш IP-адрес — от relay и провайдера, когда включён Tor.",
      "Данные на диске: зашифрованы вашим паролем (Argon2id).",
    ],
  },
  partly: {
    title: "Частично",
    items: [
      "Метаданные: relay видит, какой ключ кому пишет и когда, и размер, округлённый до 512 / 4096 / 16384 байт. Инкогнито-контакты через Tor дают ему пары ключей, которые он не может связать с вами.",
      "Человек у вашего разблокированного компьютера: автоблокировка, исчезающие сообщения, чаты без истории.",
      "Принуждение выдать пароль: пароль под принуждением не поможет, если диск скопировали заранее.",
      "Потеря устройства: только при наличии резервной копии.",
    ],
  },
  notProtected: {
    title: "Не защищено",
    items: [
      "Вредоносное ПО, кейлоггер, скомпрометированный терминал или ОС.",
      "Глобальный наблюдатель, сопоставляющий время трафика.",
      "Ключи в swap и файле гибернации: включите шифрование диска.",
    ],
  },
  threatModel: "Полная модель угроз",
  howTitle: "Как это работает",
  how: [
    ["Обменяйтесь приглашениями", "Отправьте приглашение — строку, начинающуюся с termchat:, — любым способом. В нём публичные ключи и одноразовый код."],
    ["Сверьте один раз", "Сравните номер безопасности лично или по телефону. Цифры совпали — посредника нет."],
    ["Переписывайтесь", "Сообщения идут через relay в зашифрованном виде, с новым ключом для каждого сообщения."],
  ],
  installTitle: "Установка",
  installTabs: { unix: "macOS · Linux", windows: "Windows", android: "Android", manual: "Вручную" },
  install: {
    brew: "Через Homebrew",
    sh: "Или одной командой",
    ps: "В PowerShell",
    zip: "Или скачайте .zip, откройте его и дважды щёлкните termchat.exe.",
    android: "Скачайте termchat-…-android-arm64.apk со страницы релизов и откройте его; Android один раз спросит разрешение ставить приложения из браузера. Сервисы Google не нужны. Чтобы сообщения приходили при закрытом приложении, поставьте ntfy и включите пробуждения в настройках.",
    manual: "Скачайте архив для своей системы на странице релизов, сверьте его с SHA256SUMS и запустите termchat.",
    then: "Потом наберите termchat. Экран настройки уже заполнен адресом публичного relay.",
    verify: "Как проверить релиз",
    releases: "Релизы",
  },
  copy: "копировать",
  copied: "скопировано",
  relayTitle: "Свой relay",
  relay: "Relay — один бинарник рядом с termchat в каждом релизе: SQLite, лимиты, onion-адрес по желанию. Ваши друзья пользуются вашим сервером, больше никому знать не нужно.",
  relayLink: "Инструкция по установке",
  footer: {
    licence: "AGPL-3.0",
    use: "Правила использования",
    help: "Нужна помощь? findahelpline.com",
    note: "Без cookies, аналитики и сторонних запросов.",
    about:
      "termchat — бесплатный некоммерческий проект с открытым кодом, который создаётся и поддерживается как личное хобби. В нём нет рекламы, подписок, платных функций и монетизации. Любой может запустить свой relay-сервер.",
  },
};

export const texts = { en, ru };
