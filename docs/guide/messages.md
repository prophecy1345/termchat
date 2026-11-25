---
title: "Messages in termchat.app: answers, edits, reactions, files, disappearing"
description: "Sending messages and documents in termchat.app, answering, editing, deleting and reacting, disappearing messages, chats kept in memory only, search, and what the warnings in a chat mean."
---

# Messages

Русская версия: [messages.ru.md](messages.ru.md).

## Notes to yourself

**Notes** is a chat with yourself: links to read later, addresses, drafts. It has its own row above your contacts, labelled "only on your devices". Write, edit, delete and forward notes; its menu holds only what fits a chat with yourself: **Search**, **Export**, the disappearing timer and **Clear history** (on Android also on a long press on its row). Notes cannot be answered or reacted to, and Notes itself is not renamed, muted, pinned or archived.

- **On one device nothing leaves it.** Notes stay in the encrypted state file and never go on the wire.
- **With linked devices** each note, edit and delete reaches your other devices, end-to-end encrypted, directly between them. A contact can never write into your Notes.
- A newly linked device sees notes written from then on, not older ones; a device offline for more than 14 days misses what was written meanwhile.
- **Keep it in order:** write `#tags` in a note (`#work`, `#read-later`). Over Notes a row of filters shows **All**, **Favorites**, **Links** and, once a note has a tag, **By Tags**: a list of your tags with their counts, with a cross next to it to go back to All. Tap a filter again to go back to All. Star a note to keep it in Favorites: **Star** under it on the desktop, the long-press menu on Android, **Shift+↑** then **s** in the terminal (**Ctrl+P**, **f** steps through the filters there). Tags and stars reach your other devices too. Links are never opened or previewed.
- Documents in Notes are not supported yet.

## Sending

Type and send: **Enter** in the terminal and the desktop app (**Shift+Enter** there starts a new line), **Send** on Android. A message holds up to 16 000 bytes: about 16 000 Latin or 8 000 Cyrillic characters. Pasted text is only text; it never acts as key presses.

Next to your messages: `…` (terminal) or "sending" while it goes out, one check (`✓`) once the relay has it, `✗` or "failed" with the reason. In a chat, two checks (`✓✓`) say it is on a device of theirs, and two in the theme's accent colour that they read it. Their app sends these a few seconds to half a minute later, or with its next message; a contact on 0.25 or older sends none, and groups have none. Both can be turned off in [Settings](settings.md).

If your contact is offline, the relay keeps the encrypted message for up to 14 days and hands it over when they come back.

A message you started and did not send stays in its chat as a draft: when you open another chat, lock the app or quit, and on the next start. Drafts live in the encrypted state file, never in a chat with **No history**. Editing one of your sent messages takes the place of the draft.

## Answers, edits, deletes, reactions

Since version 0.9, between apps that both run 0.9 or newer:

- **Answer** a particular message. The answer shows the first line of what it answers; it carries a pointer, not the text, so a disappearing message still disappears on time.
- **Edit** your own message. The other side sees the new text and an "edited" mark. No history is kept, and the timer of a disappearing message does not move.
- **Delete** your own message for both of you. A line "message deleted" stays. A delete is a request: a modified app, a screenshot or a backup on the other side keeps what it kept.
- **React** with one of seven signs: like, love, laugh, wow, sad, thanks, dislike. The same one again takes it back. One reaction each per message.

How:

- **Terminal:** **Shift+↑** or **Ctrl+N** picks the newest message; **↑/↓** move. Then **Enter** answers, **e** edits, **d** deletes (after **y**), **1** to **7** react, **Esc** goes back to typing. While you answer or edit, the top of the field says so; **Esc** cancels.
- **Desktop:** right-click a message: the seven reactions on top, then **Answer**, and **Edit** and **Delete for both** on your own. The line over the field says what SEND will do; a click on it or **Esc** cancels.
- **Android:** long press a message: the reactions, **Answer** and **Copy**, and **Edit** and **Delete for both** on your own. **×** over the field cancels.

A contact on 0.8 or older cannot read these. Until their app says it can, yours sends them plain messages and offers no answers or reactions in that chat ("their app is older"). When you first update, contacts still on 0.8 see one "could not be decrypted" line and one "1 message lost": nothing is lost in fact.

## Forward

Send a message or a received document on to another chat or group. It goes as a new message of yours, with no mark of where it came from, and follows the timer of the chat it lands in. A document can be forwarded until you save or discard it; after that it is a file on your disk, and you send it from there.

- **Terminal:** pick the message (**Shift+↑**), press **f**, choose the chat and press **Enter**.
- **Desktop:** right-click the message → **Forward**, then click the chat.
- **Android:** long press the message → **Forward**, then tap the chat.

## Documents

Files of up to 25 MiB, end-to-end encrypted like messages. Up to 1 MiB a file goes at once: 200 KB takes about 10 seconds, 1 MiB about 3 minutes. A larger one is first offered, and its bytes go only when the contact downloads it: 25 MiB takes about 2 minutes (about 10 with a relay older than 0.30.0), and nobody gets a large file they did not ask for. Both of you need 0.29.0 or later. Files work only between accepted contacts, and in groups (there up to 1 MiB).

- **Send:** **Ctrl+L** in the terminal (type the path); in the desktop app **File**, then **Choose** for the system dialog, or drop a file on the window; the paperclip on Android. In chats and in groups alike.
- **Offered:** a file over 1 MiB first shows as "offered". **Ctrl+G** in the terminal asks for it (type `DISCARD` to decline instead); the desktop app and Android have **Download** and **Decline** under it. An offer nobody answers is dropped after 7 days.
- **Receive:** the file waits in the chat as "unsaved". **Ctrl+G** in the terminal saves the newest one into a folder you type (`~/Downloads` by default; the folder must exist), or discards it if you type `DISCARD`. The desktop app has **Save** under it (**Choose** picks the folder), Android **Save** and **Discard**.

**View an image** (desktop and Android): a received PNG, JPEG, GIF or WebP gets **View** under it while it waits unsaved. Only that click decodes it, inside termchat.app, with limits; zoom with the wheel or a pinch, Esc or back closes. The first frame of a GIF only; nothing is cached or written to disk. Once you save or discard the file, View goes with it.

Apart from that, termchat.app never opens, previews or renders a file, and never overwrites one when saving. Files from contacts can be harmful: check before you open them. A saved file lives outside termchat.app, and wiping does not erase it.

## Disappearing messages and no history

- **Disappearing messages** (**Ctrl+T** in the terminal, the timer button next to the field in the apps): off → 5 minutes → 1 hour → 1 day → 1 week. It applies to what you send from then on, and the message disappears on both sides when its time is up. The chat title shows `⏱`.
- **No history** (**Ctrl+E** in the terminal, ⋮ → **No history** in the apps): this chat is never written to disk. It is gone when you lock or quit.

## Pin and archive chats

Archived chats sit above the others, under their own heading; pin a chat to keep it at the top of your contacts. In the desktop app and on Android the Archived, Contacts and Groups headings fold: click or tap one to hide or show its chats. An archived chat still receives messages and counts them as unread; mute it as well to keep it quiet. In the terminal: **Ctrl+P**, then **i** pin or **z** archive, or right-click the contact; in the desktop app and on Android, the chat menu → **Pin** or **Archive**. Groups keep their own place below the chats.

## Mute a chat or group

**Ctrl+V** in the terminal, ⋮ → **Mute** in a chat or **Mute** in a group in the desktop app, ⋮ → **Mute** on Android: off → 1 hour → 1 day → always; each tap takes the next step, and the menu stays open until you pick. In the list a muted chat or group has a struck bell beside its name, in the terminal a dim **~** before its count. A muted chat makes no sound and no notification; its unread count still grows, and a new contact request always alerts. On Android, a wake-up that arrives while the app is locked cannot tell which chat it is for, so it still says "New message".

## Export a chat

Save a chat or group as a plain text file: each message with its time in UTC and who wrote it, documents as their names. **The file is not encrypted.** Anyone who can read it reads the chat, disappearing messages included, so keep it somewhere safe and delete it when you are done.

- **Terminal:** **Ctrl+P**, then **e**, and the folder (`~/Downloads` by default).
- **Desktop:** ⋮ → **Export** in a chat, or **Export** in a group, then the folder.
- **Android:** ⋮ → **Export**, then where to save it.

An existing file is never overwritten.

## Clear a chat's history

Delete every message of a chat or group on this device: **Ctrl+P**, then **w** in the terminal; ⋮ → **Clear history** in a chat or **Clear history** in a group in the desktop app; ⋮ → **Clear history** on Android. The others, and your other devices, keep theirs; what is still on its way (a message not sent yet, a document still arriving) stays. The cleared messages are wiped from the state file. This cannot be undone.

## Search

**Ctrl+S** in the terminal, ⋮ → **Search** on Android, and **Ctrl+F** (**Cmd+F** on a Mac), ⋮ → **Search** or **Search** in a group in the desktop app. Results show as you type. The desktop app also finds documents by name; **Esc** or **×** closes the search. Search runs on your device only.

## Warnings in a chat

- **"⚠ N message(s) missing so far"**: something sent to you did not arrive, lost or dropped by the relay. It goes away if the message turns up later.
- **"⚠ they restarted the conversation"**: your contact restored a backup or reinstalled. Messages sent around then may be lost; nothing else to do.
