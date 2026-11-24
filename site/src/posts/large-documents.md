---
title: "Send large files end to end encrypted, only when they ask: termchat.app 0.29"
description: "termchat.app 0.29 sends documents of up to 25 MiB end to end encrypted, with no file storage on the server. Above 1 MiB the receiver gets an offer first and downloads only what they want. How it works and what the relay still sees (ADR-043)."
date: 2026-11-24
lang: en
---

Until now termchat.app sent documents of up to 1 MiB. That covers a config, a key or a contract draft, but not a scanned passport, a photo straight off a phone or a slide deck. People then left for a less private channel to send the file. Version 0.29 raises the limit to **25 MiB**, and changes who decides that a large file travels: the receiver.

## Offered first, downloaded on request

Up to 1 MiB nothing changes: the file goes at once.

Above 1 MiB the sender's app sends a short **offer**: the file's name and size, end to end encrypted like a message. The receiver sees it in the chat as "offered" and chooses:

- **Download**: the file comes, with progress on both sides;
- **Decline**: the sender's copy is deleted and both chats say "not taken";
- nothing: the offer is dropped after 7 days.

So nobody gets 25 MiB they did not ask for, on mobile data or a full phone. In the terminal **Ctrl+G** downloads (type `DISCARD` to decline); the desktop app and Android have **Download** and **Decline** buttons under the file.

## No file storage on the server

Most messengers send large files the same way: the file is encrypted with a one-off key, uploaded to the server's file storage, and the key goes in a message. That is fast, but it needs a new encryption format, storage and an upload API on the server.

termchat.app keeps its rule of no homemade cryptography: every protocol comes from [vodozemac](https://github.com/matrix-org/vodozemac), and vodozemac has no attachment encryption. So a large file travels like everything else, as ordinary Olm messages:

- the file is cut into parts of 64 000 bytes, each one an end-to-end encrypted message;
- the relay queues them like any message and deletes each one once delivered;
- the receiver confirms every 16 parts; the sender keeps at most 64 parts (4 MiB) unconfirmed, so a transfer never fills the receiver's queue and text still gets through;
- both devices can go offline and come back: the transfer resumes where it stopped.

While a file travels, both apps keep its bytes in a spool next to the encrypted state file, sealed with the same key. Wiping the app, the duress password, deleting the chat or the 7-day expiry remove it. Nothing is ever opened: termchat.app does not preview, render or open files, and saves them only when you ask, never overwriting anything.

## What it costs

- **Speed.** 25 MiB takes about **10 minutes**, 4 MiB about a minute and a half. Without file storage the relay paces messages per queue (one a second), and that is the price.
- **Metadata.** Parts travel in a 64 KB size class used for nothing else, so the relay can tell that a large document is moving between two queues, roughly how big it is (to about 64 KB) and when the receiver asked for it. It still does not know who the two queues belong to, the file's name or what is in it. This is in the [threat model](https://github.com/prophecy1345/termchat/blob/main/docs/threat-model.md).
- **Versions.** Both sides need 0.29 and their relay too. To an older contact the app sends up to 1 MiB, as before.
- **Groups** stay at 1 MiB for now: a group shares one queue, so parts for one member would reach all of them.

## No image previews, on purpose

We considered showing a small preview of an image before download. We decided against it: image decoders are where zero-click attacks on messengers have come from, and anyone who can message you could feed yours. A document in termchat.app stays opaque bytes. (Update: since 0.30 a received image can be viewed on an explicit tap, decoded by our own Rust code: [how and why](/blog/view-images/).)

The decision and its details are in [ADR-043](https://github.com/prophecy1345/termchat/blob/main/docs/adr/043-large-documents.md). How to send and save files in each app: [Messages and documents](/docs/messages/).
