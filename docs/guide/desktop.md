---
title: "termchat.app desktop app for macOS, Linux and Windows"
description: "The termchat.app desktop app: the same encrypted account in a window, with QR invites, right-click actions on messages, settings, and what a window shows that a terminal does not."
---

# Desktop app

Русская версия: [desktop.ru.md](desktop.ru.md).

`termchat-gui` is termchat.app in a window, for macOS, Linux and Windows: the same account, encryption and relay as the terminal client, with the look of the Android app. [Install](install.md) it with one command or Homebrew.

The terminal client and the desktop app open the same account (`~/.termchat`), but not at the same time: the second one says the account is already open.

## The window

- **Left:** your contacts, with **+ Add**, **Invite**, **Settings** and **Lock** below.
- **Right:** the chat. ⋮ at the top holds **Safety number**, **Rename…**, **No history**, **Block**, **Report…** and **Delete…** (**Reject request…** for a request).
- **Bottom:** the field, **SEND**, **File** and the disappearing-messages timer (**timer off**, then 5m, 1h, 1d, 1w with each click). **Enter** sends, **Shift+Enter** starts a new line.
- **Status bar:** online or connecting, your nickname, warnings.

Double-click a message to answer it. Right-click it to answer, react, and edit or delete your own; see [Messages](messages.md).

## Groups

A small private group of up to 32 people, by invitation only. Nobody can find it.

- **Create:** **+ New** above the list of groups, then a name. You are its admin.
- **Invite:** **Invite** in the group. Only your own contacts, on a version that knows groups; an invited one shows **✓ Invited**.
- **Join:** the invite comes in your chat with the admin, with **Join** and **Decline**.
- **In the group:** each run of messages shows who wrote it, each name in a colour of its own; answers, edits, deletes and reactions work as in a chat. Only the admin sets the timer.
- **Members:** **Members** lists everyone with their fingerprints; the admin removes people there (**Remove…**). The group moves to a new queue; they read nothing from then on.
- **Leave:** **Leave**. The others are told; the chat stays with you, read-only.

A new member does not see what was said before they joined. Groups carry no files yet.

## Invites with a QR code

**Invite** shows your one-time invite as a QR code for the Android app to scan, with **Copy invite** for the text. Under "Other invites" are **Public invite**, **Incognito invite** and **Revoke unused invites**. See [Contacts](contacts.md).

## Settings

**Settings** under the contact list:

- **Appearance:** the theme (the same as in the terminal and on Android), the system sound for new messages, and **Lock after** 1, 5, 15 or 30 minutes.
- **Contacts:** **One connection for all contacts** (see [Tor and bridges](tor.md)).
- **Account and security:** **Turn Tor on/off**, **Back up your identity**, **Change password**, **Set a duress password**, **Tor bridges**, **What's new in this version**.
- **Danger zone:** **Wipe everything**. See [Backups, locking, wiping](safety.md).

## What a window shows

A window shows more to the rest of the computer than a terminal does. Screenshots and screen recording can see it; there is no protection like Android's. The text passes through the window system and the graphics driver. If that matters for a conversation, use the terminal client, and lock the app (**Lock**) when you step away.
