---
title: "Why should the relay know who talks to whom? termchat.app 0.7.0 moves to anonymous queues"
description: "termchat.app 0.7.0 replaces accounts on the relay with anonymous queues: no user keys, no senders, no social graph. What the relay still sees, what we gave up, and why every contact has to exchange invites once more (ADR-023)."
date: 2026-11-16
lang: en
---

termchat.app is an end-to-end encrypted messenger for the terminal, the desktop and Android. Until now, our relay could not read a single message, but it knew a lot about them: who sent each one, to whom, and when. Version 0.7.0 takes that away. This is how, what the relay still sees, and what it cost.

## What the relay knew

Up to 0.6.3, every client logged in to the relay with its identity key and a signature. Each envelope named the recipient's key. So the relay knew:

- **the sender**, from the logged-in connection;
- **the recipient**, from the envelope;
- **the time** of every message.

Put together, that is the social graph: who talks to whom, how often, at what hours. Encryption hides what you say, not who you say it to. Our threat model said so plainly, and incognito contacts (ADR-014) helped for people who turned them on. But the default was a relay that could draw your circle of contacts.

So we asked: why does the relay need to know any of this?

## The answer: it does not

A relay has to move bytes. It cannot avoid seeing when they arrive and leave. But it does not need to know whose bytes they are. The SimpleX messenger showed that a relay can work with anonymous queues only, and we took the same idea, without any new cryptography.

In 0.7.0 the relay knows no users. It knows **queues**. A queue has:

- a random `recv_id`, to read it;
- another random `send_id`, to write to it;
- a random token, the owner's secret. The relay keeps only its SHA-256.

Nobody logs in. Anyone who knows a queue's `send_id` can write to it; only the owner, with the token, can read it.

## How two people meet

1. Your invite carries the address of your **invite queue**. Strangers can write only there.
2. Someone adds your invite and writes into that queue: a contact request, and then the address of a queue they made just for you.
3. You accept. Your client makes a queue just for them, sends its address, and only then says "accepted".
4. From then on each of you writes to the queue the other made. Those addresses travel only inside the encrypted messages, so only the two of you know them.

The relay sees envelopes going into queue X and coming out of queue Y. It does not see names, keys or senders. The envelope itself names nobody: both names are inside the ciphertext, where the client checks them.

## One connection per contact

Anonymous queues mean little if one connection reads all of them: the relay would see "these queues belong to one person" and draw the graph again. So by default **each contact gets a connection of its own**, and with Tor, a circuit of its own.

That costs battery and startup time, especially on a phone with many contacts. If you prefer, settings → security → connections puts all contacts of an identity on one connection. Then the relay sees your queues together, and the app says so.

## What we gave up

- **Bans after reports.** A report still goes to the relay operator with the text you chose to disclose. But the relay cannot ban anyone: it does not know who wrote. Instead, blocking a contact **deletes their queue**, so they cannot put a single byte in it. That is stronger than before for the person blocking.
- **Your contacts, once.** Version 1 addressed people by key; version 2 addresses queues. There is no way to carry an old conversation across without a new invite. After you update, every contact shows "needs a new invite". Ask them for one, or send yours: the chat, its history and their name stay.
- **Old invites.** They named only your identity, with no queue. They no longer work.

## What the relay still sees

We will not call this "untraceable". The relay still sees:

- **timing**: when each queue is written to and when it is read. A message into one queue followed by a read from another can link them. Only a mixnet with cover traffic hides that, and we do not have one;
- **your IP address**, unless Tor is on. Without Tor, the shared IP links your connections;
- **size**, rounded up to 512, 4096 or 16384 bytes;
- roughly **how many contacts** you have, from how many connections come up when you unlock.

All of this is in the [threat model](/threat-model/), and the decision with its trade-offs is in ADR-023.

## Updating

- Update the app, then exchange invites with your contacts once.
- If you run a relay: update it. The upgrade erases the old database, which held exactly the keys and the graph we no longer want anyone to keep. `--host` is no longer needed. Clients older than 0.7.0 are told to update.
