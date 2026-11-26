# ADR-048: Markdown in messages and formatting buttons

Status: **accepted** (owner, 2026-11-26). Русская версия: [048-markdown.ru.md](048-markdown.ru.md). Refines rule 9 and [ADR-017](017-documents.md), [ADR-044](044-viewing-images.md). Encryption and the protocol do not change.

## Context

The owner wants termchat.app to show Markdown, and the composer to insert it with buttons: bold, italic, strikethrough, code, code block, link. Rule 9 now says "no message markup" and forbids this. The rule was written against HTML, remote images and previews: anything that reaches the network or needs a large rendering engine. Markdown that reads fine unrendered is a different case, but it has risks too. They are reviewed below to decide: everywhere, or Notes only.

## Risk review

A message stays the same `Body::Message` with plain text up to `MAX_TEXT_BYTES` (16,000 bytes). The engine already strips control characters and bidi overrides from it (`clean_text`). Only one thing is new: the recipient shows parts of the text differently. The risks:

| # | Risk | Severity | Mitigation |
|---|---|---|---|
| 1 | **Disguised link:** `[bank](https://evil.example)` shows one thing and leads to another | High, the main risk | The full address is always visible next to the text. The app never follows a link, it only copies the address (§3) |
| 2 | **Opening a link reveals the IP and bypasses Tor:** the browser goes around Tor, the site sees the IP and the time | High | Same: termchat.app opens nothing, as today (the desktop has no eframe `links` feature). The person copies the address and decides |
| 3 | **Remote image** `![](https://…)` as a tracking pixel | High | Not supported, shown as text |
| 4 | **HTML** and dangerous schemes (`javascript:`, `file:`, `intent:`) | High on the web | HTML is not parsed and is shown as text. Schemes do not matter: nothing is opened |
| 5 | **UI spoofing:** a large heading or a quote that looks like a system notice ("Safety number changed") or a reply quote | Medium | No headings. A `>` quote looks different from a reply quote (ADR-025). System events carry a pixel icon, not a text style |
| 6 | **Hidden text** (spoilers, white text, tiny font) | Medium | Not supported: no colours, sizes or spoilers. Every style keeps text visible |
| 7 | **Parser denial of service:** deep nesting, quadratic parsing | Low | Our own linear line-based parser without recursion, text at most 16,000 bytes, a fuzz target |
| 8 | **Terminal:** escape sequences in styles | Low | ratatui applies styles. Raw escape sequences are already removed by `clean_text` |
| 9 | **Metadata** | None | Frame size and kind are unchanged. The relay cannot tell Markdown from text |
| 10 | **Old clients, `recv`, scripts, notifications** | None | They see the source text, which reads fine as is |

**Conclusion.** Nearly all the risk is in links and images. If links are never opened, the address is always visible, and there are no images or HTML, the added danger is small. Notes-only would then be safer only against UI spoofing (§5 of the table), which is closed separately. **Proposed: enable everywhere:** direct chats, groups and Notes.

## Decision

### 1. The subset

The engine parses it (`crates/engine`, new `markdown` module), one implementation for the terminal, desktop and Android:

- `**bold**`, `*italic*` and `_italic_`, `~~strikethrough~~`, `` `code` ``;
- a code block between ```` ``` ```` lines, no highlighting, monospaced, no wrapping inside (horizontal scroll, or truncation with "…" in the terminal);
- a `> ` quote, a one-level `- `/`* ` and `1. ` list;
- a `[text](address)` link;
- `\` escapes the next character.

Not supported and shown as text: headings, tables, images, HTML, footnotes, nested lists, spoilers, colours.

The engine gives clients a list of `(text, style)` spans and blocks. Clients only paint them and parse nothing themselves. Unbalanced markup (`**unpaired`) stays text.

### 2. Where it shows

- Messages in chats and groups, Notes, reply quotes, pinned messages.
- Not formatted: notifications, the chat list, search. They show the source text, parsing there is not needed.
- Copy and forward pass the source text with markup.
- A "Show formatting" setting (on by default). Off shows the source text everywhere.

### 3. Links

- `[text](address)` shows as "text" followed by the full address in a smaller font. In the terminal: `text <address>`.
- A tap (desktop, Android) or selection (terminal) copies the address to the clipboard, with a short "Address copied" hint. No browser opens.
- Bare addresses in text are not changed or highlighted, as today.

### 4. Formatting buttons

- Desktop and Android: a row of buttons above the composer (bold, italic, strikethrough, code, code block, link, quote, list). They wrap the selection in syntax, or insert an empty pair with the cursor inside. "Link" inserts `[selection](` with the cursor at the address. Icons are our own pixel sprites. Labels have no "…" (rule 16).
- Terminal: key shortcuts, no buttons. The exact keys are chosen during implementation, without clashing with current ones.
- The editor only inserts text. What is sent is what is in the field.

### 5. Rule 9

New wording in `CLAUDE.md` and `CONTRIBUTING`:

> "Text and documents (ADR-017, ADR-044, ADR-048). Documents travel as opaque bytes. On desktop and Android a received image can be viewed on an explicit tap; only the engine's Rust decoder with limits decodes it. Text may contain Markdown from the ADR-048 subset, parsed only by the engine. Do not add HTML, images by URL, previews, thumbnails, automatic file opening or following links."

## Consequences and honest limits

- Links are less convenient than in other messengers: they have to be copied. That is the price of termchat.app never reaching the network around the relay and Tor.
- `*` and `_` in plain text (`2*3*4`, `snake_case`) will sometimes turn italic. `\`, backtick code and the §2 setting help. `_` inside a word does not count as italic.
- A recipient on an old version sees the source text. That is fine, Markdown is designed for it.
- A new parser is new attack surface on input from the other side. So it is our own, linear, without `unsafe`, with a fuzz target.

## Rejected

- **Notes only.** The link risk is closed by opening nothing, the rest is small. Limiting it to Notes would gain almost nothing and lose a lot.
- **Opening links after a confirmation.** Reveals the IP around Tor, and one careless tap decides it. Can be revisited in a separate ADR.
- **`pulldown-cmark`.** Well tested, but it is full CommonMark: HTML, images and nesting would have to be turned off and filtered after parsing. For eight elements our own parser is smaller and easier to bound (rule 7).
- **Formatting outside the text** (separate style fields in `Body`, like Telegram entities). Changes the protocol, old clients lose styles, and the source text stops being the truth.

## Dependencies

None new.
