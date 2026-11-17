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

18 colour themes, light and dark, the same in all three apps; the terminal adds `terminal`, its own 16 colours, for terminals without true colour. In the terminal, moving through the list shows each theme at once; the chat keeps your terminal's background, and the theme colours the bars and windows.

## Notifications

termchat.app never puts a name or a text where the operating system keeps them.

- **Terminal:** a card with the sender's name (never the text) shows in the corner for 5 seconds when a message arrives in a chat you are not looking at; click it to open the chat. **sound for new messages** plays the system sound (macOS: Glass; Linux: `canberra-gtk-play` or `paplay`; otherwise the terminal bell). **system notification** (off by default) asks the terminal for a desktop notification that always says just "termchat: new message". The window title shows only the unread count: `termchat (3)`.
- **Desktop:** the system sound for new messages, in **Appearance**.
- **Android:** see [Android](android.md): "termchat: New message", with no name and no text.

## Mouse (terminal)

The **interface** tab turns the mouse on or off. While it is on, the terminal's own text selection needs **Shift** held while dragging.

## Where data lives

| | |
|---|---|
| `~/.termchat/state` | Everything about your identity: keys, contacts, history. One file, encrypted with your password. Windows: `C:\Users\<you>\.termchat\state`. |
| `~/.termchat/arti` | Tor's directory cache, when Tor is on. |
| `~/.termchat/settings.json` | Theme, sound, mouse, lock time. Not encrypted, so that the lock screen can use your theme; it holds nothing about your contacts, and a wipe removes it. |
| `~/.termchat/lock` | Keeps a second program from opening the same account. |

The terminal client and the desktop app share this folder. Set `TERMCHAT_HOME` to keep a separate identity elsewhere, for example to try termchat.app with two identities on one computer:

```sh
TERMCHAT_HOME=~/termchat-test termchat
```

On Android everything lives in the app's private storage.
