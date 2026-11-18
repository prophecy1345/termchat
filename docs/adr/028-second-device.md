# ADR-028: a second device

Status: **accepted** (maintainer, 2026-11-18). Русская версия: [028-second-device.ru.md](028-second-device.ru.md).

## Context

Today one identity lives on one device. Someone who writes from a phone and from a laptop needs two identities: two fingerprints, two entries in each contact's list, two separate histories. That is the main thing that keeps a messenger from being the only one a person uses. PLAN.md, section R, item 4.

The Olm and Megolm state of a device cannot simply be copied to another one. Two copies of a ratchet go out of step on the first message, and both stop working. So each device needs keys of its own, and the identity has to tie those devices together in a way contacts can check.

What has to be decided:

1. **What an identity is** when it has several devices, and what a contact verifies.
2. **How a new device is linked** to an existing one, safely, with the tools we have.
3. **How messages travel** over anonymous queues (ADR-023) to several devices, and how a device sees what the others sent.
4. **Groups** (ADR-027) with members who have several devices.
5. **Losing and removing a device**, the duress password, and older clients.

## Decision

### 1. Identity: the first device signs the others

- The identity key stays what it is today: the Ed25519 key of the **first device** (the primary). Fingerprints and safety numbers do not change, so no contact has to verify anyone again.
- The primary signs a **device list**: a numbered list of devices, each with its own Ed25519 and Curve25519 keys. Every change makes a new list with a higher number. Contacts get only this list. A short label for each device ("phone", "laptop") goes separately, over Olm, to the person's own devices only. A device or a contact accepts only a list signed by the identity key and with a number higher than the one it has.
- **At most 4 devices**, the primary included.
- Only the primary adds and removes devices. This is the admin model of ADR-027 again, for the same reasons: one key that contacts already trust, and no new way for a stolen laptop to add devices of its own.

### 2. Linking a new device

The new device is linked by the path that already exists for contacts: an invite, a pre-key message over Olm, and a check by eye.

1. On the new device the person picks **Link to my existing termchat**. The device makes its own Olm account and a one-time **link code** `termchat-link:...`: its own `Identity`, the address of a fresh queue and 16 random bytes. It shows the code as text and as a QR code.
2. On the primary: **Settings → Devices → Add**, then paste or scan the code. The primary checks the signature in it, writes a pre-key Olm message into that queue with the 16 bytes (as an invite code, ADR-013) and a `DeviceLink` request.
3. **Both screens show the identity fingerprint** and ask the person whether it is the same on both. A person who copied the code is the only party who sees both screens; someone who intercepted the code and pushes their own `DeviceLink` shows a different fingerprint. Nothing is sent before the person confirms on both devices.
4. The primary then sends, over Olm, everything the new device needs (section 3): the new device list, contacts with their keys, nicknames and verification marks, the address of each conversation, and the settings that are not tied to a device.
5. **No history.** The new device starts empty and shows "older messages stay on your other devices". Copying history would take every old message out of one device's protection and put it into another's, and disappearing messages would have to be tracked across both. Revisit if people ask.

### 3. Messages: one write, a reader per device

ADR-027 gave a queue several readers. Contact chats use the same mechanism.

- **Every incoming queue gets a reader per own device** (`AddReader` with the queue owner's token). Each device subscribes with its own `recv_id` and token, on a connection of its own, as now (ADR-023, section 7).
- **Sending is one write of a bundle**: one Olm ciphertext for every device of the contact and for every other own device, each with the target device's key. Each reader gets the whole bundle and decrypts its own part. This is how Matrix sends to-device messages; there is no new cryptography, only several Olm messages in one envelope.
- **Your other devices learn what you sent** from the copy in the same bundle. It goes into **your own incoming queue for that contact**, not into a shared queue of your own: that queue is already linked to the contact's on the same connection, so the relay learns nothing new about who talks to whom. A shared "self" queue would link all contacts by timing.
- **Prekeys:** each reader of a contact queue publishes its own one-time keys there; `ClaimKey` returns one per reader. A contact starts an Olm session with each device it finds.
- Answers, edits, deletes, reactions (ADR-025) and documents (ADR-017) travel the same way. An edit or delete from any of your devices applies to your message on all of them.
- **Read marks** are not synced between devices in the first version.

### 4. Groups (second release)

- The first release covers one-to-one chats. Until the second, only the primary takes part in groups; the other devices show "groups are on your main device for now".
- Each device of a member is a reader of the group queue and has its own outbound Megolm session (ADR-027, section 3). Session keys go over Olm to every device of every member.
- The roster stays a list of identities. Next to each identity the admin carries the device list signed by that identity, so members who are not each other's contacts can check devices without trusting the admin for them.
- A device removed from its owner's list is treated as a removed member: everyone rotates their Megolm session.

### 5. Removing and losing devices

- **Removing a device** (on the primary: Settings → Devices → Remove): a new device list without it, `RemoveReader` on every queue, and a session rotation in groups. Contacts drop their Olm sessions with that device. The removed device keeps what it already had; it reads nothing new.
- **A secondary device is lost:** remove it from the primary as above.
- **The primary is lost:** the other devices keep working with the current list, but cannot add or remove devices. Restoring the primary from its backup file (ADR-003) brings the identity key back; sessions with contacts start anew. Handing the primary role to another device is left for later.
- **The duress password** wipes the device it is typed on and nothing else, as now. A signed "wipe everything" message to other devices is not part of this ADR: a stolen primary could then erase a person's other devices.
- **Scripts** (ADR-024) keep their own identity and are never linked as a device.

### 6. Older clients

`Understands` (ADR-025, ADR-026) goes to **level 3**: devices. A contact on level 2 or lower cannot read bundles. With such a contact only the primary talks, with plain Olm messages as now; the other devices show "this contact needs to update to see messages here". Group members below level 3 make the group fall back the same way.

## What the relay sees, honestly

- **The number of devices.** A queue with three readers belongs to someone with three devices. Bundle size, in buckets, also grows with the number of devices on both sides.
- **Device links on one connection.** Each device has its own connection per contact, so the relay sees several connections read the same queue. With Tor these are separate circuits; without Tor, IP addresses tie them to each other and to the person.
- **A device is added or removed** when `AddReader`/`RemoveReader` come for many queues at once.
- It still does not see who is in a conversation, the device list, the labels, or a single message.

For contacts: a contact sees how many devices a person has and their keys, never the labels.

## This is not a new cryptographic protocol (rule 1)

- Each device is an ordinary vodozemac Olm account; contacts talk to each device over ordinary Olm sessions.
- The device list is an Ed25519 signature by the identity key, as the roster in ADR-027.
- The link is a contact invite with a one-time code (ADR-013, ADR-023), followed by a fingerprint check by eye.
- A bundle is several Olm messages in one envelope, as Matrix's to-device messages.

## Rejected

- **Copy the whole state** to the second device: two copies of a ratchet break on the first message.
- **Every device a separate identity**, contacts add each: the problem this ADR exists to solve.
- **A new master key** for the identity (as Matrix cross-signing): every contact would have to verify everyone again; keeping the first device's key gives the same with no re-verification. A separate master key can come later for "hand over the primary role".
- **vodozemac's ECIES channel with a check code** (used for Matrix QR login) for linking: it works, but our invite path already carries a one-time code, Olm and the fingerprint check, and is tested. Revisit if linking by QR from the phone to a laptop turns out awkward.
- **One write per device** instead of a bundle: N writes at once show the same device count, cost more and are easier to correlate.
- **A shared self-queue** for syncing own messages: links all contacts by timing (section 3).
- **History for the new device:** see section 2, point 5.

## Maintainer's decisions (2026-11-18)

1. At most 4 devices, the primary included.
2. Only the primary adds and removes devices.
3. No history on a new device.
4. Two releases: one-to-one chats first, groups in the second.
5. Device labels are seen only by the person's own devices, never by contacts.
