---
title: "Your phone and your laptop, one identity: termchat.app 0.12"
description: "termchat.app 0.12 links up to four devices to one identity with a code and a fingerprint check on both screens. Each device has keys of its own; contacts write to each. What moves over, what does not, and what the relay learns (ADR-028)."
date: 2026-11-18
lang: en
---

Until now termchat.app lived on one device. Writing from the phone and from the laptop meant two identities: two fingerprints, two entries in every contact's list, two separate histories. Version 0.12 lets one identity live on up to four devices.

## Linking takes a minute

1. On the new device, create an identity (any nickname; it gets replaced) and pick **Link to my main device**. It shows a code, as text and as a QR code.
2. On your main device, open **Devices**, **Add a device**, and scan or paste the code.
3. Both screens now show one fingerprint. Go on only if it is the same on both, and confirm on both.

That is all. The new device takes your nickname and your contacts, and from then on messages from them reach both devices.

## Why the fingerprint check

The link code is the one thing that travels outside the encryption. Someone who copied it, from a screenshot or over a shoulder, could try to answer it first and pass their own device off as your main one. Then the new device would show *their* fingerprint, not yours. So nothing secret moves until a person has looked at both screens and said "the same" on each. It is the same idea as comparing safety numbers with a contact, done once, with yourself.

## Keys of its own, not a copy

The obvious shortcut, copying the whole state to the second device, does not work: the ratchets of two copies drift apart at the first message, and both break. So each device gets keys of its own, and the first one, your main device, signs a list of them with your identity key.

- **Fingerprints do not change.** The identity is still your main device's key; no contact has to verify you again.
- **Contacts check the list.** A contact accepts a device list only if your identity signed it, and talks to each device on it over a session of its own.
- **Each message goes once per device.** To each of the contact's devices, into its own queue, and as a copy to each of yours, so your laptop shows what you sent from the phone.

## What moves over, and what does not

- **Contacts move**, with your names for them and their verified marks.
- **History does not.** A new device starts empty. Copying old messages would move all of them out of one device's protection into another's, and disappearing messages would have to be tracked across both.
- **Groups stayed on the main device** in 0.12; since 0.13 they reach every device.
- **Documents** you send reach the contact's main device only for now, and your other devices get no copy of them.
- **Incognito contacts stay** where they are: each is an identity of its own.

## Losing a device

- A lost laptop or tablet: remove it from the main device. It keeps what it already had, says it was removed, and reads and sends nothing more; contacts stop writing to it.
- A lost main device: the others keep working, but cannot add or remove devices until you restore the main one from its backup.
- The duress password wipes only the device it is typed on. A signed "wipe everything" message would let a stolen main device erase your other ones.

## What others learn, honestly

- **Each device is one more way in.** A stolen unlocked laptop reads what your phone would. Lock it, and remove it when it is lost.
- **Contacts** see how many devices you have, and their keys. Labels like "phone" or "laptop" stay on your own devices.
- **The relay** sees a message to you as one write per device and can tie those queues together as one person's devices. Without Tor, IP addresses tie them too. It still reads nothing.

## One design change on the way

The first version of the design sent one write with a ciphertext for every device in it. Planning the code showed it would not fit: a long message or a document chunk, times seven devices, passes the relay's frame limit, and every device would receive every copy. Separate writes, one per device, cost less and show the relay nothing it does not already see. The design record says so in its amendment.

The full design is in ADR-028.
