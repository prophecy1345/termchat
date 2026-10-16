# ADR-009: leaving: goodbye to contacts and removal from the relay

Status: **accepted** (maintainer, 2026-10-16). Date: 2026-10-16. Русская версия: [009-leaving.ru.md](009-leaving.ru.md).

## Context

"Wipe everything from this device" erases the state file (ADR-003), but the
relay still knows the key. The account lives on until it has been idle for
90 days (`account_ttl`). Until then contacts keep writing: the relay accepts
and queues their messages for 14 days, the sender sees "Sent", and nobody will
ever read them. Contacts who were approved should learn that the person left.

The duress password must not change: it wipes at once, silently and without the
network (ADR-003). A "fresh install" that immediately talks to the relay would
show anyone watching the network that the duress password was used.

## Decision

### 1. The goodbye message
A new inner body, `Body::Goodbye`, with no fields. It goes through the existing
Olm session, like any text, so only the contact can read it. It is padded to
the smallest bucket (protocol.md, section 8), so the relay sees an ordinary
small `Send`.

It goes to every contact with a session who is not blocked and whose request
is not pending. Strangers and pending requests get nothing.

### 2. Removal from the relay
A new frame, `ClientFrame::Unregister`, allowed only on an authenticated
connection. The relay:

- deletes the `accounts` row; `ON DELETE CASCADE` removes the prekeys and the
  incoming queue, and the queue byte counter is reduced accordingly;
- forgets the in-memory state of the key (limits, recipients) and its live
  connection;
- answers `ServerFrame::Unregistered` and closes the connection.

A banned key cannot authenticate (ADR-006), so it cannot erase its ban this way.
`Unregister` counts toward the per-key frame budget like any frame.

After that, `ClaimKey` and `Send` to the key answer `UnknownRecipient`.

### 3. Order on the client (menu → wipe)
1. Send `Goodbye` to every contact from section 1 and wait until the relay
   accepts each one (`Accepted`), at most about 10 seconds.
2. Send `Unregister` and wait for `Unregistered`.
3. Wipe the device as before (`data::wipe`).

If the relay cannot be reached or does not answer in time, the client asks:
"No connection to the relay. Wipe without telling your contacts?" Yes wipes
silently, no cancels. If the relay accepted the goodbyes but does not know
`Unregister` (an older relay), the client says so and wipes anyway: the
account expires there after 90 days.

### 4. The duress password sends nothing
Nothing changes in ADR-003: erase first, no network, the first-run screen.
Contacts learn later and only indirectly: once the relay forgets the idle key
(90 days), their sends fail with `UnknownRecipient`.

### 5. What the contact sees
- On `Goodbye`: a system line "‹name› deleted their account" in the chat; the
  contact is marked as left (`left`, stored in the encrypted state). The chat
  becomes read-only: the input is disabled and says "this contact left". The
  history stays until the user removes the contact.
- On `UnknownRecipient` for a message: the message shows "contact is no longer
  on the relay" instead of the error code. The contact is **not** marked as
  left automatically: a relay that lost its database answers the same way.

## Metadata, honestly

- The relay sees a burst of small sends followed by `Unregister`. It learns that
  this key left and to whom it wrote last; it already sees who writes to whom.
- A network observer sees ordinary traffic.
- Nothing new on the duress path.
- `Unregister` removes data from the relay: it is a reduction of what the relay
  keeps (rule 14).

## Compatibility

- An older relay does not know `Unregister`: it answers `Malformed` and closes
  the connection. Handled in section 3.
- An older client does not know `Goodbye`: the Olm layer decrypts it, the inner
  message does not parse, and the chat shows "a message could not be decrypted
  (malformed)". That is acceptable for the transition; nothing is lost.

## Alternatives rejected

- **Goodbye on the duress path too**, sent in the background after the wipe:
  network traffic right after a "first run" gives the duress password away
  (section 4).
- **Only `Unregister`, no goodbye**: contacts would find out only when a send
  fails, and only if they write.
- **Marking a contact as left on `UnknownRecipient`**: a relay that lost its
  data or a misconfigured relay would make every contact "leave".
