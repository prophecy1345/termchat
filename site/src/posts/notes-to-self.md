---
title: "Saved messages that stay yours: Notes in termchat.app 0.33"
description: "termchat.app 0.33 adds Notes, an end-to-end encrypted chat with yourself. Nothing is stored on a server: notes stay on the device and go only to your own linked devices. Tags, filters and favorites keep them from turning into a heap."
date: 2026-11-24
lang: en
---

Everyone keeps a chat with themselves: a link to read later, an address, a draft, a code to copy from the phone to the laptop. In most messengers that chat lives on the company's servers, and after a year it is a heap nobody can find anything in.

Version 0.33 adds **Notes** to termchat.app, built to avoid both problems.

## Nothing on a server

- **On one device, notes never leave it.** They are written into the same encrypted state file as your chats and never go on the wire.
- **With linked devices**, each note, edit, delete and star goes to your other devices, and only to them, as an ordinary end-to-end encrypted message. A contact cannot write into your notes: a note is taken only from one of your own devices.
- **The relay keeps nothing.** It holds ciphertext only until delivery, as for any message, and cannot tell a note from the copies your devices already exchange.

The honest limits: a newly linked device sees notes written after it was linked, not the old ones, and a device offline for more than 14 days misses what was written meanwhile. Documents in Notes come in a later version.

## Keeping Notes in order

The idea comes from bookmark managers such as Raindrop.io, cut down to what a messenger needs.

- **Tags from the text.** Write `#recipes` or `#read-later` anywhere in a note. There is no tag editor and nothing new on the wire: the tag is part of the text.
- **Filters**, one at a time: **All**, **Untagged** (the inbox: notes you have not sorted yet), **Favorites**, **Links**, and one per tag with its count.
- **Favorites.** Star a note to keep it at hand. The star reaches your other devices.

What we did not take: link previews and fetched page titles. Fetching a page would tell the site, and whoever watches the network, what you saved.

## How to use it

- **Desktop and Android:** Notes has its own row above your contacts. Filters are a row of chips over the notes. **Star** is under a note on the desktop and in the long-press menu on Android.
- **Terminal:** Notes is the first row. **Ctrl+P**, then **f** switches the filter; pick a note with **Shift+↑** and press **Ctrl+P**, **s** to star it.

Your other devices need 0.33 or later to receive notes. The design is in [ADR-046](https://github.com/prophecy1345/termchat/blob/main/docs/adr/046-notes-to-self.md); the guide is in [Messages](/docs/messages/).

## Also new since 0.32: introductions

If you skipped 0.32: you can now introduce two of your contacts. The person introduced is asked first, the other decides whether to add them, and nobody can be reached without saying yes. Details in [Share a contact without giving anyone away](/blog/introductions/).
