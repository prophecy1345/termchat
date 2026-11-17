# ADR-026: saying `Understands` again

Status: **accepted** (maintainer, 2026-11-17). Русская версия: [026-understands-again.ru.md](026-understands-again.ru.md).

## Context

ADR-025, section 6: an updated client sends `Understands { level: 1 }` once to every contact, and remembers that it did (`told`). A chat offers answers, edits, deletes and reactions only after the contact has said level 1.

"Once" loses the message when the two sides update at different times:

1. Alice updates to 0.9.0 and tells Bob. `told` is set as soon as the message is handed to the relay.
2. Bob is still on 0.8. He cannot read it ("could not be decrypted", "1 message lost") and stores nothing.
3. Bob updates to 0.9.0 and tells Alice. Alice now knows Bob reads the new kinds.
4. Alice never tells Bob again: `told` is already 1. For Bob, Alice stays an old client for good.

What people see, both on 0.9.0:

- Bob gets no answers, edits, deletes or reactions in this chat at all.
- Alice gets them only for her own messages: Bob still sends her plain `Text`, which has no id to point at.

Only the order of the two updates decides this. Nothing is lost and nothing is exposed; the chat just stays on the old kinds.

## Decision

### 1. News is answered

When `Understands` arrives with a level higher than the one stored for that contact, the client stores it, forgets what it has told that contact (`told = 0`) and sends its own `Understands` again with the next flush, which starts at once.

- The contact has just shown that it reads `Understands` now, so this copy arrives.
- No loop: the same level a second time is not news and is not answered. Two updated clients exchange at most one extra `Understands` each, once per contact.
- A stranger whose request is not accepted still hears nothing (ADR-025).
- A higher level in a later release is answered the same way.

### 2. Pairs already stuck: one repair

Rule 1 needs a new `Understands` to arrive, and in a stuck pair both sides have already sent theirs. So the first time 0.9.1 opens an account, it forgets `told` for every contact that has already said level 1. A flag in the app data (`understands_resent`) keeps it from happening twice; a new account starts with the flag set.

- Only contacts known to read `Understands` get it again, so nobody on 0.8 sees a second "could not be decrypted" line.
- A stuck pair heals when the side that updated first (Alice above) runs 0.9.1; the other side may stay on 0.9.0.

## What the relay sees, honestly

One more message of the smallest size per contact, once, at the first start of 0.9.1 or when a contact updates. It looks like any short message. It may tell the relay that a client was just updated, as the first `Understands` already did.

## This is not a new cryptographic protocol (rule 1)

No new kind of message and no change on the wire: the same `Understands`, sent one more time. Olm, vodozemac and the relay do not change.

## Rejected

- **Sending `Understands` on every connection.** Works, but every contact still on 0.8 would see "could not be decrypted" each time.
- **Waiting for the relay's `Sent` before setting `told`.** The relay stored the message; that says nothing about whether the contact's app could read it.
- **An acknowledgement kind (`Understood`).** A new kind for what the contact's own `Understands` already shows.
- **Forgetting `told` for every contact in 0.9.1.** Costs contacts on 0.8 a second false "message lost" and repairs nothing more: a side that has not heard from its contact is not the side whose message was lost.
