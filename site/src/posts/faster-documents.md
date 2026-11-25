---
title: "Encrypted files, much faster: termchat.app 0.34 and 0.35"
description: "termchat.app 0.34 sends a 1 MiB document in under 10 seconds instead of about 3 minutes, with the same end-to-end encryption. 0.34.1 fixes 13 findings of a code review; 0.35 lets you pick several messages and delete them on your device."
date: 2026-11-25
lang: en
---

A photo of 900 KiB used to take about three minutes to send in termchat.app. Not because of encryption or the network: because of the pace we set ourselves, to keep one sender from flooding someone else's queue on the relay. Version 0.34 changes that pace, and nothing about the encryption.

## How much faster

- **1 MiB:** under 10 seconds, was about 3 minutes.
- **25 MiB:** under a minute, was about 2 minutes, on a link of about 4.5 Mbit/s each way.

The new pace needs a relay of 0.34 or later; the public relay runs it. On an older self-hosted relay the app keeps the old pace, so it never runs into the relay's limits.

## Why it is safe to go faster

Every chunk is still encrypted with its own Olm message, padded to the same fixed sizes, and checked before it is kept. What protects a queue from flooding is its quota (10 MiB, 1000 envelopes) and the relay's own limits, not the pace: the pace only decided how soon the quota could fill. Document chunks now have their own budget on the relay (60 at once, then 10 a second), apart from text, so a document never slows down the messages in the same chat.

The honest limit: the app keeps one chunk in flight per queue and sends the next once the relay took it. Over a slow round trip, or through Tor, the round trip sets the speed, not the budget: about 5 chunks a second at 200 ms. The design and what we rejected are in [ADR-047](https://github.com/prophecy1345/termchat/blob/main/docs/adr/047-faster-documents.md).

## 0.34.1: 13 findings of a code review, all fixed

termchat.app has had no independent audit yet. A review of the 0.34 code found 13 problems, none in the encryption itself. Among them:

- A backup carried group history, drafts and unsent messages. It now holds only what a restore needs.
- A message that arrived while the disk could not save it could be lost. The relay now keeps it until a save works.
- A large document stalled for good if the connection dropped or either app restarted in the middle. It now goes on from where it stopped.
- Restore accepted any file and left a state that never opened. It now refuses a file that is not a backup.
- The release workflow published Linux programs and the Android app from other builds than the two it compared byte for byte. It now publishes exactly the compared builds, ARM Linux included.

## 0.35: pick and delete

- **Delete several messages at once** on this device: **Select** in a message's menu on the desktop and Android, **Space** and **x** in the terminal. The other side and your other devices keep theirs.
- **Android:** a long message no longer fills the screen while you type it.
- **Terminal:** the colour theme now colours the whole screen.
- **Alerts:** `termchat hook --quiet 22:00-08:00` holds monitoring alerts at night, lets the critical ones through, and sends the rest as one summary in the morning. See [Scripts and servers](/docs/scripts/).

Update from the [download page](/#install), Homebrew, or the app's own update notice.
