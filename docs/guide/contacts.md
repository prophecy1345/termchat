---
title: "Contacts in termchat.app: invites, requests, safety numbers"
description: "How to add contacts in termchat.app with one-time, public and incognito invites, accept or reject requests, compare safety numbers, and rename, block, report or delete a contact."
---

# Contacts

Русская версия: [contacts.ru.md](contacts.ru.md).

There is no search and no directory: nobody can find you. People are added by exchanging **invites**.

## Invites

An invite is a long line starting with `termchat:`. It holds your public keys, where to reach you, and a code. It is safe to send by email, another messenger, or as a QR code.

- **One-time** (the usual kind): works once. Every time you open your invite you get a new one, so send each to one person.
- **Public**: works for anyone until you renew it, which stops the old one. For a profile or a post.
- **Incognito**: a new identity of yours, with its own keys and a name you choose, used only with whoever takes this invite. Contacts of different identities cannot link you to each other, and over Tor the relay cannot either. Deleting an incognito contact deletes that identity.
- **Revoke unused** stops every one-time invite you have handed out and nobody has taken yet.

Invites from before version 0.7.0 no longer work: ask for a new one.

Where to find it:

- **Terminal:** **Ctrl+O** shows your invite. **c** copies it (in terminals with OSC 52: iTerm2, kitty, WezTerm, Windows Terminal and others, not macOS Terminal), **p** switches to the public invite and **n** renews it, **i** makes an incognito invite, **r** revokes unused ones.
- **Desktop:** **Invite** under the contact list shows a QR code and **Copy invite**; under "Other invites" are **Public invite** (with **Renew**), **Incognito invite** and **Revoke unused invites**.
- **Android:** ⋮ → **Your invite**, with a QR code, **Copy**, **Share**, **Public** and **Revoke unused**; **Incognito invite** is on the same screen.

## Adding someone

Take their invite and:

- **Terminal:** **Ctrl+A**, paste, **Enter**. **Ctrl+N** instead of Enter adds them from a new incognito identity.
- **Desktop:** **Add**, paste, **Add**; tick "Answer from a new incognito identity" for incognito.
- **Android:** **+** or ⋮ → **Add contact**, then **Scan QR code** or paste; **Add** or **Add incognito**.

## Requests

When someone adds your invite, they arrive as a **request** (a `?` in the terminal list). Until you accept, what they send stays hidden: the chat says how many messages are waiting, and up to five are kept and shown when you accept. This keeps strangers off your screen.

- Accept: **Ctrl+Y** in the terminal, **Accept** in the desktop app and on Android.
- Reject: delete the request (**Ctrl+D**, then **y**). They cannot simply ask again; if you later add their invite yourself, the rejection is taken back.

If both of you add each other's invites, neither is a request.

## Introducing two contacts

When two of your contacts should meet, introduce them instead of copying an invite around.

- **Start:** in the chat with Bob, **Introduce** (desktop and Android: the chat menu; terminal: **Ctrl+P**, then **o**) and pick Carol.
- **Carol is asked first.** Her chat with you shows "wants to introduce you to Bob·12345": **Allow**, **Allow incognito** (Bob meets a new identity of hers, not her main one) or **Decline**. She sees Bob's nickname and fingerprint; Bob hears nothing unless she allows it.
- **Then Bob decides.** His chat with you shows "introduces Carol·67890": **Add** or **Ignore**. Add opens the new chat; Carol's app accepts him by itself, since she already said yes.
- In the terminal the answers are in the chat menu too: **Ctrl+P**, then **a** (allow or add), **c** (allow incognito), **n** (decline or ignore).

The new contact starts **not verified**: you passed the invite on, and the two of them should compare their safety number before trusting it. An introduction nobody answers is dropped after 7 days. Everyone needs version 0.32.0 or later; groups and chains (Bob passing Carol on) are not possible.

## Safety number, once per contact

The relay cannot read messages, but someone who controls the network could try to give you their keys instead of your friend's when you first connect. Rule it out once:

1. Open the safety number: **Ctrl+F** in the terminal, ⋮ → **Safety number** in the desktop app and on Android. It is 60 digits. Your friend opens the same screen.
2. Compare the numbers in person, on a video call or by phone, not over termchat.app.
3. If they match, mark the contact verified: **v** in the terminal, the button on that screen in the apps.

**Or with a QR code**, when one of you has the Android app (ADR-037). The safety-number screen shows the number as a QR code too: in the terminal press **q** there, in the desktop app and on Android it is under the digits. On Android, tap **Scan their code** and point the camera at the other screen. If the codes match, the contact is verified on that phone; the other side verifies on its own, by scanning back or with **v** after comparing. A code that does not match changes nothing and says so. Scan only a code shown on their screen in front of you: one sent over a chat, a screenshot or a photo proves nothing.

If the numbers differ, do not trust the contact. Delete it and exchange invites again some other way.

Until a contact is verified, its name shows the start of its fingerprint, like `alice·36335`. Names are chosen by people themselves; the digits are what tells two "alice"s apart. They also show whenever two names look the same.

## Icons

Each contact, group and Notes shows its first letter. **Icon** in its menu (⋮ of the chat or the group header, a long press in the Android list; **Ctrl+P**, **c** in the terminal, which shows two characters such as `<>`) puts one of 15 drawings in its place: a person, code, a branch, a server, a heart, a key and others. **Letter** brings the letter back. The icon stays on this device: the contact never learns it, and your other devices keep their own.

## Rename, block, report, delete

These act on one contact. In the terminal use the keys below or right-click the contact; in the desktop app and on Android, the ⋮ menu of the chat.

| Terminal | What it does |
|---|---|
| **Ctrl+R** | Rename. Only on your device; an empty name brings back theirs. |
| **Ctrl+B** | Block or unblock. Their queue on the relay is deleted, so nothing of theirs arrives, and you cannot write to them. |
| **Ctrl+X** | Report to the relay operator. You see exactly the text the operator will read (their last five messages); confirming sends it and blocks the contact. The relay does not know who wrote, so it bans nobody. |
| **Ctrl+D** | Delete the contact and the chat, after **y**. |
