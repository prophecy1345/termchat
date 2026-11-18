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

### 3. Messages: a queue per device (amended 2026-11-18)

- **Every device has its own incoming queue for each contact**, made and read by that device alone, on a connection of its own, as now (ADR-023, section 7). The relay needs nothing new.
- **A device tells a contact its address** with the `QueueOffer` that already exists (ADR-023). The contact keeps one address per device on the signed device list.
- **Sending is one ordinary Olm message per device**: to each device of the contact, into that device's queue, and to each other own device, into its queue for the same contact. No new envelope format: each write is what a chat sends today.
- **Your other devices learn what you sent** from their copy. It goes into **their own queue for that contact**, not into a shared queue of yours: those queues are already tied to the contact's on the same connections, so the relay learns nothing new about who talks to whom. A shared "self" queue would link all contacts by timing.
- **Prekeys:** each device publishes one-time keys on its own queues, as now; a contact claims one per device with `ClaimKey` and starts an Olm session with each.
- Answers, edits, deletes, reactions (ADR-025) and documents (ADR-017) travel the same way. An edit or delete from any of your devices applies to your message on all of them.
- **Read marks** are not synced between devices in the first version.

### 4. Groups (second release)

- The first release covers one-to-one chats. Until the second, only the primary takes part in groups; the other devices show "groups are on your main device for now".
- Each device of a member is a reader of the group queue and has its own outbound Megolm session (ADR-027, section 3). Session keys go over Olm to every device of every member.
- The roster stays a list of identities. Next to each identity the admin carries the device list signed by that identity, so members who are not each other's contacts can check devices without trusting the admin for them.
- A device removed from its owner's list is treated as a removed member: everyone rotates their Megolm session.

### 5. Removing and losing devices

- **Removing a device** (on the primary: Settings → Devices → Remove): a new device list without it, and a session rotation in groups. Contacts drop their Olm sessions with that device and stop writing to its queues. The removed device keeps what it already had; it reads nothing new.
- **A secondary device is lost:** remove it from the primary as above.
- **The primary is lost:** the other devices keep working with the current list, but cannot add or remove devices. Restoring the primary from its backup file (ADR-003) brings the identity key back; sessions with contacts start anew. Handing the primary role to another device is left for later.
- **The duress password** wipes the device it is typed on and nothing else, as now. A signed "wipe everything" message to other devices is not part of this ADR: a stolen primary could then erase a person's other devices.
- **Scripts** (ADR-024) keep their own identity and are never linked as a device.

### 6. Older clients

`Understands` (ADR-025, ADR-026) goes to **level 3**: devices. A contact on level 2 or lower does not know devices. With such a contact only the primary talks, with plain Olm messages as now; the other devices show "this contact needs to update to see messages here". Group members below level 3 make the group fall back the same way.

## What the relay sees, honestly

- **The number of devices.** A message to someone with three devices is three writes at once, from one connection, into three queues; the relay can tie those queues together as one person's devices.
- **Device links by IP.** Each device has its own connection per contact. With Tor these are separate circuits; without Tor, IP addresses tie them to each other and to the person.
- **A device is added or removed** when new queues start to receive, or old ones stop, for many conversations at once.
- It still does not see who is in a conversation, the device list, the labels, or a single message.

For contacts: a contact sees how many devices a person has and their keys, never the labels.

## This is not a new cryptographic protocol (rule 1)

- Each device is an ordinary vodozemac Olm account; contacts talk to each device over ordinary Olm sessions.
- The device list is an Ed25519 signature by the identity key, as the roster in ADR-027.
- The link is a contact invite with a one-time code (ADR-013, ADR-023), followed by a fingerprint check by eye.
- A message to several devices is several ordinary Olm messages, one per device, as Matrix sends to-device messages.

## Rejected

- **Copy the whole state** to the second device: two copies of a ratchet break on the first message.
- **Every device a separate identity**, contacts add each: the problem this ADR exists to solve.
- **A new master key** for the identity (as Matrix cross-signing): every contact would have to verify everyone again; keeping the first device's key gives the same with no re-verification. A separate master key can come later for "hand over the primary role".
- **vodozemac's ECIES channel with a check code** (used for Matrix QR login) for linking: it works, but our invite path already carries a one-time code, Olm and the fingerprint check, and is tested. Revisit if linking by QR from the phone to a laptop turns out awkward.
- **One write of a bundle** with a ciphertext per device (the first version of this ADR): a 16 KB message for 7 devices is about 112 KB, past the relay's 64 KB frame and the 20 KB envelope; and every reader would get every copy, N² bytes. Separate writes show the relay nothing it does not see anyway (amendment of 2026-11-18).
- **A shared self-queue** for syncing own messages: links all contacts by timing (section 3).
- **History for the new device:** see section 2, point 5.

## Maintainer's decisions (2026-11-18)

1. At most 4 devices, the primary included.
2. Only the primary adds and removes devices.
3. No history on a new device.
4. Two releases: one-to-one chats first, groups in the second.
5. Device labels are seen only by the person's own devices, never by contacts.

## Amendment of 2026-11-18 (maintainer): a queue per device

Planning the implementation found that the bundle of section 3 does not fit: a long message or a document chunk times the number of devices passes the frame and envelope limits, and every reader would receive every copy. Section 3 now gives each device its own queue per contact and sends one ordinary Olm message per device. The relay protocol and the envelope stay as they are.

## Implementation notes of 2026-11-18 (maintainer)

1. **A channel between one's own devices.** When a device is linked, its link queue becomes its queue for the primary, and the primary makes a queue for it and sends it with `QueueOffer`, as two contacts do. Linking, the contacts handed over, `OwnDevices` and `OwnQueue` go over this channel; messages to contacts never do.
2. **`OwnQueue { contact, queue }`.** A device that makes its queue for contact X tells its other devices over the channel. Copies of what any device sends X then go only to those per-contact queues (section 3).
3. **Each device announces itself to a contact** with `Devices`: the signed list, and only its own address for that contact. The contact keeps one address per device.
4. **Sent** means that at least one of the recipient's devices has the message on its relay.
5. **First release: linked devices talk to the primary only** (maintainer, 2026-11-18). With three or four devices, contacts still write to every device, but a copy of what a linked device sends reaches the primary and not the other linked devices. Since 0.13 the primary introduces linked devices to each other: each sends the other its queue through the primary (`SiblingQueue`), and copies then go directly.

The relay learns one thing more: two devices of one person write to each other, at linking and when a contact is added, not with every message.

## Amendment of 2026-11-18 (maintainer): groups on linked devices

Section 4 said what, not how. How it lands on the group queue of ADR-027:

1. **The roster stays as it is**, one entry per identity, with the reader of the member's primary. Older clients keep reading it.
2. **`GroupDevices { group, member, list, readers }`**, sent by the admin inside the group: a member's device list as that member signed it, and the reader `send_id` the admin made for each of their linked devices. Members check the list's signature themselves, so they trust the admin only for which reader is whose, as they already do for the roster.
3. **A reader per device.** When the admin invites a contact who has several devices (the admin has their list from their `Devices`), it asks the relay for one reader per device and sends each device its own `GroupInvite`, over Olm to that device alone. The person accepts on any device; that device tells its own devices with **`OwnGroupJoin { group }`**, and they take their own pending invites without asking again.
4. **A device linked later** asks for a reader: the member's primary sends the admin **`GroupReaderAsk { group, list }`**, their current device list, over their chat; the admin adds the reader, invites that device, and sends a new `GroupDevices`.
5. **Megolm per device.** Each device has its own outbound session and shares its key over Olm with every device of every member: with contacts' devices over their chats, with others' through their readers. A message names its sending device inside the encryption; receivers take it as the member's only if that device is on the member's checked list.
6. **Removing.** A member removed: every reader of theirs goes, and the group moves as now. A device removed from a member's list: their primary sends the admin the new list (`GroupReaderAsk`), the admin removes that reader and sends `GroupDevices`; everyone starts a new Megolm session.
7. **The admin's own linked devices** are members' devices like any other. Adding and removing people stays on the admin's main device: it alone holds the group's admin token.
8. **Older members.** While any member is below `Understands` level 3, the group works as in 0.12: only primaries take part, and linked devices say so.

What the relay learns: a group queue with one reader per device, so the number of members' devices, as for chats. It still cannot read the names, the lists or the messages.

Not a new cryptographic protocol: Megolm and Olm as in ADR-027, and device lists signed as in section 1.

### Implementation notes for groups (2026-11-18, proposed)

1. **`GroupReaderAsk` is not needed.** The admin is a contact of every member it invited, so it hears each member's new list in their `Devices` (section 3) and acts on it: new devices get readers and invites, removed ones make the group move without them. The kind stays reserved in 0.13 and is never sent; it can go in the next protocol change.
2. **A removed device leaves its groups itself** when it hears its own list without itself; the admin cannot write to it any more.
3. **A device linked later joins by itself** when the invite's roster already holds its identity.
4. **The admin sends the roster again** with each `GroupDevices`: a new device reads only from that point on.
5. **Edits, deletes and reactions** made on another device of ours are not mirrored in groups yet; messages are.
6. **Limits.** The 32 readers of a group are now devices, not people. A move asks the relay for a reader per device at once; past the relay's queue budget for one IP (20 an hour) it waits, as a large removal already did.
