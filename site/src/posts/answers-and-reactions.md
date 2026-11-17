---
title: "Answers, edits and reactions without telling the server: termchat.app 0.9"
description: "termchat.app 0.9 lets you answer a particular message, edit or delete your own for both sides, and react with one of seven pixel sprites. All of it travels inside the end-to-end encryption; the relay sees an ordinary message (ADR-025)."
date: 2026-11-17
lang: en
---

Answering a particular message, fixing a typo, taking something back, a quick 👍 instead of a whole reply: people expect these from any messenger, and termchat.app had none of them. Version 0.9 adds all four, without giving the relay anything new to look at.

## What you can do now

- **Answer** a message. The answer points at it and shows its first line above your text.
- **Edit** your own message. The other side sees the new text and an "edited" mark.
- **Delete** your own message for both of you. A line "message deleted" stays, so nobody wonders why the conversation jumps.
- **React** with one of seven signs: like, love, laugh, wow, sad, thanks and dislike.

In the terminal, Shift+Up or Ctrl+N picks a message: Enter answers, e edits, d deletes, 1 to 7 react. In the desktop app, right-click a message. On Android, long press it.

## What the relay sees

Nothing new. An answer, an edit, a delete or a reaction is one more message inside the Olm encryption, padded to the same sizes as every other. A reaction looks to the relay exactly like a short "ok". It still sees that something was sent, and when, as with any message; the threat model says so.

## Decisions worth explaining

- **An answer is a pointer, not a quote.** It carries a random id of the message, never its text. Your app shows the quote from its own copy, so a disappearing message still disappears on time; when it is gone, the answer says "message no longer here".
- **Edits keep the timer and keep no history.** A disappearing message does not live longer because it was fixed, and the old text is wiped, not kept somewhere for later.
- **A delete is a request.** The other app removes the message and wipes the text. A modified app, a screenshot or a backup keeps what it kept. The app says "deleted for both", never "deleted everywhere".
- **Only your own messages.** A contact cannot edit or delete what you wrote, even by naming its id: each side names its own messages, and changes reach only the sender's.
- **Seven fixed reactions, drawn by us.** Any emoji or free text as a reaction would be a second text channel past every limit, and emoji break column widths in a terminal. So there are seven, by number, drawn by termchat.app itself as 1980s arcade sprites in your theme's colours: the same picture on every phone and computer. In the terminal they are one-column signs: ▲ ♥ ☺ ◎ ☹ ✧ ▼.

## Updating, honestly

Contacts still on 0.8 cannot read the new kinds of message. Your app first tells each contact what it reads, once, and until their app says the same, it sends them plain messages and offers no answers or reactions in that chat. The price: a contact on 0.8 sees one "could not be decrypted" line and one "1 message lost" when that first note arrives. Nothing is lost in fact. From 0.9 on, a newer kind of message is skipped quietly, so the next new feature will not cost even that.

## For scripts

`termchat recv --json` now prints the id of each message, what it answers, and edits, deletes and reactions as events of their own. A server can see that you put a 👍 on "backup failed".

The full design is in ADR-025.
