---
title: "Small private groups that nobody can find: termchat.app 0.10"
description: "termchat.app 0.10 adds groups of up to 32 people, by the admin's invitation only. Megolm from vodozemac, the group ratchet Matrix uses, with keys shared over Olm. What the relay learns, and what it cannot (ADR-027)."
date: 2026-11-18
lang: en
---

Until now termchat.app had one-to-one chats only. But people talk in small circles too: a family, a team, a few friends who share a relay. Version 0.10 adds groups for them, and only for them.

## What a group is here

- **Up to 32 people**, the one who made it included.
- **The one who makes a group is its admin.** Only the admin adds and removes members, and only from their own contacts.
- **No links that anyone can use, no search, no public groups.** Nobody can find a group. You get into one only when the admin invites you, and nothing happens until you accept.
- **The same chat as one to one:** answers, edits, deletes, reactions (they show how many chose each one) and a disappearing timer that the admin sets for everyone.

In the terminal: Ctrl+P, then g for a new group; inside a group, Ctrl+R shows the members. In the desktop app: **+ New** above the groups. On Android: **New group** in the menu.

Most group chats are built to grow. These are built to stay small.

## How it is encrypted

With Megolm from vodozemac, the group ratchet Matrix uses, next to the Olm we already use for chats. Nothing is combined in a new way.

- Each member writes with a Megolm session of their own and hands its key to every other member **over Olm, one to one**.
- **A new member reads nothing said before they joined.** They get only the current keys. History for newcomers is off by design.
- **A removed member reads nothing after.** Everyone starts a new session, and the group moves to a new queue, so the removed member does not even know where it now lives.
- Sessions also renew after 100 messages or 7 days.
- Every message names its sender inside the encryption, and keys are filed under the member who brought them, so one member cannot write as another.

## Members who are not each other's contacts

In a group of eight, you may know only the admin. How do you know "Bob" is Bob? The admin signs the member list with their identity key, and your app shows only people on the latest list it has verified. Your trust in them is the admin's word, and the app says so: "The admin added them. Compare safety numbers in person to be sure."

## What the relay sees, honestly

A group has one queue on the admin's relay, with a separate reader for each member. The relay keeps every message until each reader has taken it, or 14 days.

- **It sees the size of the group.** It knows how many readers a queue has. That is the main new leak, and we accepted it for groups this small.
- It sees when a message is written and when each reader takes it, as with any queue.
- Key shares come in bursts when someone joins or leaves, so it can notice that membership changed.
- It does **not** see the group's name, the member list, who the admin is, or a single message.
- With Tor, it cannot tell which of its other queues belong to a group's readers, except by timing. Without Tor, a member's IP links their connections.

## Choices we did not make

- **One copy per member** (pairwise fan-out): up to 32·31 queues, and each message written 31 times at once, a burst the relay can see.
- **MLS or Signal's Sender Keys:** not in vodozemac, and we do not build our own cryptography.
- **Any member can invite:** a group then grows past what its admin agreed to. We will revisit it if people ask.

## Limits for now

Groups carry text only, no documents yet. If the admin loses their device, the group stays readable but cannot change; members can make a new one. Everyone in a group needs 0.10: the admin can invite only contacts whose app understands groups, and the app says why when it cannot.

The full design is in ADR-027.
