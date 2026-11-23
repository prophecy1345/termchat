---
title: "Group admins, delivery receipts and a fourth review: termchat.app 0.26 to 0.28.1"
description: "termchat.app groups can now have up to four admins, messages show when they reached the other phone, and a fourth AI-assisted security review found five problems, all fixed in 0.28.1."
date: 2026-11-23
lang: en
---

Four releases went out on 23 November. Three of them add things people asked for; the last one fixes what a security review found in them.

## Up to four admins in a group (0.27)

Until now the person who made a group was its only admin. If they lost their phone, the group could no longer change. Now the main admin can make up to three more. The others can invite their own contacts and step down; only the main admin removes people, sets the disappearing timer and makes admins. Nobody can remove the main admin: they hand the group over, or leave, and then the admin they made first takes over.

Every member checks each change against the admin list in force, so an admin who goes rogue cannot simply sign their way to more power. When someone loses the role, the group moves to a new queue on the relay, so the key they held opens nothing. Members a helper invited, who are not the main admin's contacts, hear of moves through their own reader. Every member needs 0.27 or later first.

What cryptography cannot stop: any admin holds the queue's token and could cut a member off at the relay. The app tells that member, but choose admins you trust. [How it works](https://github.com/prophecy1345/termchat/blob/main/docs/adr/042-group-admins.md)

## Delivered and read (0.26)

A message now shows two checks when it is on one of the contact's devices, and two in the accent colour when they read it. Read receipts are off by default and mutual: if you turn them off, you neither send nor see them. Delivery receipts can be turned off too.

A receipt is a message the other side sends without writing anything, and its timing tells the relay a little: that a device was online, roughly when. Receipts therefore wait a random few seconds or go with the next message, never go to a contact request, and never go inside a group. [The trade-off in full](https://github.com/prophecy1345/termchat/blob/main/docs/adr/041-delivery-and-read-receipts.md)

## A fourth review, and its fixes (0.28.1)

termchat.app has had no independent audit yet. In the meantime an AI model, Claude Fable 5.2, reviews the source on request. Its fourth pass looked at everything since the third, and most of all at group admins, which had shipped the same day. It found five problems; three were confirmed with tests:

- a helper who stepped down could slip an admin a queue token of their choice;
- a helper could freeze a group by taking the last roster number, or list a member twice so the entry could never go;
- a helper could drop other members' linked devices and force the group to move;
- a member could pass on another member's session key as their own and hide that member's messages from one person;
- on Android, a share from another app could point at termchat.app's own private files.

All five are fixed in 0.28.1, and the review's proofs of concept now run as regression tests. The review, with what it read and what it did not, is published: [review 2026-11d](https://github.com/prophecy1345/termchat/blob/main/docs/security/review-2026-11d.md). An AI review is not an audit; it can miss whole classes of bugs.

## Smaller things

- A muted chat or group shows a struck bell in the list (a dim `~` in the terminal), and on Android Mute keeps its menu open so you can pick a longer span.
- Clear a chat's history on this device from its menu; the others keep theirs.
- On Android, long press a chat in the list for its menu.
- `termchat systemd-unit alice` prints a unit for `OnFailure=` that tells you when a service fails. [Scripts guide](/docs/scripts/)
