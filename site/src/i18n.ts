// Every word on the page, in both languages. Keep in step with README.md and
// README.ru.md: the site must not promise more than they do.

export const REPO = "https://github.com/prophecy1345/termchat";
export const SPONSOR = "https://github.com/sponsors/prophecy1345";
const RAW = "https://raw.githubusercontent.com/prophecy1345/termchat/main";

export const commands = {
  brew: "brew install prophecy1345/tap/termchat",
  sh: `curl -fsSL ${RAW}/scripts/install.sh | sh`,
  ps: `irm ${RAW}/scripts/install.ps1 | iex`,
  brewGui: "brew install prophecy1345/tap/termchat-gui",
  shGui: `curl -fsSL ${RAW}/scripts/install.sh | TERMCHAT_GUI=1 sh`,
  psGui: `$env:TERMCHAT_GUI = 1; irm ${RAW}/scripts/install.ps1 | iex`,
};

export type Lang = "en" | "ru";
type Item = [title: string, text: string];

const en = {
  lang: "en",
  title: "termchat.app: encrypted messenger without a phone number, for terminal, desktop and Android",
  description:
    "A free, open-source end-to-end encrypted messenger for text and documents: in the terminal, as a desktop app and on Android. No phone number, no accounts, Tor built in, run your own relay.",
  other: { href: "/ru/", label: "RU" },
  // Documents in this language, relative to the repository root.
  docs: {
    threatModel: "docs/threat-model.md",
    selfHost: "docs/self-host.md",
    // A page of the site, not a file in the repository.
    verify: "/docs/install/#check-that-a-download-is-genuine",
    use: "README.md#acceptable-use",
  },
  nav: { features: "Features", desktop: "Desktop", protection: "Protection", install: "Install", docs: "Docs", blog: "Blog" },
  tagline: "An encrypted messenger without a phone number: in your terminal, in a window and on your phone.",
  lead: "End-to-end encrypted text and documents, no accounts. You are a key pair and a nickname you choose; the server only ever sees ciphertext.",
  status: "not audited · experimental",
  cta: { install: "Install", github: "GitHub", download: "Download", sponsor: "Sponsor" },
  whatTitle: "What it is",
  what: [
    ["Text and documents, in a terminal, a window and on Android", "A chat where you already work, a desktop app if you prefer a window, and an app for your phone, all on the same engine. Files are opaque bytes, never opened or previewed; no inline images, no link previews, no formatting: less to leak, less to exploit."],
    ["No one to sign up with", "No phone number, email or name. Your identity is a key pair made on your device, plus a nickname only your contacts see."],
    ["A server that knows nothing", "The relay passes along envelopes it cannot open, between anonymous queues. Seizing it gives ciphertext and random queue IDs, nothing more."],
  ] as Item[],
  featuresTitle: "What it can do",
  features: [
    ["Private by design", [
      ["End-to-end encryption", "Olm from vodozemac, the library behind Matrix: an X3DH-style handshake and the Double Ratchet, the design behind Signal. No cryptography of our own."],
      ["The relay does not know who talks to whom", "It sees anonymous queues, not people: no accounts, no keys, no senders. Each contact writes to a queue made just for them."],
      ["Tor built in", "One switch hides your IP address from the relay and your provider, with a circuit per contact. Where Tor is blocked, obfs4, webtunnel or snowflake bridges get through."],
      ["Your own relay", "One small binary with SQLite, on any server, with an onion address if you like. It stores queues and ciphertext, nothing about people. Your queues stay on it while friends keep theirs on the public relay, so no single server sees both sides of a conversation."],
    ]],
    ["Contacts on your terms", [
      ["Invites, not a directory", "Nobody can find you: people join with an invite you give them, one-time or public, and wait as a request until you accept. Block, reject or report."],
      ["Safety numbers", "Compare 60 digits with a friend once and rule out anyone in the middle."],
      ["Incognito contacts", "Talk to someone from a separate identity of your own: other keys, another name. Your contacts cannot link you to each other."],
      ["Small private groups", "Up to 32 people, by the admin's invitation only, and nobody can find the group. Megolm from the same vodozemac; new members do not read the old history, removed ones not the new."],
    ]],
    ["If your device is in the wrong hands", [
      ["Duress password", "A second password that silently erases everything and shows a fresh install."],
      ["Auto-lock and disappearing messages", "Keys leave memory after 1 to 30 idle minutes, or at once with Ctrl+K. Messages can vanish after 5 minutes to a week, or never touch the disk."],
      ["Backups and moving", "Back up your identity and restore it on a new device; the old one is wiped without telling anyone."],
    ]],
    ["Where you work", [
      ["Terminal, desktop and Android", "The same engine in a terminal, a window on macOS, Linux and Windows, and an app for your phone without Google services. Mouse, themes and UnifiedPush wake-ups if you want them."],
      ["Scripts and servers", "termchat send and recv from a shell, cron or CI: get \"backup failed\" on your phone, end to end encrypted, without a bot account or a third party reading it."],
      ["Answers, edits and reactions", "Answer a particular message, edit or delete your own for both of you, react with one of seven pixel sprites. All inside the encryption; the relay sees an ordinary message."],
      ["Documents, offline, searchable", "Files up to 1 MiB, never opened by the app. Messages wait on the relay for up to 14 days. Search runs on your device only."],
    ]],
  ] as [string, Item[]][],
  desktopTitle: "termchat.app in a window",
  desktopLead: "Not everyone likes a terminal. The desktop app for macOS, Linux and Windows is the same messenger in a window: the same account, encryption and relay, with the look of the Android app and this site's synthwave.",
  desktopAlt: "The termchat.app desktop app: a contact list on the left and a chat with alice on the right",
  desktop: [
    ["Everything the terminal client does", "Requests, chats, disappearing messages, documents, the safety number, backups, the duress password, Tor and bridges, wipe."],
    ["Invites with a QR code", "Show your invite on screen and scan it with the Android app, or copy the text."],
    ["One account, one client at a time", "The terminal client and the app open the same account, but not together: the second one says it is already open."],
    ["Honest about a window", "Screenshots and screen recording can see it, and the text passes through the window system and the graphics driver. A terminal shows less."],
  ] as Item[],
  desktopInstall: "How to install",
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
      "Metadata: the relay knows anonymous queues, not who writes to whom; it sees when each queue is written and read, and the size rounded up to 512 / 4096 / 16384 bytes. Each contact gets a connection of its own; over Tor, a circuit of its own.",
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
  installTabs: { unix: "macOS · Linux", windows: "Windows", desktop: "Desktop app", android: "Android", manual: "Manual" },
  install: {
    desktopBrew: "macOS, Linux: with Homebrew",
    desktopSh: "Or with one command: termchat.app goes to ~/Applications on macOS, into the app menu on Linux",
    desktopPs: "Windows, in PowerShell: with a Start menu entry",
    desktopNote: "Not signed with a paid Apple or Microsoft certificate: installed these ways it opens straight away. Downloaded through a browser, macOS asks once (System Settings → Privacy & Security → Open Anyway), and so does Windows SmartScreen. Linux needs glibc 2.28 or newer and X11 or Wayland.",
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
  relay: "The relay is one binary next to termchat.app in every release: SQLite, rate limits, an optional onion address. Your queues stay on your server and friends on the public relay keep theirs there, so neither server sees both sides of a conversation. Or your friends join your server too.",
  relayLink: "Self-hosting guide",
  compareLink: "Compared with Signal, Session, SimpleX and Briar",
  sponsorTitle: "Support the project",
  sponsor: "termchat.app is free and stays free: no ads, no paid features, no data to sell. It is built by one person. Donations through GitHub Sponsors let me keep working on it, and pay for the public relay server, signing certificates for the apps and, some day, an independent security audit. A sponsor gets nothing extra in the app and no one is treated differently: thank you is all it buys.",
  sponsorLink: "Sponsor on GitHub",
  footer: {
    licence: "AGPL-3.0",
    use: "Acceptable use",
    privacy: "Privacy",
    compare: "Compare",
    faq: "FAQ",
    threatModel: "Threat model",
    changelog: "Changelog",
    sponsor: "Sponsor",
    help: "Need help now? findahelpline.com",
    note: "No cookies, no analytics, no third-party requests.",
    about:
      "termchat.app is a free, non-commercial, open-source project created and maintained as a personal hobby. It has no advertising, subscriptions, paid features or commercial monetization; voluntary donations cover its costs. Anyone can run their own relay server.",
  },
};

const ru: typeof en = {
  lang: "ru",
  title: "termchat.app: зашифрованный мессенджер без номера телефона для терминала, компьютера и Android",
  description:
    "Бесплатный мессенджер с открытым кодом и сквозным шифрованием для текста и документов: в терминале, в приложении для компьютера и на Android. Без номера телефона и аккаунтов, Tor встроен, свой relay по желанию.",
  other: { href: "/", label: "EN" },
  docs: {
    threatModel: "docs/threat-model.ru.md",
    selfHost: "docs/self-host.ru.md",
    verify: "/ru/docs/install/#как-проверить-что-загрузка-подлинная",
    use: "README.ru.md#правила-использования",
  },
  nav: { features: "Возможности", desktop: "Приложение", protection: "Защита", install: "Установка", docs: "Документация", blog: "Блог" },
  tagline: "Зашифрованный мессенджер без номера телефона: в терминале, в окне и в телефоне.",
  lead: "Текст и документы со сквозным шифрованием, никаких аккаунтов. Вы — это пара ключей и ник, который выбираете сами; сервер видит только шифртекст.",
  status: "без аудита · экспериментально",
  cta: { install: "Установить", github: "GitHub", download: "Скачать", sponsor: "Поддержать" },
  whatTitle: "Что это",
  what: [
    ["Текст и документы в терминале, в окне и на Android", "Чат там, где вы и так работаете, приложение с окном, если оно вам удобнее, и приложение для телефона — всё на одном движке. Файлы — непрозрачные байты, приложение их не открывает и не показывает превью; без картинок в чате, превью ссылок и разметки: меньше утечек и меньше поводов для атак."],
    ["Не нужно регистрироваться", "Ни телефона, ни email, ни имени. Ваша личность — пара ключей, созданная на устройстве, и ник, который видят только ваши контакты."],
    ["Сервер ничего не знает", "Relay передаёт конверты, которые не может открыть, между анонимными очередями. Изъятие сервера даст шифртекст и случайные ID очередей, не больше."],
  ],
  featuresTitle: "Что умеет",
  features: [
    ["Приватность по устройству", [
      ["Сквозное шифрование", "Olm из vodozemac, библиотеки Matrix: рукопожатие в стиле X3DH и Double Ratchet, та же схема, что в Signal. Никакой своей криптографии."],
      ["Relay не знает, кто кому пишет", "Он видит анонимные очереди, а не людей: ни аккаунтов, ни ключей, ни отправителей. Каждый контакт пишет в очередь, созданную специально для него."],
      ["Встроенный Tor", "Один переключатель скрывает ваш IP от relay и провайдера, с отдельной цепочкой на каждый контакт. Где Tor заблокирован, помогут мосты obfs4, webtunnel или snowflake."],
      ["Свой relay", "Один небольшой бинарник с SQLite, на любом сервере, можно с onion-адресом. Хранит очереди и шифртекст, ничего о людях. Ваши очереди остаются на нём, а очереди друзей — на публичном relay, так что ни один сервер не видит обе стороны разговора."],
    ]],
    ["Контакты на ваших условиях", [
      ["Приглашения вместо каталога", "Найти вас нельзя: люди приходят по приглашению, которое вы дали, одноразовому или публичному, и ждут как запрос, пока вы не примете. Блокировка, отказ, жалоба."],
      ["Номер безопасности", "Один раз сверьте 60 цифр с другом и исключите посредника."],
      ["Инкогнито-контакты", "Общайтесь с человеком от отдельной личности: другие ключи, другое имя. Контакты не смогут связать вас между собой."],
      ["Маленькие приватные группы", "До 32 человек, только по приглашению администратора, найти группу нельзя. Megolm из той же vodozemac; новые участники не читают старую историю, исключённые — новую."],
    ]],
    ["Если устройство попало в чужие руки", [
      ["Пароль под принуждением", "Второй пароль, который молча стирает всё и показывает чистую установку."],
      ["Автоблокировка и исчезающие сообщения", "Ключи уходят из памяти после 1–30 минут бездействия или сразу по Ctrl+K. Сообщения исчезают через 5 минут – неделю или вовсе не попадают на диск."],
      ["Копия и переезд", "Сохраните личность и восстановите её на новом устройстве; старое стирается, никого не оповещая."],
    ]],
    ["Там, где вы работаете", [
      ["Терминал, компьютер и Android", "Один движок в терминале, в окне на macOS, Linux и Windows и в приложении для телефона без сервисов Google. Мышь, темы и пробуждения через UnifiedPush — по желанию."],
      ["Скрипты и серверы", "termchat send и recv из shell, cron или CI: «бэкап не прошёл» приходит вам на телефон со сквозным шифрованием, без бота с аккаунтом и без третьей стороны, которая это читает."],
      ["Ответы, правка и реакции", "Ответ на конкретное сообщение, правка и удаление своих у обоих, реакция одним из семи пиксельных спрайтов. Всё внутри шифрования; relay видит обычное сообщение."],
      ["Документы, офлайн, поиск", "Файлы до 1 МиБ, приложение их не открывает. Сообщения ждут на relay до 14 дней. Поиск работает только на вашем устройстве."],
    ]],
  ],
  desktopTitle: "termchat.app в окне",
  desktopLead: "Терминал нравится не всем. Приложение для macOS, Linux и Windows — тот же мессенджер в окне: тот же аккаунт, шифрование и relay, в стиле приложения для Android и синтвейва этого сайта.",
  desktopAlt: "Приложение termchat.app: список контактов слева и чат с alice справа",
  desktop: [
    ["Всё, что умеет терминальный клиент", "Запросы, чаты, исчезающие сообщения, документы, номер безопасности, резервные копии, пароль под принуждением, Tor и мосты, стирание."],
    ["Приглашения с QR-кодом", "Покажите приглашение на экране и отсканируйте его приложением для Android или скопируйте текст."],
    ["Один аккаунт, один клиент за раз", "Терминальный клиент и приложение открывают один аккаунт, но не одновременно: второй скажет, что он уже открыт."],
    ["Честно про окно", "Скриншоты и запись экрана его видят, а текст проходит через оконную систему и графический драйвер. Терминал показывает меньше."],
  ],
  desktopInstall: "Как установить",
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
      "Метаданные: relay знает анонимные очереди, а не то, кто кому пишет; он видит, когда в очередь пишут и когда её читают, и размер, округлённый до 512 / 4096 / 16384 байт. У каждого контакта своё соединение, через Tor — своя цепочка.",
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
  installTabs: { unix: "macOS · Linux", windows: "Windows", desktop: "Приложение", android: "Android", manual: "Вручную" },
  install: {
    desktopBrew: "macOS, Linux: через Homebrew",
    desktopSh: "Или одной командой: termchat.app окажется в ~/Applications на macOS и в меню приложений на Linux",
    desktopPs: "Windows, в PowerShell: с пунктом в меню «Пуск»",
    desktopNote: "Не подписано платным сертификатом Apple или Microsoft: установленное так, оно открывается сразу. Если скачать через браузер, macOS один раз спросит («Системные настройки» → «Конфиденциальность и безопасность» → «Всё равно открыть»), Windows SmartScreen тоже. На Linux нужен glibc 2.28 или новее и X11 или Wayland.",
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
  relay: "Relay — один бинарник рядом с termchat.app в каждом релизе: SQLite, лимиты, onion-адрес по желанию. Ваши очереди остаются на вашем сервере, а друзья на публичном relay держат свои там, так что ни один сервер не видит обе стороны разговора. Или друзья тоже переходят на ваш сервер.",
  relayLink: "Свой relay: инструкция",
  compareLink: "Сравнение с Signal, Session, SimpleX и Briar",
  sponsorTitle: "Поддержать проект",
  sponsor: "termchat.app бесплатный и останется бесплатным: без рекламы, платных функций и данных на продажу. Его делает один человек. Пожертвования через GitHub Sponsors позволяют мне продолжать над ним работать и оплачивают публичный relay, сертификаты для подписи приложений, а когда-нибудь и независимый аудит безопасности. Спонсор не получает в приложении ничего сверх остальных и ни к кому не относятся иначе: это просто спасибо.",
  sponsorLink: "Поддержать на GitHub",
  footer: {
    licence: "AGPL-3.0",
    use: "Правила использования",
    privacy: "Конфиденциальность",
    compare: "Сравнение",
    faq: "Вопросы",
    threatModel: "Модель угроз",
    changelog: "История версий",
    sponsor: "Поддержать",
    help: "Нужна помощь? findahelpline.com",
    note: "Без cookies, аналитики и сторонних запросов.",
    about:
      "termchat.app — бесплатный некоммерческий проект с открытым кодом, который создаётся и поддерживается как личное хобби. В нём нет рекламы, подписок, платных функций и монетизации; расходы покрывают добровольные пожертвования. Любой может запустить свой relay-сервер.",
  },
};

export const texts = { en, ru };
