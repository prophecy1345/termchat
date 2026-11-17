---
title: "termchat.app in the terminal: the screen, marks and keys"
description: "The termchat.app terminal client: what is on the screen, what the marks next to contacts mean, every key, the menu and the mouse."
---

# Terminal

Русская версия: [terminal.ru.md](terminal.ru.md).

`termchat` runs in any terminal window. Use the keyboard or the mouse. **?** or **F1** lists the keys at any time; **Ctrl+P** opens the menu, which also holds a manual.

## The screen

```
 contacts                  │ alice·36335   ! not verified (Ctrl+F)
                           │
 ! alice·36335             │ alice·36335  2m
 ● bob              2      │ hi!
 ? carol·58121             │
                           │ you  now
                           │ hello ✓
                           │
                           │ › message
 + add              ≡ menu │
 ● online  you  ! no backup: Ctrl+P          ^P menu  ? keys  ^K lock  ^Q quit
```

- **Left:** contacts, the most recent chat on top. Click one, or use **↑/↓**. The number on the right counts unread messages. Drag the line between the list and the chat to resize it. In a narrow window the list hides; **↑/↓** still switch chats.
- **Right:** the chat. Messages are grouped under the sender's name and their age (`now`, `2m`, `3h`, `4d`). **PgUp/PgDn** or the mouse wheel scroll back; `↓ PgDn` shows you are not at the newest message.
- **Bottom:** where you type. Long text wraps.
- **Status bar:** `● online`, `● online via Tor` or `○ connecting…`, your nickname, and warnings. The hints on the right can be clicked.

## Marks

| Mark | Meaning |
|---|---|
| `●` | unread messages |
| `?` | a contact request you have not accepted |
| `!` | safety number not verified yet |
| `×` | blocked |
| `-` | the contact deleted their account |
| `↻` | needs a new invite (from before anonymous queues) |
| `alice·36335` | name plus the start of the fingerprint: for unverified contacts, and whenever two names look the same |
| `…` `✓` `✗` | your message: sending, on the relay, failed |
| `▲ ♥ ☺ ◎ ☹ ✧ ▼` | reactions: like, love, laugh, wow, sad, thanks, dislike |

## Keys

| Key | Action |
|---|---|
| **Enter** | send |
| **↑ / ↓** | choose contact |
| **PgUp / PgDn** | scroll the chat |
| **Shift+↑** or **Ctrl+N** | pick a message: **Enter** answer, **e** edit, **d** delete for both, **1**–**7** react (again to take back), **Esc** back |
| **Ctrl+A** | add a contact from an invite |
| **Ctrl+O** | your invite: **c** copy, **p** public, **i** incognito, **r** revoke unused |
| **Ctrl+Y** | accept a request |
| **Ctrl+F** | safety number: **v** marks verified |
| **Ctrl+S** | search this chat |
| **Ctrl+T** | disappearing messages: off, 5m, 1h, 1d, 1w |
| **Ctrl+E** | no history: this chat stays in memory only |
| **Ctrl+L** / **Ctrl+G** | send a file / save the newest received file |
| **Ctrl+R** | rename the contact |
| **Ctrl+B** | block / unblock |
| **Ctrl+D** | delete the contact, or reject a request |
| **Ctrl+X** | report to the relay operator |
| **Ctrl+P** | menu: **s** settings, **k** keys, **m** manual, **n** what's new, **o** invite, **a** add, **b** backup |
| **Ctrl+K** | lock now |
| **Ctrl+Q** or **Ctrl+C** | quit |
| **?** or **F1** | the list of keys |
| **Esc** | close a window, cancel an answer or edit |

Editing what you type: **←/→**, **Home/End**, **Alt+←/→** (by word), **Ctrl+W** (delete a word), **Ctrl+U** (delete to the start).

## Mouse

Click a contact to open it; right-click it for everything you can do with it. Drag the edge of the contact list to resize it. While the mouse is on, the terminal's own text selection needs **Shift** held while dragging (Option in some macOS terminals); or turn the mouse off in settings.
