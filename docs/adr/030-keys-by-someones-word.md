# ADR-030: keys known by someone's word, and addresses from peers

Status: **accepted** by the maintainer (2026-11-19), whole. Sections 2, 3, 5, 7 and finding 8 shipped in 0.16.1; sections 1, 4 and 6 in 0.17.0 (level 4). Open: §7, a relay-wide share per origin. Русская версия: [030-keys-by-someones-word.ru.md](030-keys-by-someones-word.ru.md).

## Context

The third source review ([review-2026-11c](../security/review-2026-11c.md)) found that ADR-027 and ADR-028 let a third party say which Curve25519 key belongs to an Ed25519 key:

- a `RosterMember` is vouched for by the admin, a `Device` by the owner of the list; neither carries the consent of the key it names;
- the core stores such a pair like a contact that came with a signed `Identity`;
- fingerprints and safety numbers are computed from the Ed25519 key alone.

From this follow findings 1, 2, 4 and 5: a planted contact with someone else's fingerprint, erased sessions, messages moved between chats, a member removed by a non-admin. Findings 3, 6 and 8 are separate gaps in the same new code.

## Decision

### 1. A device proves that it belongs to the list

`Device` gains `proof`: the device's own Ed25519 signature over `"termchat-device-v1" || identity || curve25519`. `verify_devices` checks it for every entry. A key that did not sign cannot be listed.

- The same shape as `Identity` (an Ed25519 key signs a Curve25519 key), with the identity it joins added. No new primitive (rule 1).
- The new device makes the proof when it confirms the link and sends it with `DeviceLinked`.
- Wire change: `Understands` goes to 4. Lists from level 3 are refused; devices linked under 0.12 to 0.16 are linked again. Devices shipped on 2026-11-18, so few exist.

### 2. The core keeps peers and members apart

- A **peer** is known by a signed `Identity` (invite, contact request, link code, or handed over by our own primary). A **member** is known by a roster or a device list.
- A member entry never counts on an invite queue: `decrypt(None, …)` looks only at peers, and a stranger's request is always checked against its signed identity.
- `add_contact` with a signed `Identity` replaces a member entry for the same key.
- A roster entry with a Curve25519 key that differs from the one we hold for a peer is refused, and the roster with it.

### 3. One key, one owner

A device list is refused if it names a key that is already our identity, one of our devices, a contact, or a device of another contact. Sessions are forgotten only for keys that the list itself brought in. In a group, a device key that is also a roster member's key is ignored.

### 4. A copy says which chat it is for

Messages between our own devices that mirror what was sent to a contact travel as `Body::Copy { contact, body }`; the receiving device files it under `contact` only if it arrived in its queue for that contact. Needs level 4 as well.

### 5. Addresses from peers

- Every address a peer sends (`GroupInvite`, `GroupMove`, `QueueOffer`, `Devices`, `SiblingQueue`, `LinkedContact`) passes one check at receipt: `wss://` only, or `ws://` to an onion host; a loopback address only if our own relay is one (tests, a relay on the same machine).
- With Tor on, an address without an onion URL is kept but not dialled; the chat says the contact cannot be reached over Tor.
- `start_links` never fails as a whole: a link that cannot start is skipped with a note, so the state always opens.
- The threat model says that without Tor a contact who names their own relay learns our IP address.

### 6. Groups notice gaps and refuse strangers on readers

- `Groups::decrypt` returns the Megolm message index; the engine shows "messages missing" per sender device when indexes are skipped within a session, as chats do.
- Olm envelopes on a group reader are read only with sessions of group members and only for `GroupKey`; they never create a contact request.
- The threat model says that a reader past its quota or rate limit loses group messages, and that the admin holds every reader's token.

### 7. Relay

- Reports are limited per queue, not per connection.
- Open question: a lower relay-wide share per origin, so one origin cannot fill the 10 GiB cap.

## Order

Sections 5 (links never fail to start, the address check) and 3 need no wire change and can ship first as a patch release. Sections 1, 2, 4 and 6 follow with level 4.

## Rejected

- **Local rules only, no proof (sections 2 and 3 alone):** stops the attacks found, but a list could still name a key we have not met yet, and every new feature would need to remember the rule.
- **A fingerprint over both keys:** every contact would have to verify again.
