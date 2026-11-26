---
title: "Markdown that never opens a link: formatting in termchat.app 0.38"
description: "termchat.app 0.38 shows bold, italic, code, quotes, lists and links in messages. A link always shows its full address, and a click only copies it: the app never opens a browser around Tor."
date: 2026-11-26
lang: en
---

Formatting in a messenger looks harmless, and most of it is. The risky part is links. A label like `[your bank](https://evil.example)` can say one thing and lead to another, and a link that opens a browser sends your IP address to a website, outside Tor, at a moment the sender picked. Remote images in Markdown do the same without a click: a tracking pixel.

termchat.app 0.38 adds Markdown and keeps those doors shut.

## What works

- `**bold**`, `*italic*` or `_italic_`, `~~strikethrough~~`, `` `code` ``;
- a code block between two lines of three backticks, kept unwrapped;
- a `> ` quote, a `- ` or `1. ` list;
- `[label](address)` links;
- a backslash before a mark keeps it as text: `\*not italic\*`.

It works in chats, groups, Notes and reply previews. Headings, tables, images and HTML are not supported and show as the text you typed.

## Links: the address is always visible

A link shows its label followed by the **full address** in smaller text. Clicking or tapping it **copies the address**. termchat.app opens nothing. You see where it leads, and you decide whether to open it, and where. In the terminal, pick the message and press **l** to choose an address to copy.

This is less convenient than a tap that opens a page. It is the price of the app never reaching the network around the relay and Tor. The threat model now says so too: opening a copied address in another app can reveal your IP to that site.

## Built in, not bolted on

- **One parser for all three apps.** The Rust engine reads the Markdown; the terminal, desktop and Android only paint what it returns. It runs in linear time on at most 16,000 bytes, without recursion, and is fuzzed like the protocol parsers.
- **Nothing changes on the wire.** A message is the same encrypted text as before. The relay cannot tell formatting from plain text, and a contact on an older version sees the source, which reads fine as it is.
- **Copy and forward keep the source**, markup included.

The reasoning and the risks are in [ADR-048](https://github.com/prophecy1345/termchat/blob/main/docs/adr/048-markdown.md).

## Writing it

- **Desktop and Android:** a row of buttons above the input wraps the selection, or inserts an empty pair with the cursor inside.
- **Terminal:** **F2** to **F9** insert formatting, **F10** starts a new line.
- **Show formatting** in Settings (on by default) turns display off and shows the source instead.

Also new: copy a whole message with **c** on a picked message in the terminal, or **Copy text** in the desktop's right-click menu.

The guide is in [Messages](/docs/messages/). Update from the [download page](/#install), with Homebrew, or from the notice in the app.
