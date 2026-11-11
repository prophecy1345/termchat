# ADR-019: replayed pre-key messages and fallback key age

Status: **accepted** (maintainer, 2026-11-11). Русская версия: [019-prekey-replay.ru.md](019-prekey-replay.ru.md).

## Context

The security review of November 2026
([review-2026-11.md](../security/review-2026-11.md), findings 1 and 2) found
two gaps in how ADR-002 was built.

**Replay.** Every message a sender writes before the recipient's first reply
is an Olm pre-key message. `Client::decrypt` tries the sessions it has; if
none decrypts the message, it calls `create_inbound_session`.

- A session started on a one-time key cannot be created twice: vodozemac
  removes the key on first use.
- A session started on the fallback key can. The key stays in the account, so
  the same pre-key message builds the same session again, the plaintext
  decrypts again, and the new session's empty `seq` tracker accepts it.

So the relay can deliver such a message again, as often as it likes, while the
recipient holds that fallback key. A test confirms it. This breaks item 5 of
the threat model.

**Rotation.** ADR-002 says the client replaces the fallback key when a
pre-key message built on it arrives. The code replaces it when the relay's
`KeyCount` says `fallback_used`. A relay that hands out the fallback key every
time and never says so keeps one key in use for good:

- the replay window never closes;
- the first messages of every new session lose forward secrecy against a
  later leak of that one key.

The relay may hand out the fallback key whenever it wants: the key carries the
owner's signature, and the sender cannot tell why it got it.

## Decision

### 1. A known session is never built twice
A pre-key message whose `session_id()` equals a session the contact already
has, and which that session cannot decrypt, is an error (`DecryptFailed`). It
never reaches `create_inbound_session`.

A new session from the same peer has a new base key, so a new id: honest
traffic is not affected.

### 2. Remember the ids of inbound sessions
Rule 1 does not help once the session is gone: pushed out by the limit of 5
per contact, or removed with the contact. So each identity keeps a list of
the ids of the inbound sessions it created.

- Before `create_inbound_session`, the id of the pre-key message is looked up.
  A known id is refused (`DecryptFailed`).
- After a session is created, its id is added, whoever the sender is and
  whichever key it used. The client does not need to know whether the key was
  the fallback one.
- An entry carries the number of the fallback key that was current when it
  was added. When an old fallback key is forgotten (`forget_fallback_key`),
  entries with a lower number than the current key go: nothing they guard
  against can be built any more.
- At most 200 entries per sender and 10 000 in all; the oldest of that sender
  (then the oldest overall) is dropped first. A sender who starts hundreds of
  sessions only weakens the protection of its own old messages.
- The list is part of the identity's state: it is saved with it, and a backup
  carries it, since a backup carries the fallback keys too.

An id is the 43 characters vodozemac returns. 10 000 of them add about
0.5 MB to the state file at most; a normal account holds a few dozen.

### 3. The fallback key also rotates by age
The client notes when it made the current fallback key. It replaces the key
if the relay's `KeyCount` says it was used (as today) **or** if it is older
than 7 days. The age is checked on every `KeyCount` and also on the client's
own timer while it is connected: the relay chooses when to send `KeyCount`,
so the check must not wait for one.

vodozemac keeps one previous fallback key, and ADR-002 keeps it for the queue
TTL plus a day (15 days). A new rotation waits for that. So in practice a key
is current for about 15 days and accepted for about 15 more, whatever the
relay reports. The client's clock decides, not the relay.

Accounts from before this change have no date for their key: it counts as old
and is replaced at the next connection.

### 4. Compatibility
- No change on the wire and none on the relay.
- The state file gains fields with defaults; an older client that opens a
  newer file ignores them and loses the list.
- Both sides of a conversation benefit separately: each client protects what
  it receives.

## Not a new crypto protocol (rule 1)

Nothing is encrypted, signed or derived differently. The client refuses to
call a vodozemac function twice for the same input, and calls
`generate_fallback_key` on a timer as well as on the relay's word.

## What is left

- A relay can still hand out the fallback key every time. The first messages
  of a session then depend on a key that lives about 30 days instead of on a
  one-time key. Signal's signed pre-key has the same property.
- A relay can still drop messages, and can deliver a replay that the client
  now refuses; the user sees "a message could not be decrypted" in the status
  bar.
- A client that stays offline longer than a key's life keeps that key until it
  connects again.

## Rejected

- **Rotating on the pre-key message itself, as ADR-002 says:** the client
  would need to know which of its keys a message used. Remembering every
  inbound session id covers the replay without that, and the timer covers the
  rotation.
- **Refusing the fallback key on the sender's side:** the sender cannot tell
  a fallback key from a one-time key, and marking them would change the
  signed format.
- **Dropping list entries by date:** a key's life depends on when the client
  is online, not on the calendar. Tying entries to the key is exact.

## Tests

- The replay test from the review: a second and third delivery fail.
- A replay after the session was pushed out by five newer ones fails.
- A replay after the contact was deleted does not bring the request back.
- Forgetting the old fallback key drops the entries that came before it, and
  keeps the rest.
- The caps: the 201st session of one sender drops that sender's oldest entry.
- A fallback key older than 7 days is replaced with `fallback_used = false`;
  no second rotation happens while a previous key is still kept.

## Documents updated

`docs/protocol.md` sections 4 and 7, ADR-002 (fallback key), and the threat
model (item 5, and the note on the start of a session).

## Dependencies

None new.
