# ADR-027: small private groups

Status: **accepted** (maintainer, 2026-11-17). Русская версия: [027-small-private-groups.ru.md](027-small-private-groups.ru.md).

## Context

termchat.app has one-to-one chats only. People want to talk with a few others at once: a family, a team, the friends who share a relay. PLAN.md, section R, item 3.

Rule 15 forbids public groups, directories, channels and broadcasts. A small private group is none of these: it has a handful of members, nobody can find it, and one gets in only by invitation.

What has to be decided:

1. **How messages travel** between members over anonymous queues (ADR-023), and what the relay then sees.
2. **How they are encrypted**, using only what vodozemac has (rule 1). vodozemac has Megolm, the group ratchet Matrix uses, next to Olm.
3. **Who is in the group**, how people join, are removed and leave, and how every member knows the list is real.
4. What happens with **older clients**.

## Decision

### 1. What a group is

- At most **32 members**, the creator included.
- It has a name and a member list, both known only to members (inside the encryption).
- The creator is its **admin**. Only the admin adds and removes members (section 4). Rule 15 stays: no public groups, no join links that anyone can use, no search.

### 2. Transport: one group queue with a reader per member

A group has **one queue on a relay**, the admin's home relay. The relay protocol (ADR-023) gains readers:

- `CreateGroupQueue` makes a queue with a `send_id` and no reader yet.
- `AddReader` (admin only, with the group's admin token) adds a reader: the relay makes it, as it makes a queue, and returns its `recv_id`, token and its own `send_id` (for key shares to that member alone, section 3). `RemoveReader` removes one. (Wording settled in the implementation, protocol.md section 5.)
- Every message written to `send_id` is kept until **each** reader has acknowledged it, or 14 days, as now.
- Each member subscribes with **their own** `recv_id` and token, on a connection of their own (a circuit of their own with Tor), as for a contact.

The relay learns: this queue has N readers; a message was written at this time and read by each reader at those times. It does not learn who the readers are, their keys, the group's name or the member list. The member's group connection is not linked to their other queues except by IP without Tor, or by timing.

### 3. Encryption: Megolm, keys shared over Olm

This is how Matrix uses the same two vodozemac primitives; nothing is combined in a new way (rule 1).

- Each member has their own **outbound Megolm session** for the group (`GroupSession`) and sends with it. A Megolm message is signed with that session's Ed25519 key; others read it with an `InboundGroupSession`.
- A member hands their session key to every other member **over Olm**, one-to-one. Members who are not each other's contacts set up an Olm session through the group queue: a pre-key message addressed to that member, with their published one-time key (section 4). The group queue carries both kinds: `Megolm` envelopes for everyone and `Olm` envelopes addressed to one member.
- **Rotation:** a member starts a new outbound session when someone leaves or is removed, and after 100 messages or 7 days (Matrix uses similar limits). A new member gets only the current session keys: **no history from before they joined**.
- A key is filed under the member whose Olm channel brought it, and every message names its sender inside the encryption; a message naming anyone else is refused. So a member cannot write as another. Known limit: a member who passes someone else's session key off as their own, before its owner shares it, makes that owner's messages unreadable for the receiver until the next rotation. It does not let them read or forge anything.
- The plaintext inside Megolm is the same `Body` as in a chat (`Message`, `Edit`, `Delete`, `React`, `File`, ADR-025, ADR-017), plus the group's own kinds below.

### 4. Members

- **Adding:** the admin can add only **their own contacts**. The admin sends the contact, in their one-to-one chat, a `GroupInvite`: the group's name, the relay and `send_id`, a fresh reader `recv_id` and token, and the current **roster**. The contact sees "X invites you to group Y" and accepts or declines; nothing happens before they accept.
- **The roster** is the member list with each member's identity key and nickname, numbered and **signed by the admin's identity key**. Every change makes a new roster, sent to all members. A member's app shows only people on the latest roster it has verified, and ignores Megolm sessions from anyone else.
- **Verifying members:** members who are not each other's contacts trust the admin's word for who the others are. The app says so ("added by the admin; not verified by you"), and a member can compare safety numbers with another member as with a contact.
- **Removing:** the admin removes a member with `RemoveReader` and a new roster. Everyone rotates their Megolm session, so the removed member cannot read anything after that. Because they still know `send_id`, the admin also **moves the group to a new queue** (new `send_id`, new readers) and sends its address to the remaining members over Olm.
- **Leaving:** a member says goodbye in the group, the admin removes them as above.
- **The admin leaves or loses their device:** the group stays readable but cannot change any more; members can make a new group. Handing the admin role over is left for later.

### 5. In the apps

Groups appear in the contact list with a mark and a member count. The same answers, edits, deletes and reactions as in a chat, and the same disappearing timer, set by the admin for the whole group. A new group: **Ctrl+P**, then **g** in the terminal; **New group** in the desktop app and in the Android menu.

### 6. Older clients

`Understands` (ADR-025, ADR-026) goes to **level 2**: groups. The admin can invite only contacts that said level 2. A member on an older version cannot be added, and the app says why.

## What the relay sees, honestly

- One queue per group, the number of its readers, and when each message is written and read by each reader. Group size is visible to the relay operator; that is the main new leak.
- With Tor and the default connection per contact, it cannot tell which of its other queues belong to a group's readers, except by timing. Without Tor, a member's IP links their group connection to their other queues.
- A Megolm message and an Olm key share look alike in size buckets, but the key shares come in bursts when someone joins or leaves, which shows membership changes.
- It cannot read the name, the roster, the messages, or who the admin is.

## This is not a new cryptographic protocol (rule 1)

Megolm and Olm from vodozemac, used together as Matrix uses them: Megolm for group messages, Olm to share Megolm keys one-to-one. The roster signature is an Ed25519 signature with the identity key vodozemac already gives. The relay changes are about who may read a queue, not about cryptography.

## Rejected

- **Pairwise fan-out** (as SimpleX does): every pair of members needs queues of its own, up to 32·31 for a full group, and every message is written N−1 times at once, a burst that shows the group to the relay by timing.
- **A shared queue with one token for everyone:** the relay could not tell readers apart, so it could not remove anyone or know when a message is delivered to all.
- **MLS or Signal's Sender Keys:** not in vodozemac (rule 1).
- **History for new members:** sharing old session keys would let a new member read what was said before they came. Off by design.
- **Any member can add people:** keeps the group from growing past what the admin agreed to. Revisit if people ask.

## Maintainer's decisions (2026-11-17)

1. At most 32 members.
2. Only the admin adds and removes members.
3. No history for new members.
4. The group queue lives on the admin's relay.
5. The relay operator seeing a group's size is accepted.
