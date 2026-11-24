---
title: "View received photos safely, without zero-click risk: termchat.app 0.30"
description: "termchat.app 0.30 lets you view a received image on the desktop and Android, decoded by its own memory-safe Rust code and only when you tap View, never by a system decoder. Large encrypted files are now about five times faster: 25 MiB in about 2 minutes."
date: 2026-11-24
lang: en
---

termchat.app 0.29 made large documents possible. The first thing people sent was photos, and the first thing they asked was to see them without saving each one and opening it in another app. Version 0.30 adds that on the desktop and Android, and makes large files about five times faster.

## Why messengers are careful with images

Image decoders are among the most attacked code in any messenger. A malformed picture that crashes the decoder in the right way can run code on the phone, and in a messenger it arrives on its own: anyone who can write to you can feed your decoder, often before you look at the chat. Real attacks went this way: FORCEDENTRY reached iPhones through an image parser, and a bug in libwebp (CVE-2023-4863) affected browsers and apps on every platform.

That is why termchat.app treated every file as opaque bytes and never showed images. Version 0.30 changes that under three conditions that keep the main risk out.

## How viewing works

- **Only on a tap.** A received PNG, JPEG, GIF or WebP gets a **View** button. Nothing is decoded on arrival: no thumbnails in the chat, no previews in notifications, no autoplay. An attack that needed zero clicks now needs you to open the picture.
- **Our own decoder, in memory-safe Rust.** The engine decodes the image with the Rust `image` crate on the desktop and on Android alike. Android's system decoders are not used; the app only gets ready pixels. Memory-safe code rules out the buffer overflows behind the attacks above.
- **Hard limits.** At most 25 MiB in, 8 192 pixels a side, 40 megapixels and 256 MiB of memory for the decoder. A picture made to exhaust memory is refused instead of decoded. The result is scaled to 2 560 pixels on the long side, enough for any screen.

The viewer opens over the chat: zoom with the wheel, the trackpad or a pinch, drag to move, Esc or back closes. Photos from a phone are shown upright; nothing else from their metadata is read. A GIF shows its first frame. The pixels stay in memory only, never in a cache, on disk or in the clipboard, and on Android the app's screenshot block covers the viewer too.

Only accepted contacts can send you files at all, so a stranger cannot reach your decoder. The terminal shows no images.

What we do not claim: a bug in a Rust decoder is far less likely, not impossible. That is why the tap stays, and why the limits exist. The details are in [ADR-044](https://github.com/prophecy1345/termchat/blob/main/docs/adr/044-viewing-images.md) and the [threat model](https://github.com/prophecy1345/termchat/blob/main/docs/threat-model.md).

## Large files five times faster

In 0.29 a 25 MiB document took about ten minutes. Without file storage on the server, every part of a file is an ordinary end-to-end encrypted message, and the relay paced all messages into a queue at one a second.

In 0.30 the relay paces the parts of large documents on a budget of their own, four a second, while text keeps its own budget. The app sends a part every 0.3 s:

- **25 MiB in about 2 minutes**, 4 MiB in about 20 seconds;
- text you send during a transfer is not held up behind it;
- the receiver's queue still holds at most 4 MiB of a transfer at a time, and the queue quota is unchanged.

The relay learns nothing new: parts were already recognisable by their size, and it still does not know who the queues belong to or what the file is. With a relay older than 0.30 the app keeps the old pace.

Also in 0.30: an offer nobody has answered yet no longer holds back the next file to the same person, and a file's buttons sit right under it.

How to send, download and view files in each app: [Messages and documents](/docs/messages/).
