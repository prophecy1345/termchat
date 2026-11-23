# ADR-042: several group admins, handing over and removing the role

Status: **accepted** (owner, 2026-11-23). Русская версия: [042-group-admins.ru.md](042-group-admins.ru.md). Changes [ADR-027](027-small-private-groups.md), section 4 ("handing the admin role over is postponed"), and builds on [ADR-028](028-second-device.md) (devices in groups) and [ADR-030](030-keys-by-someones-word.md) (keys on someone's word).

## Context

A group has exactly one admin: its creator. Only they add and remove members, set the disappearing timer, hold the admin token of the group's queue on the relay and sign the roster with their identity key. If they leave or lose their device, the group stays readable but can no longer change.

PLAN.md P10 asked for handing the role over. The owner widened it (2026-11-23):

1. **Add an admin** to an existing one, from among the group's members.
2. After that **the first admin can stay or be removed**, but only they can remove themselves. Handing over is a special case: name a new one and leave the role.

The feature is risky: it changes whose signature members trust and gives the queue token to more people. Hence the separate audit section below.

## Decision

### 1. The main admin and helpers

- The roster gets an **ordered** list **`admins`**: 1 to **4** identity keys, each from the member list. **The first one is the main admin** (at first, the creator). The rest are **helpers**, in the order they were made. The `admin` field stays and means "who signed this roster". The signer must be in `admins`.
- **Chain of trust.** A member accepts roster N+1 only if it is signed by an admin from the **last roster they accepted**, N, and the signer had the right to every change between N and N+1 (table below). The first roster comes in the invite, as now: the inviter signs it and must be in `admins`.
- A group **always has a main admin**. A roster with an empty `admins` is invalid.

**Who can do what.** Each member checks this themselves by comparing N and N+1; they do not trust the signer:

| Change between N and N+1 | Main | Helper |
|---|---|---|
| Invite one's own contact | yes | yes |
| Remove an ordinary member (one who left included) | yes | no |
| Remove an admin from the group | a helper, yes | no |
| Make a helper | yes | no |
| Remove a helper's role | yes | only their own |
| Remove the main admin's role or membership | only their own | no (except section 4) |
| Group name, disappearing timer | yes | no |
| Order of `admins` | only when handing over (section 4) | no |

Until a group actually has several admins, everything stays as in ADR-027: one main admin, who is the creator.

### 2. Who can be made admin

- **Only the main admin** makes helpers, and only from **members of the group** whose devices all report `Understands` level 6 or higher (section 7).
- A helper can only **invite their own contacts**, like the creator (ADR-027, section 4). They cannot remove anyone except themselves from `admins`. The main admin takes a member who left (`GroupLeave`) off the roster.
- Members who are not the helper's contacts trust them in what they trusted the creator with: who is in the group. The ADR-030 checks (a member we know as a contact must have the contact's key; a device list is signed by the member themselves) apply to a roster from any admin.

### 3. Rights over the relay queues

The relay protocol **does not change**. The main admin passes the admin token of the group's queue (`AddReader`, `RemoveReader`) to a helper over Olm, one to one, in a new kind of message **`GroupAdminKeys { group, roster_no, queue, token }`** inside encryption. Only the helper's **primary device** receives it (ADR-028, section 7). The receiver accepts it only if:

- they have already accepted roster `roster_no` or newer with themselves in `admins`;
- it came over Olm from a device on the device list (ADR-028) of an admin of that roster;
- `queue` is the group's current queue.

Otherwise it is dropped silently. The token is stored like the creator's token now: only encrypted at rest, `Zeroize`, no `Debug` showing the value, never in logs or errors (rule 6).

### 4. The main admin leaves: hand over or exit

Nobody but the main admin can remove the main admin. There are two ways:

- **Hand over** ("Hand over admin"): the main admin signs a roster with the chosen member first in `admins`, and themselves either dropped from `admins` (staying a member) or removed (leaving the group). If the successor was not a helper, this also makes them admin. The main admin then moves the group (section 5) and passes the new queue's token to all remaining admins.
- **Exit without naming a successor** (the main admin leaves the group with `GroupLeave`): **the next one in `admins`**, the helper made earliest, becomes main. With no helpers, the group, as now, stays without an admin and cannot change.

In the second case the roster is signed not by the main admin but by their successor. So that a helper cannot announce the main admin's exit on their own, the main admin's exit carries a **signed statement**: `GroupResign { group, roster_no }`, signed by the main admin's identity key with its own signing context (like the roster signature). The successor puts it into their roster N+1. A member accepts a roster without the main admin only if:

- the main admin of N signed it, **or**
- it carries the main admin's `GroupResign` from N with `roster_no == N` and a valid signature, and the next one in `admins` signed it.

The statement is bound to the group and the roster number, so it cannot be reused or applied to another group. It is checked from the roster itself, without the queue: a member who was offline checks it the same way.

**If the main admin lost their device** without handing over, nobody can remove them. Helpers can only invite new members. Nobody can be removed, those who left stay on the roster, and the name, the timer and the admin list stay as they are. That is the price of protecting the main admin, and the app says so.

### 5. Removing a helper, and the move

A removed helper stays a member but **knows the token**. So after any admin's role or membership is removed, the group **moves to a new queue**, as after removing a member (ADR-027, section 4). The signer of the new roster does the move. The new queue is created on their home relay with a new token, which goes only to the remaining admins in `GroupAdminKeys`. The old queue is deleted: the signer removes all its readers (`RemoveReader`). Megolm sessions change only if the removed admin left the group. If they stay a member, they keep reading.

If the removed admin called `AddReader` or `RemoveReader` before the move, it only affects the old queue. They do not know the new one.

### 6. Two admins change the group at once

Rosters travel through the group's queue (`Body::GroupRoster` inside Megolm). One queue has one order for all readers. The roster N+1 **written to the queue first** wins. Everyone rejects the second one with the same number by the existing `roster.no <= current` rule. The loser sees the other N+1 before their own, learns their change was not taken and offers to repeat it on top: "change not applied: X was changing the group at the same time; repeat?". A reader they added on the relay but did not get into an accepted roster they remove (`RemoveReader`).

### 7. Old clients

- `Understands` goes to **level 6**: several admins.
- While `admins` holds one key and it is the creator, the roster goes in the old format. Single-admin groups do not change for old clients.
- A roster with `admins` is a **new kind of message**. An old client would not understand it and would stop seeing changes to the group. So making a helper or handing over is possible only when **all devices of all members** report level 6. Otherwise the app names who must update. A group that already has several admins, or whose main admin is not the creator, accepts invitations only to contacts at level 6.
- **No format rollback.** Once a client accepts one roster of the new kind for a group, it never again accepts an old-kind roster for it.

### 8. In the apps

The main admin sees **Make admin** in a member's menu and **Remove admin** in a helper's. A helper sees **Step down** for themselves. The main admin sees **Hand over admin**: pick a member, then "stay in the group or leave". The member list marks the main admin and the helpers differently. Everyone sees system lines in the chat: "X made Y an admin", "X removed Y as admin", "Y stepped down as admin", "X handed the group over to Y", "X left; Y is now the main admin". Every change of rights is visible to all: an admin cannot be made or removed in secret.

## What the relay sees, honestly

- The relay does not see a helper being made: the token travels inside encryption and the relay is spoken to with the same token.
- It sees removing an admin or handing over as a **group move** to a new queue, the same as after removing a member. It cannot tell them apart.
- If the new queue is on another relay (the signer's home relay), the old relay's operator sees the group leave. The new one sees it arrive, and its size.

## Not a new crypto protocol (rule 1)

The roster and `GroupResign` signatures are Ed25519 with identity keys through vodozemac, like the roster signature now, each with its own signing context. Only **whose** signature to trust changes: the rules of the table in section 1 instead of the creator's single key. The token travels over Olm, like Megolm keys and invites. Megolm and Olm are used as in ADR-027.

## Security audit

The attacker in each item is one we expect to meet: a malicious helper, a removed admin, a member, the relay. For each: what they can do, what stops them, and what remains.

### A1. A helper tries to take the group over

- **Remove the main admin's role or membership.** Section 1 table: a member rejects a roster where the main admin is gone or not first, unless the main admin signed it or it carries their `GroupResign`. Every client checks this, not the signer.
- **Fake the main admin's exit.** `GroupResign` is signed by the main admin's identity key and bound to the group and the roster number. A helper cannot create one, and an old one cannot be reused.
- **Make an accomplice admin, or reorder `admins`** to become next. Only the main admin makes admins and changes the order.
- **Change the name or the timer** (for example, turn disappearing off). Main admin only.
- **Remove the other members.** Not possible: only the main admin removes. A helper can only invite their own contacts, everyone sees it as a line in the chat, and the main admin can remove both those invited and the helper.

**What remains: cutting someone off at the relay.** A helper knows the queue token and can call `RemoveReader` on the main admin's reader, or any member's, without touching the roster. That person stops getting the group's messages, and the others do not see it. Cryptography cannot prevent this: the relay does not know whose reader is whose (ADR-023) and cannot check the roster. Defences:

1. Every client **notices losing its own reader**: the relay answers the subscription with "no such queue". The app says "you were cut off from the group on the relay" and suggests asking the main admin. This exists for lost queues (`queue_lost`); groups only need a clear line.
2. A main admin cut off this way is still main for every member. They create a new queue and move the group there: the signer of the new roster does the move, and only they can sign it.
3. Abuse shows in its effects, and the main admin should make helpers only of people they trust. The hint on **Make admin** says so.

The rejected alternative, separate relay tokens with rights only for the main admin, would show the relay the group's structure (see "Rejected").

### A2. A helper moves the group into their own queue without the main admin

A move is a message from an admin telling members the new queue's address. If a helper moves the group without the main admin, the main admin keeps writing to the old queue that nobody reads.

Defence: a member accepts a move only together with a **new roster signed by the same admin** in which every member, the main admin included, has a new reader (`RosterMember.reader`). If the main admin is missing from that roster, it is rejected by A1. If the main admin is in it but no reader was created for them on the relay, that is A1's "cutting off at the relay", and the main admin sees their own reader lost.

### A3. A removed admin

- **The old token** works only for the old queue. The group moved (section 5) and the old queue is deleted.
- **Rosters in their name.** They are no longer in `admins` of the last accepted roster, so their rosters are rejected.
- **Invites with an old roster.** They can send a contact an invite with an old roster, where they are still admin, and the old queue's address. The invitee joins a dead group where nobody writes. The other members are not harmed. Limit: the invitee cannot know the roster is stale. This remains; the app will show "no messages in the group for a long time".
- **Race: removed, but sign something quickly.** Anything they signed after the accepted roster that removed them is rejected by number.

### A4. Passing the token

- **A forged `GroupAdminKeys`** from a non-admin, or from an admin before a roster made the receiver admin: dropped (section 3).
- **The token goes to the wrong device.** Only to the primary device on the helper's own signed device list (ADR-028). Linked devices neither get nor ask for it.
- **A leaked device of any admin** gives the current queue's token. It was one device; it becomes up to four. The limit of 4 and the main admin's power to remove a helper (with a move) bound this.
- **The token in logs, errors, debug output.** The token type has no `Debug` showing the value and is `ZeroizeOnDrop`. A negative test checks that `format!("{:?}")` does not reveal it.

### A5. Rollback and forks of the roster

- **An old roster replayed** (with the previous `admins`): rejected by number.
- **An old-format roster** after the new one: rejected (section 7, "no format rollback"). This closes going around the table through the old `sender == admin` check.
- **Two N+1 branches:** one queue order for all (section 6), and everyone takes the same one. A new member gets the roster from the invite, signed by the inviter. If the inviter is on the losing branch, the invitee moves to the shared branch with the next roster signed by an admin from their roster, or stays in a dead branch as in A3.
- **A member offline longer than 14 days** who missed rosters cannot rebuild the chain. They wait for a roster resend (ADR-028, decision 4). The resend must carry **the whole chain of `admins` changes since N**, or they will reject a roster signed by a helper they do not know. The shortcut "accept the latest if signed by the main admin of N" is safe only if the main admin did not change. Otherwise the chain is needed.

### A6. Substituting members' keys (ADR-030)

A helper, like the creator, vouches for members' keys. A malicious helper could put a key into the roster as someone we know. The ADR-030 checks apply to a roster from any admin: a member who is our contact must have the contact's key, and members we have not verified are marked "added by admin X; you have not verified them". The only new thing: the mark names the specific admin, not "the admin".

### A7. The relay

The relay can reorder, delay and drop entries in a queue, but cannot forge them. It can reorder two N+1 rosters only the same way for all readers of one queue: one queue has one shared order. It could give readers different orders if the queue stores entries per reader. Then different members would accept different N+1 branches. Defence: the next roster N+2 from any admin carries the chain (A5); a member on the other branch sees the mismatch and shows "the group's roster disagrees, ask the main admin". This does not fully protect against a malicious relay, as today: it can also simply not deliver.

### What tests must cover before release

Negative tests for every "no" in the section 1 table. A forged, foreign and reused `GroupResign`. `GroupAdminKeys` from a non-admin, before the roster, to a linked device, for another queue. An old-format roster after a new one. A race of two N+1 rosters. The chain after a gap. Fuzzing the parsing of the new roster, `GroupResign` and `GroupAdminKeys` (rule 4). A check that the token does not show in `Debug`. Before release, a separate review of this code.

## Rejected

- **All admins equal** (the first draft of this ADR): anyone could remove the creator. The owner chose to protect the main admin.
- **Several admin tokens on the relay** (one each, `RemoveReader` for the main admin only): the relay would learn how many admins a group has, the relay protocol would grow, and the move on removal is still needed because the removed one knows `send_id`.
- **Admins voting** on changes: much code and state for groups of at most 32.
- **Handing over without a move:** the removed one would keep the token and could keep adding readers on the relay.
- **Helpers removing a main admin who stays silent for long:** any time threshold is a way to take the group while the main admin is on holiday.

## Clarified during implementation (2026-11-23, approved by the owner)

The roster format (`Roster`) **does not change**. The admin list is a separate signed object, **`AdminList { group, no, signer, admins, resign, signature }`**: `no` is the roster it came with, `signer` an admin of the list before, `resign` the main admin's `GroupResign` when their successor signs. New kinds of message:

- **`GroupAdmins { group, chain }`**: a chain of lists, oldest first, at most 8. It goes right before the roster in the same channel: in the group, in an invite, in a move. A member who fell behind checks the chain from their last list (A5) without getting whole old rosters: a roster of 32 members is about 5 KB, a chain of 8 lists about 3 KB.
- **`GroupResign { group, resign }`**: the main admin's statement, in the group, before their `GroupLeave`.
- **`GroupAdminKeys { group, roster_no, grant }`**: the token, over Olm only. Proto refuses it inside the group and inside a `Copy` to one's own devices.

A list takes effect only when the roster with the same `no` from the same `signer` is accepted. A list whose roster lost the race (section 6) is dropped with it. The roster's `admin` field is its signer, who must be in the list in force.

This changes section 7: instead of "a new kind of roster", `GroupAdmins` next to the old roster. An old client rejects a roster not signed by the creator, so the "all devices at level 6" rule stays. "No format rollback" is no longer needed: there is one format. Instead: while a group has an accepted `AdminList`, a roster without a matching list from the same signer is checked against the last accepted list, not against the old roster's `admin` field.

## Amendment 2 (2026-11-23, approved by the owner): the reader channel and who moves the group

Found during implementation. One to one, an admin talks to a member over their chat (`GroupMove`, `GroupLeave` on removal, `GroupInvite`), so only to contacts. The reader channel (Olm with a one-time key claimed from the reader, ADR-027, section 3) carries only `GroupKey` now. With one admin that was enough: they invited everyone, everyone is their contact. With helpers it is not: members a helper invited are not the main admin's contacts, so on a move the main admin could not send them `GroupMove` and they would drop out of the group.

1. **The reader channel also carries `GroupMove`, `GroupLeave` (removal) and `GroupAdminKeys`.** The receiver accepts them only if the Olm sender is a device of an admin in the list in force, and for `GroupMove` and `GroupLeave` of the **main** admin. Anything else on this channel is dropped, as now. The sender's Olm key comes from the roster an admin signed (as for `GroupKey` now), so nothing new is trusted: a member already trusts the roster for others' keys.
2. **Only the main admin in force moves the group.** Less code, and it closes A2 almost entirely: a helper cannot take the group away, because nobody accepts their `GroupMove`. Hence:
   - **A helper steps down:** they sign the list without themselves and a roster. The main admin, accepting it, moves the group. Until then the former helper still knows the old queue's token. They left on their own, and the window lasts until the main admin's next connection.
   - **Handing over:** the main admin signs a list and a roster with the successor first. If the successor was not a helper, the main admin first sends them `GroupAdminKeys`. The **successor** then moves the group: they sign the next roster, the former main admin no longer can.
   - **The main admin leaves with `GroupResign`:** the successor moves the group, as in section 4.
3. **Only the main admin sends `GroupAdminKeys`**, only to those in their new list, over their chat or the reader.

What the relay sees: nothing new. Olm envelopes already appear in a reader's queue (key transfer), and a `GroupMove` looks like them in size and timing.

Read sections 3 to 5 and A2 with this amendment: where they say the signer moves the group, the main admin does.

## Amendment 3 (2026-11-23, the owner chose option A): the old queue on a move

Found during implementation. Until now the main admin deleted the old queue right after a move (`DeleteGroup`), and that protected members: one who had not yet heard of the move could not write where the removed one would read it. With helpers this cannot stay: members a helper invited hear from the main admin through their reader (amendment 2), the reader lives in the old queue, and `DeleteGroup` would wipe the `GroupMove` before it is read.

1. **Relay: a new frame `RemoveReaderAt { group_id, token, send_id }`.** The same as `RemoveReader`, but the reader is named by its `send_id`: the main admin knows it from the roster for every reader, those a helper made included, while the `recv_id` only for their own. The relay looks for the reader only in this group and only with the admin token. It learns nothing new: it knows the reader's `send_id` already.
2. **A move:** the main admin removes the removed ones' readers at once (`RemoveReaderAt`), sends `GroupMove` (to contacts over the chat, to others through their reader in the old queue) and deletes the old queue once every remaining member has shown up in the new one (sent a key or a message), but **no later than 24 hours**.
3. **Read first, then write:** after connecting to a group, a client sends what it has queued there only once it has read everything waiting in its reader. A member who fell behind a move gets the `GroupMove` first and writes to the new queue.
4. **What remains:** a removed **admin** knows the old queue's token and, while it lives (up to 24 hours), can add a reader for themselves. They would read only what stragglers write there with the old Megolm session, and point 3 makes that nearly impossible. A removed ordinary member has no token and after `RemoveReaderAt` reads nothing.
5. **An old relay** does not know `RemoveReaderAt` and answers with an error. The main admin then deletes the old queue at once, as before, and members a helper invited who are not the main admin's contacts drop out of the group. The main admin's app warns about this. The termchat.app relay is updated with the release (ADR-010).

## Owner's decisions (2026-11-23)

1. The creator (main admin) cannot be removed: they only leave themselves.
2. At most 4 admins.
3. Only the main admin changes the name and the timer, and after they leave, whoever became main after them.
4. Special attention to the security audit: the feature is risky.
5. Only the main admin makes helpers.
6. A helper can only invite; only the main admin removes.
