---
title: "termchat.app settings: themes, notifications, mouse, where data lives"
description: "termchat.app settings in the terminal, desktop and Android apps: colour themes, notifications that never show names, the mouse, and the files on your disk."
---

# Settings and data

Русская версия: [settings.ru.md](settings.ru.md).

## Opening settings

- **Terminal:** **Ctrl+P**, then **s**. **Tab** or a click switches tabs (theme, notifications, interface, security), **↑/↓** move, **Enter** or a click changes the value.
- **Desktop:** **Settings** under the contact list.
- **Android:** ⋮ → **Settings**.

The security settings (backup, passwords, Tor, bridges, connections, lock time, wipe) are described in [Backups, locking, wiping](safety.md) and [Tor and bridges](tor.md).

## Themes

19 colour themes, light and dark, the same in all three apps; the terminal adds `terminal`, its own 16 colours, for terminals without true colour. In the terminal, moving through the list shows each theme at once; the chat keeps your terminal's background, and the theme colours the bars and windows.

## Formatting

**Show formatting** is on by default. Turn it off to show the Markdown source in messages and reply previews. The composer still lets you insert formatting for recipients. Find the switch in the terminal's **interface** tab or under **Appearance** in Settings on desktop and Android. [Supported Markdown](messages.md#markdown-formatting)

## Notifications

termchat.app never puts a name or a text where the operating system keeps them.

- **Terminal:** a card with the sender's name (never the text) shows in the corner for 5 seconds when a message arrives in a chat you are not looking at; click it to open the chat. **sound for new messages** plays the system sound (macOS: Glass; Linux: `canberra-gtk-play` or `paplay`; otherwise the terminal bell). **system notification** (off by default) asks the terminal for a desktop notification that always says just "termchat: new message". The window title shows only the unread count: `termchat (3)`.
- **Desktop:** the system sound for new messages and, on macOS and Linux, a system notification that only says "New message" while the window is not in front, both in **Appearance**. Linux needs `notify-send` (package `libnotify-bin` or `libnotify`). Windows has no notification yet.
- **Android:** see [Android](android.md): "termchat.app: New message", with no name and no text.

## Receipts

- **Delivery receipts** (on by default) tell a contact that their message is on your device: they see two checks. Off: you send none, and still see theirs.
- **Read receipts** (off by default) tell them you read it while the chat was open, and show you when they read yours. It is mutual: off, you neither send nor see them.
- **Terminal:** the notifications tab (**Ctrl+P**, **s**). **Desktop:** **Receipts** in Settings. **Android:** Settings → Notifications.

A receipt is encrypted like a message, but its timing is visible: a small message goes back soon after one arrives, so over time the relay can tell which two queues talk and when the device is online ([threat model](../threat-model.md)). Turn delivery receipts off if that matters more than the checks.

## Mouse (terminal)

The **interface** tab turns the mouse on or off. While it is on, the terminal's own text selection needs **Shift** held while dragging.

## Where data lives

| | |
|---|---|
| `~/.termchat/state` | Everything about your identity: keys, contacts, history. One file, encrypted with your password. Windows: `C:\Users\<you>\.termchat\state`. |
| `~/.termchat/arti` | Tor's directory cache, when Tor is on. |
| `~/.termchat/settings.json` | Theme, sound, mouse, message formatting, lock time. This file is plain text so the lock screen can use your theme. It holds nothing about your contacts. A wipe removes it. |
| `~/.termchat/lock` | Keeps a second program from opening the same account. |

The terminal client and the desktop app share this folder. Set `TERMCHAT_HOME` to keep a separate identity elsewhere, for example to try termchat.app with two identities on one computer:

```sh
TERMCHAT_HOME=~/termchat-test termchat
```

On Android everything lives in the app's private storage.
