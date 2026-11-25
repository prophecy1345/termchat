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
| `#` | a group; the number on the right counts its members |
| `alice·36335` | name plus the start of the fingerprint: for unverified contacts, and whenever two names look the same |
| `…` `✓` `✗` | your message: sending, on the relay, failed |
| `▲ ♥ ☺ ◎ ☹ ✧ ▼` | reactions: like, love, laugh, wow, sad, thanks, dislike |

## Keys

| Key | Action |
|---|---|
| **Enter** | send |
| **↑ / ↓** | choose contact |
| **PgUp / PgDn** | scroll the chat |
| **Shift+↑** or **Ctrl+N** | pick a message: **Enter** answer, **e** edit, **d** delete for both, **1**–**7** react (again to take back), **Space** mark, **x** delete the marked on this device, **Esc** back |
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
| **Ctrl+P** | menu: **s** settings, **k** keys, **m** manual, **n** what's new, **o** invite, **a** add, **b** backup, **g** new group, **v** devices, **l** link to my main device (a new device) |
| **Ctrl+K** | lock now |
| **Ctrl+Q** or **Ctrl+C** | quit |
| **?** or **F1** | the list of keys |
| **Esc** | close a window, cancel an answer or edit |

Editing what you type: **←/→**, **Home/End**, **Alt+←/→** (by word), **Ctrl+W** (delete a word), **Ctrl+U** (delete to the start).

## Groups

A small private group of up to 32 devices (32 people with one each; fewer when members use several), by invitation only ([ADR-027](https://github.com/prophecy1345/termchat/blob/main/docs/adr/027-small-private-groups.md)).

- **New group:** **Ctrl+P**, then **g**, and a name. You are its main admin.
- **Invite:** in the group, **Ctrl+R** opens the members, **a** lists contacts you can invite. Any admin invites, only their own contacts, on a version that knows groups.
- **Join:** the invite comes in your chat with the admin who invited you; **Ctrl+Y** there joins.
- **In the group:** answers, edits, deletes and reactions work as in a chat. Each member's name has a colour of its own, the same on every device. Only the main admin sets the timer (**Ctrl+T**).
- **Documents:** **Ctrl+L** sends one (up to 1 MiB, one at a time), **Ctrl+G** saves the newest one received, as in a chat.
- **Remove:** **Ctrl+R**, pick the member, **x** (main admin only). The group moves to a new queue; they read nothing from then on.
- **Admins:** up to 4, marked "main admin" and "admin" in **Ctrl+R**. There the main admin picks a member and presses **m** to make or remove an admin, or **h** to hand the group over (then **y** to stay, **l** to leave). Another admin can only invite their own contacts, and steps down with **d**. Only the main admin removes people, sets the timer and makes admins. If the main admin leaves, the admin made first takes over. Every member's app must be updated first. Make admins only of people you trust: an admin can cut a member off at the relay ([ADR-042](https://github.com/prophecy1345/termchat/blob/main/docs/adr/042-group-admins.md)).
- **Leave:** **Ctrl+D**. The others are told; the chat stays with you, read-only.

A new member does not see what was said before they joined.

## Mouse

Click a contact to open it; right-click it for everything you can do with it. Drag the edge of the contact list to resize it. While the mouse is on, the terminal's own text selection needs **Shift** held while dragging (Option in some macOS terminals); or turn the mouse off in settings.
