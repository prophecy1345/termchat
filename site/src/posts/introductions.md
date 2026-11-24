---
title: "Share a contact without giving anyone away: introductions in termchat.app 0.32"
description: "termchat.app 0.32 lets you introduce two of your contacts in an end-to-end encrypted messenger with no phone numbers and no user search. The person introduced is asked first, nobody can be reached without saying yes, and the relay cannot tell an introduction from any other invite."
date: 2026-11-24
lang: en
---

Most messengers let you share a contact in one tap: a phone number or a username goes to someone else, and from then on they can write to that person whether that person wants it or not. termchat.app has no phone numbers, no usernames and no search, on purpose: nobody can reach you unless you let them in. Until now that meant two of your friends who should meet had to swap an invite some other way, often over a less private channel.

Version 0.32 adds introductions, built so that the old promise still holds.

## How an introduction works

Alice knows Bob and Carol. In her chat with Bob she picks **Introduce**, then Carol.

1. **Carol is asked first.** Her chat with Alice shows "wants to introduce you to Bob", with Bob's nickname and fingerprint. She chooses **Allow**, **Allow incognito** or **Decline**.
2. **If she declines, Bob never hears of it.** Only Alice sees the answer.
3. **If she allows**, her app makes a one-time invite for this introduction alone, and Alice's app passes it to Bob.
4. **Bob decides.** His chat with Alice shows "introduces Carol": **Add** or **Ignore**.
5. **Carol's app accepts Bob by itself**, because she already said yes, but only for the code made for this introduction and only from the key Alice named. If the invite leaked to someone else, that someone is an ordinary pending request.

The new chat has its own encrypted sessions and its own queues on the relay, like any other. Alice sees nothing of it.

## What makes it private

- **Consent first.** What gets shared is Carol's address, so Carol decides before anything leaves her hands. Bob, in turn, chooses whether to add her.
- **Nothing of Alice's is passed on.** Alice never hands over her own queue to Carol or her own keys; the invite is Carol's, made for Bob. The relay cannot link Alice and Bob through it.
- **Incognito if you want.** "Allow incognito" makes a new identity just for Bob, with its own keys and name, so Bob cannot connect Carol to her other contacts.
- **The relay learns nothing new.** It sees an ordinary contact request to an invite queue and cannot tell an introduction from any other invite.
- **No chains.** Bob cannot pass Carol on to someone else: every introduction asks the person introduced.

## What it does not promise

Alice is trusted the way an introducer is in real life. She passes the invite on, so she could swap it for one of her own and sit in the middle. That is why a contact made through an introduction starts **not verified**: Bob and Carol should compare their safety number, in person or by QR code, before trusting it, as with any invite that came through someone else.

Carol also learns Bob's nickname and fingerprint when she is asked, before Bob has agreed to anything. That is Alice's decision, the same as mentioning Bob in a message, and the app says so when you introduce someone.

## How to use it

- **Desktop and Android:** the chat menu, **Introduce**; the answers appear as buttons in the chat with the person who introduced you.
- **Terminal:** **Ctrl+P**, then **o** to introduce; **a**, **c** or **n** to answer.

Everyone involved needs 0.32 or later. The design and its limits are in [ADR-045](https://github.com/prophecy1345/termchat/blob/main/docs/adr/045-introductions.md); the step-by-step guide is in [Contacts](/docs/contacts/).
