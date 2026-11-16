# ADR-023: anonymous queues on the relay

Status: **accepted** (maintainer, 2026-11-16). Русская версия: [023-anonymous-queues.ru.md](023-anonymous-queues.ru.md).

## Context

Today the relay knows the social graph ([threat model](../threat-model.md), "Relay operator" and "Partly protected"):

- **the sender**: every connection passes `Auth` with the identity's Ed25519 key, and the relay sets `sender` in `Deliver` (protocol, section 5);
- **the recipient**: `Envelope.recipient` is the recipient's Ed25519 key (section 6), and `ClaimKey` names its `target` (section 4);
- **the time** of every message and every connection.

Size is already hidden by buckets, and IP by Tor (ADR-004). What remains is the graph of who writes to whom.

Incognito contacts (ADR-014) give a contact a separate identity, but:

- they are turned on by hand for each contact; for the main identity the relay sees the whole circle of contacts;
- the relay still stores the key of every identity and authenticates the sender.

Sealed sender was rejected in ADR-014 (section 5): with incognito it hides little, and abuse protection without knowing the sender needs cryptography that vodozemac does not have.

The maintainer's question: why does the relay need to know who talks to whom at all?

Answer: the relay cannot move bytes without seeing **when** they move. A packet arrives and leaves at a specific moment. But it does not have to know **whose** bytes they are. The SimpleX messenger shows that a relay can work with anonymous queues only, with no accounts and no user identifiers.

## Decision

### 1. Queues instead of accounts

The relay has no accounts. It has queues. A queue has:

- `recv_id`: 16 random bytes. The owner fetches messages with it.
- `send_id`: 16 different random bytes. Others write to the queue with it.
- `recv_token`: 32 random bytes, the queue owner's secret.

The relay links `send_id` and `recv_id` of one queue. It stores no Identity and no Ed25519 key of the user. Two different IDs mean that someone watching the wire (if TLS is broken or terminated by a third party) cannot match writes to reads.

```
C -> R  CreateQueue
R -> C  QueueCreated { recv_id, send_id, recv_token }
C -> R  DeleteQueue { recv_id, recv_token }
```

The relay generates the IDs and the token, so a client cannot pick the ID of someone else's queue. Queue creation is limited like registration is today: per IP, and by a shared budget for onion (protocol, section 11).

### 2. A pair of queues per contact

A person has a separate incoming queue for each contact. They pass its `send_id` to the contact only inside Olm, in a new inner message variant:

```
Body::QueueOffer { relay_url: String, onion_url: Option<String>, send_id: [u8; 16] }
```

- Each side writes to the other's queue and reads from its own. This gives a pair of queues, one per direction.
- The recipient knows which contact is writing by the queue the message arrived in. The `sender` field in `Deliver` goes away. The client does not trust it anyway: decryption and `Inner.sender` confirm the sender (protocol, section 7). Those checks stay.
- `Envelope.recipient` goes away: the address is the `send_id`. The recipient still checks `Inner.recipient` against its own key.

### 3. Sending without authentication

```
C -> R  Send { client_id, send_id, envelope }
R -> C  Accepted { client_id } | Error { client_id, code }
```

Sending needs no `Hello`, `Challenge` or `Auth`. Anyone who knows the `send_id` can write to the queue. Only the contact knows it, because the ID travelled inside the ciphertext.

### 4. Subscribing to your own queues

```
C -> R  Subscribe { recv_id, recv_token }
R -> C  Subscribed { recv_id } | Error { code }
R -> C  Deliver { recv_id, relay_id, envelope }
C -> R  Ack { recv_id, relay_id }
```

Delivery and acknowledgement work as today (ADR-007, ADR-020). Push (ADR-012) is tied to a queue, not an account: `SetPush { recv_id, recv_token, endpoint }`.

### 5. Invite queue and prekeys

An invite has its own queue: strangers write to it.

```
Invite { version: 3, relay_url, onion_url, identity: Identity, invite_send_id: [u8; 16], code: [u8; 16] }
```

- Prekeys are published on the invite queue: `PublishKeys { recv_id, recv_token, one_time, fallback }`.
- A stranger claims a key by `send_id`: `ClaimKey { send_id }`. They check the `SignedKey` signature with the key from `identity` in the invite, as today.
- The first message (`ContactRequestV2` with the ADR-013 code) goes to the invite queue. Once the request is accepted, the two sides exchange `QueueOffer` and from then on talk through the contact's pair of queues.
- To revoke an invite, the person deletes the invite queue and creates a new one.

The relay sees that someone writes to an invite queue, but does not know whose queue it is or who writes.

### 6. Abuse (ADR-006, ADR-007)

What changes:

- **Limits.** Per-key limits (`Send`, frames, new recipients per day) become per-queue and per-connection limits. The "new recipients per day" limit loses its meaning: the relay does not know the sender.
- **Blocking a contact** is deleting their incoming queue. This is stronger than today: the blocked person can no longer put a single byte in it, and it takes no space in the recipient's queue.
- **Spam from strangers** is possible only into the invite queue. The stranger quota from ADR-007 becomes the invite queue's quota. If an invite is used for spam, the person re-creates the queue.
- **Prekey exhaustion.** The "`ClaimKey` to one target" limit counts per invite queue. The "`ClaimKey` from one key" limit becomes a per-connection and per-IP limit.
- **Reports and bans (a loss).** The relay does not know the sender's key, so it cannot ban a key. A report still reaches the operator with the disclosed text, but the operator can only limit the connection or IP that writes to the queue. The automatic ban after K reports (ADR-006) goes away. Protection rests on invites, blocking (deleting the queue) and limits.

### 7. Connection isolation (the main trade-off)

Anonymous queues give nothing if the relay can link them by connection:

- one connection is subscribed to queues A, B, C, so they belong to one person;
- the same connection writes to queues X, Y, so this person talks to the owners of X and Y.

That way the relay builds the graph again. Therefore:

- **Per-contact isolation (default):** reading your queue and writing to the contact's queue for one contact go over a separate connection and a separate Tor circuit. The cost is the same as incognito (ADR-014): a connection per contact, battery, startup time.
- **Shared connection:** cheaper. The relay sees "these queues belong to one person", but not who is on the other end, unless the contact is also not isolated. Without Tor, the IP links all connections.

Maintainer's decision: per-contact isolation by default. The user can choose a shared connection in settings.

### 8. Compatibility and migration

- The relay protocol gets `version = 2`. **The relay does not support version 1** (maintainer's decision): there is no transition period. A version 2 relay answers old clients with `UnsupportedVersion`, and they show the update notice (ADR-010).
- On upgrade the relay erases version 1 accounts, keys, prekeys, push endpoints and queues. Key bans lose their meaning and are erased too.
- **Existing contacts are cut off.** A `QueueOffer` cannot travel through a version 1 relay, because there is none. To keep talking, both sides exchange version 3 invites again.
  - If a new contact request comes from the same Identity as a saved contact, the client attaches it to that contact: history, nickname and the verified fingerprint stay. Without this the person would see a duplicate.
  - Old Olm sessions with that contact are deleted: the new exchange starts a new session.
- Old invites of versions 1 and 2 are not accepted; the client asks the contact for a new one.
- The release notes say this honestly: after the update, every contact has to exchange invites again.
- New enum variants go at the end, as in ADR-012.

## What the relay knows after the change

| | Now | After |
|---|---|---|
| User keys | all | none (except those still on version 1) |
| Who sent | yes | no |
| Addressed to | recipient key | random queue ID |
| Which queues belong to one person | — | only by shared connection, IP or timing |
| Time of writes and reads | yes | yes |
| Size | to the bucket | to the bucket |
| IP | without Tor | without Tor |

## Metadata, honestly

After the change, this remains:

- **Time.** The relay sees when a queue was written to and when it was read. Timing correlation (a message into X, a read from A one second later) can link queues. Only a mixnet with cover traffic removes this; it stays under "Not protected".
- **Links through connection and IP** without isolation (section 7).
- **Size** to the bucket; a document (ADR-017) is a series of 16 KB messages.
- **A pre-key message** is longer than a normal one; the relay sees a new session start in a queue.
- **The number of queues** on one connection roughly tells the number of contacts, if the connection is shared.

## Not a new cryptographic protocol (rule 1)

- `recv_id`, `send_id` and `recv_token` are random bytes sent over TLS (and inside Olm for `send_id`). No new combination of primitives.
- Olm, vodozemac, the Identity and `SignedKey` signatures do not change.
- The relay stores the SHA-256 of `recv_token`, not the token (maintainer's decision). A seized database does not let anyone read or delete queues. This is an ordinary use of a hash, not a protocol. Comparison is on the hash, in constant time. `sha2` is already a dependency of `core`.

## Rejected

- **Sealed sender with anonymous credentials** (as in Signal): abuse protection needs blind signatures or anonymous tokens. vodozemac has none, and writing our own is not allowed (rule 1).
- **Rotating IDs derived from a shared secret with a KDF:** combining primitives into our own protocol (rule 1). Random IDs and an explicit `QueueOffer` give the same without new cryptography.
- **Mixnet and cover traffic:** would hide timing, but cost latency, traffic and battery. Too expensive for the project now.
- **Incognito for all contacts only:** cheaper, because the protocol does not change. But the relay still stores keys, authenticates the sender and sees pairs. It stays as the fallback if this ADR is not accepted.
- **Private information retrieval** for reading queues: too heavy in computation and traffic.

## Maintainer's decisions (2026-11-16)

1. **Isolation:** by default a connection and a Tor circuit per contact (section 7).
2. **Token:** the relay stores the SHA-256 of `recv_token`.
3. **Version 1:** not supported for a single day. Existing contacts exchange invites again (section 8).
4. **Return queue on another relay:** allowed. `QueueOffer` carries `relay_url`, and the client keeps connections to every relay that holds its contacts' queues. Then no single relay sees both sides of a pair.
5. **Automatic ban after reports:** removed. Protection rests on invites, deleting the queue and limits (section 6).

## Amendment of 2026-11-16 (maintainer): contact prekeys and incognito profiles

Planning the implementation found two gaps.

### 1. A new session with an existing contact

Sometimes a new Olm session is needed with a contact you already have: for example, after restoring from a backup without sessions. Today the key for it comes from `ClaimKey { target }`. Under section 5, prekeys live only on the invite queue, and the contact may no longer hold a working address of it.

The same fallback key cannot be published on every queue: the relay would link the queues by identical key bytes.

Decision:

- On each of its incoming contact queues the client publishes **2 one-time keys** (`PublishKeys { recv_id, recv_token, one_time, fallback: None }`). One-time keys are all different, so they cannot link queues.
- The fallback key is published only on the invite queue.
- The contact claims a key with `ClaimKey { send_id }`, using our queue's address, which it already knows.
- The relay sends `KeyCount` to whoever is subscribed to the queue. The client tops the keys up to 2.
- With no keys left, the relay answers `NoKeys`, and the contact tries later. This queue cannot fall back to the fallback key.
- vodozemac 0.11 keeps up to 5000 secret one-time keys and erases the oldest when full. A contact queue key published long ago may be gone. Then a session on it cannot be created. A client that receives a pre-key message it cannot decrypt publishes new keys on that queue.

### 2. Incognito profiles (ADR-014)

With queues, profiles no longer hide anything from the relay: it does not know any keys, and every contact gets its own connection (section 7). But profiles still hide contacts from each other: a profile has its own key, nickname and fingerprint.

Decision:

- Profiles stay as protection **from contacts**.
- A profile has no relay connection of its own. Its queues are served by the same per-contact connection as every other contact's.
- A profile's `Unregister` is replaced by `DeleteQueue` on its queues.
- The parts of ADR-014 about what the relay sees are replaced by this ADR. The part about contacts still holds.

## Amendment of 2026-11-16 (maintainer): requests without an invite code

ADR-013 allowed contact requests without a code until the owner turned off the "accept requests without an invite code" setting, so that old invites without a code kept working. With queues, that setting lost its meaning and became a hole.

- **Meaning.** Invites of versions 1 and 2 are refused (section 8). A version 3 invite always carries a code. A genuine request without a code cannot arrive.
- **Hole.** The invite queue's address stays the same when the owner renews the public invite: only the code changes. Someone holding the old public invite could send a `ContactRequest` without a code into the same queue and get past the renewal, if the setting were on.

Decision:

- A request without a code (`ContactRequest` instead of `ContactRequestV2`) from a stranger is always refused, silently, like any request with a wrong code.
- The setting is removed from every interface. Its place in settings goes to "one connection for all contacts" (decision 1).
- A contact we already have is not affected: their request from the invite queue joins the existing contact without a code check (section 8).
- So renewing the public invite is now enough: the old code opens nothing, and there is no way in without one. The invite queue itself stays the same.
