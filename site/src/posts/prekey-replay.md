---
title: "Replaying a message nobody can read: the fallback-key gap in our Olm integration"
description: "How a relay could deliver an end-to-end encrypted message again and again without reading it, why vodozemac's fallback key allowed it, and how termchat.app now refuses to build a session twice (ADR-019)."
date: 2026-11-14
lang: en
---

End-to-end encryption keeps the relay from reading your messages. It does not, by itself, keep the relay from delivering one of them twice. termchat.app had such a gap in how it used Olm: a relay could replay the first messages of a conversation as often as it liked. It could not read them, but the recipient saw each copy as a new message. It was found by a source review, fixed in 0.5.3, and tightened again in 0.6.2. This is what went wrong and what the fix does.

## How a conversation starts in Olm

termchat.app uses Olm from [vodozemac](https://github.com/matrix-org/vodozemac), the Rust library behind Matrix. Starting a conversation works roughly like Signal's X3DH:

- Each identity publishes **one-time keys** and a signed **fallback key** on the relay.
- To write to someone, the sender claims one of their keys from the relay and builds an outbound session on it.
- Until the recipient replies, every message the sender writes is a **pre-key message**: it carries what the recipient needs to build the matching inbound session.
- A one-time key is deleted when it is used. The fallback key is not: it is there for when the one-time keys run out, and it stays until it is replaced.

That last point is by design. Without a fallback key, a recipient whose one-time keys are used up could not be reached at all.

## The gap

When a message arrives, the client tries the sessions it already has. If none decrypts it and it is a pre-key message, it builds a new inbound session from it.

- On a **one-time key** that fails the second time: the key is gone.
- On the **fallback key** it succeeds again. The same pre-key message builds the same session again, the same plaintext comes out, and the new session's fresh sequence counter accepts it.

So a relay could take any message written before the recipient's first reply and deliver it again, any number of times, while the recipient still held that fallback key. The app showed each copy as a new message. "Pay 100" arrives three times; a contact request from someone you deleted comes back.

The relay also decided how long that window stayed open. The client replaced its fallback key when the relay reported that it had been used. A relay that hands out the fallback key every time, and never says so, keeps one key alive for good. The sender cannot tell: the key carries the recipient's valid signature either way.

The review confirmed it with a short test: decrypt, then decrypt the same envelope again. Both returned the message.

## The fix (ADR-019)

The fix adds no cryptography. The client stops calling one vodozemac function twice with the same input, and replaces a key on its own clock.

1. **A known session is never built twice.** A pre-key message whose session id matches a session the contact already has, and which that session cannot decrypt, is refused. An honest new session from the same peer has a new base key, so a new id.
2. **Remember the ids of inbound sessions.** Rule 1 does not help once the session is gone: pushed out by the limit of five per contact, or deleted with the contact. So each identity keeps the ids of the inbound sessions it built, saved with its state and carried in its backup. Before building a session, the id is looked up; a known id is refused. An entry goes when the fallback key it could have been built on is forgotten, since nothing can build that session any more.
3. **The fallback key also rotates by age.** The client replaces it when the relay says it was used, as before, or when it is older than seven days, on the client's own clock.

The list is capped: 200 entries per sender and 10,000 in all, so a busy sender cannot grow the state file without bound. About 0.5 MB at most; a normal account holds a few dozen.

## The second look

A second source review found a way around the cap. The client filed each entry under the sender the relay named, and did so before it checked whether that name was true. A relay could start 200 sessions of its own, label each one as coming from a contact you had deleted, and push that contact's real entry out of the list. Each forged message was refused, but it was remembered first. Then the old contact request replayed again.

The fix in 0.6.2 is one line: entries are counted under the **Curve25519 key inside the pre-key message**, which Olm vouches for, not under the name the relay gives. The relay's 200 sessions now count against its own key, and the contact's entry stays. A test reproduces the attack with 201 sessions; it failed before the change and passes after it.

## What is left

- **The fallback key still lives about 30 days.** About 15 days current and 15 more accepted, whatever the relay reports. A relay can still hand out the fallback key every time, and the first messages of a new conversation then depend on that key instead of a one-time key. Signal's signed pre-key has the same property.
- **The overall cap of 10,000 entries can still be filled** by someone who reaches the fallback key, with sessions of their own.
- **A relay can still drop messages.** That cannot be prevented; a gap in the sequence numbers shows it.

## What we took from it

A library's guarantee holds only on the path you actually take. vodozemac does what it promises: one-time keys are used once, fallback keys stay until replaced. The gap was in our code. When a decrypt fell through, nothing asked whether this exact session had been built before.

The decision, with the rejected alternatives, is [ADR-019](https://github.com/prophecy1345/termchat/blob/main/docs/adr/019-prekey-replay.md). Both reviews were made with Anthropic's Claude Fable 5.2 model and are published with their findings: [first](https://github.com/prophecy1345/termchat/blob/main/docs/security/review-2026-11.md) and [second](https://github.com/prophecy1345/termchat/blob/main/docs/security/review-2026-11b.md). Neither is an independent audit; termchat.app has not had one yet, so treat it as experimental.
