---
title: "termchat.app desktop app for macOS, Linux and Windows"
description: "The termchat.app desktop app: the same encrypted account in a window, with QR invites, right-click actions on messages, settings, and what a window shows that a terminal does not."
---

# Desktop app

Русская версия: [desktop.ru.md](desktop.ru.md).

`termchat-gui` is termchat.app in a window, for macOS, Linux and Windows: the same account, encryption and relay as the terminal client, with the look of the Android app. [Install](install.md) it with one command or Homebrew.

The terminal client and the desktop app open the same account (`~/.termchat`), but not at the same time: the second one says the account is already open.

## The window

- **Left:** archived chats, your contacts and your groups, each under a heading that folds on a click (**+** beside Groups makes a new one), with **Add**, **Invite**, **Settings** and **Lock** below.
- **Right:** the chat. ⋮ at the top holds **Search**, **Safety number**, **Rename…**, **No history**, **Block**, **Report…** and **Delete…** (**Reject request…** for a request).
- **Bottom:** the field, **SEND**, **File** and the disappearing-messages timer (**timer off**, then 5m, 1h, 1d, 1w with each click). **Enter** sends, **Shift+Enter** starts a new line.
- **Status bar:** online or connecting, your nickname, warnings.
- **Keys:** **Cmd+K** (Ctrl+K on Linux and Windows) goes to a chat or group by part of its name; **F1** lists all the shortcuts.
- **Unread:** the number of unread messages, in chats and groups that are not muted, shows on the Dock icon on macOS and in the window title elsewhere. Only the number, never a name.

Double-click a message to answer it. Right-click it to answer, react, and edit or delete your own; see [Messages](messages.md).

## Groups

A small private group of up to 32 devices (32 people with one each; fewer when members use several), by invitation only. Nobody can find it.

- **Create:** **+** beside the Groups heading, then a name. You are its admin.
- **Invite:** **Invite** in the group. Only your own contacts, on a version that knows groups; an invited one shows **Invited** with a check mark.
- **Join:** the invite comes in your chat with the admin, with **Join** and **Decline**.
- **In the group:** each run of messages shows who wrote it, each name in a colour of its own; answers, edits, deletes and reactions work as in a chat. Only the admin sets the timer.
- **Members:** **Members** lists everyone with their fingerprints; the admin removes people there (**Remove…**). The group moves to a new queue; they read nothing from then on.
- **Leave:** **Leave**. The others are told; the chat stays with you, read-only.

A new member does not see what was said before they joined. Documents go to the group as in a chat (up to 1 MiB, saved only by hand).

## Invites with a QR code

**Invite** shows your one-time invite as a QR code for the Android app to scan, with **Copy invite** for the text. Under "Other invites" are **Public invite**, **Incognito invite** and **Revoke unused invites**. See [Contacts](contacts.md).

## Settings

**Settings** under the contact list:

- **Appearance:** the theme (the same as in the terminal and on Android), the system sound for new messages, and **Lock after** 1, 5, 15 or 30 minutes.
- **Contacts:** **One connection for all contacts** and **Connect the open chat at once** (see [Tor and bridges](tor.md)).
- **Devices:** **Devices…** lists your devices; **Link to my main device** on a new one. See [Devices](devices.md).
- **Account and security:** **Turn Tor on/off**, **Back up your identity**, **Change password**, **Set a duress password**, **Tor bridges**, **What's new in this version**.
- **Danger zone:** **Wipe everything**. See [Backups, locking, wiping](safety.md).

## What a window shows

A window shows more to the rest of the computer than a terminal does. Screenshots and screen recording can see it; there is no protection like Android's. The text passes through the window system and the graphics driver. If that matters for a conversation, use the terminal client, and lock the app (**Lock**) when you step away.
