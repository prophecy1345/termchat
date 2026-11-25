---
title: "Scheduled messages with no server in on it: Send later in termchat.app 0.36"
description: "termchat.app 0.36 lets you write a message now and send it later, from 10 minutes to a day. The scheduled message waits only on your device, encrypted; the relay learns about it only when it goes, as an ordinary message."
date: 2026-11-25
lang: en
---

Most messengers that schedule messages do it on their servers: you hand over the text now, and the company's machine sends it at the time you chose. That means the server holds your message, readable or not, for hours before anyone receives it, and knows when you meant to say it.

termchat.app 0.36 adds **Send later** without any of that.

## How it works

- **Write the message, pick when:** in 10 minutes, 1, 3 or 8 hours, or a day.
- **It waits on your device only**, inside the same encrypted state file as your chats. Nothing goes to the relay until the time comes.
- **When it is due**, the app sends it as an ordinary end-to-end encrypted message. To the relay and to the other side it is indistinguishable from one you just typed.
- **Until then you can cancel it.** What waits is listed over the message field.

The honest limit: since no server sends it for you, your device has to. The message goes while the app is unlocked, open or in the background on Android. If the app is locked at that moment, the message goes at the next unlock. A chat set to keep no history keeps nothing for later either, and a backup never includes scheduled messages.

## Where to find it

- **Android:** long-press **Send**.
- **Desktop:** right-click **SEND**.
- **Terminal:** type the message, then **Ctrl+P**, **u** and a number.

It works in chats, groups and Notes. The guide is in [Messages](/docs/messages/).

## Also in 0.36

Unpicking the last selected message now ends selecting, instead of leaving **Delete here** and **Cancel** on screen. Selecting several messages to delete on your device came in 0.35; see [Encrypted files, much faster](/blog/faster-documents/).

Update from the [download page](/#install), Homebrew, or the app's own update notice.
