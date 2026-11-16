# termchat.app protocol, version 1

Status: accepted (M0, 2026-10-15); version 2, anonymous queues, accepted 2026-11-16 (ADR-023). Format changes increase `version`.
Русская версия: [protocol.ru.md](protocol.ru.md).

## 1. Layers

1. **Transport:** WebSocket over TLS (`wss://`). A reverse proxy terminates
   TLS; the relay sees plain WS. Every message is one binary WS frame. Text
   frames are a protocol error and close the connection.
2. **Frames:** the `ClientFrame` and `ServerFrame` enums, encoded with
   `postcard`. Used for queues, keys, sending and delivery.
3. **Envelope:** what the relay forwards from client to client. Its body is an
   Olm message.
4. **Inner message:** the plaintext inside Olm, padded.

All integers use postcard encoding (varint). Ed25519 and Curve25519 keys are 32
bytes, Ed25519 signatures 64 bytes.

Only the canonical encoding is accepted: postcard allows over-long varints, so
the decoder re-encodes the value and compares it with the input (found by
fuzzing in M1).

**Identity** is a self-certifying card:

```
Identity { ed25519: [u8; 32], curve25519: [u8; 32], signature: [u8; 64] }
```

`signature` is an Ed25519 signature over `"termchat-id-v1" || curve25519`. It
binds the exchange key to the signing key. Without it, swapping `curve25519` in
an invite would not be caught by comparing fingerprints (the fingerprint covers
only Ed25519), and whoever swapped it could read messages. A client accepts a
contact only after checking this signature.

## 2. Version

The client's first frame on every connection is `Hello { version: u16 }`;
currently `version = 2` (ADR-023). The relay sends nothing back if it accepts
it. Otherwise it answers `Error { code: UnsupportedVersion }` and closes the
connection. `Hello` stays variant 0 and `Error` variant 7 of their enums, as in
version 1, so a version 1 client can still read that refusal. The envelope
carries `version` too.

Version 1 had accounts: a signed login, and envelopes addressed to identity
keys. It is not supported; a relay of version 2 erases its data on upgrade.

The relay's HTTP upgrade response carries `x-termchat-version: X.Y.Z`, the
relay's software version. Clients use it only to say that a newer termchat.app is
out (ADR-010); it plays no part in the protocol.

## 3. Queues (ADR-023)

The relay knows no users. It knows queues:

```
C -> R  CreateQueue
R -> C  QueueCreated { recv_id: [u8; 16], send_id: [u8; 16], token: [u8; 32] }
C -> R  Subscribe { recv_id, token }
R -> C  Subscribed { recv_id }
C -> R  DeleteQueue { recv_id, token }
R -> C  QueueDeleted { recv_id }
```

- The relay picks all three values at random. It stores `recv_id`, `send_id`
  and the SHA-256 of `token`, and compares hashes in constant time.
- `recv_id` and `token` stay with the queue's owner: they read and delete it.
  `send_id` is what others write to; it goes to them only inside Olm
  (`QueueOffer`, section 7) or in an invite (section 9).
- A wrong token and an unknown queue get the same answer, `UnknownQueue`.
- After `Subscribed` the relay sends the queue's stored envelopes, then
  `KeyCount`, then new envelopes as they come. A newer subscription to the
  same queue takes it over.
- Every identity has an invite queue, where strangers write. Every contact
  has a queue of their own, which only that contact writes to. Blocking a
  contact deletes their queue.
- A client keeps one connection per queue it reads, so the relay cannot link
  queues by connection (ADR-023, section 7); with Tor, each on its own circuit.
- Queues unused for 90 days are deleted.

## 4. Keys (prekeys)

```
C -> R  PublishKeys { recv_id, token, one_time: Vec<SignedKey>, fallback: Option<SignedKey> }
C -> R  ClaimKey { send_id }
R -> C  ClaimedKey { send_id, key: SignedKey } | Error { code: NoKeys }
R -> C  KeyCount { recv_id, one_time_left: u32, fallback_used: bool }
```

- An invite queue carries the identity's one-time keys and its fallback key.
  A contact's queue carries two one-time keys and no fallback key, so a contact
  can start a new session (after a restore from backup) without the relay
  linking queues by a shared key (ADR-023, amendment).
- `KeyCount` goes to the queue's subscriber after `Subscribed`, in reply to
  `PublishKeys`, and when someone claims one of its keys.
- `fallback_used = true`: the relay handed out the fallback key; the client
  rotates it (ADR-002). The client also rotates a fallback key older than 7
  days on its own clock, whatever the relay reports (ADR-019).
- An error on `ClaimKey` comes with `client_id: None`; the client claims keys one
  at a time.
- Past the limit on `ClaimKey` towards one queue, the relay hands out only the
  fallback key and spends no one-time keys.

`SignedKey { key: [u8; 32], signature: [u8; 64] }`: the owner's Ed25519 signature
over `"termchat-otk-v1" || key`. No separate id is needed: vodozemac finds a
one-time key by its public part. The client receiving a key checks the signature
against the identity it knows from the invite, so the relay cannot slip in a key
of its own.

The relay hands out each one-time key once and deletes it. When they run out it
hands out the fallback key, if the queue has one. How many to publish and when
to top up: ADR-002.

## 5. Sending and delivery

```
C -> R  Send { client_id: u64, send_id, envelope: Envelope }
R -> C  Accepted { client_id } | Error { client_id, code }
R -> C  Deliver { recv_id, relay_id: u64, envelope: Envelope }
C -> R  Ack { recv_id, relay_id }
C -> R  Report { recv_id, token, text: String }  // ADR-006, text up to 16000 bytes
R -> C  Reported                                 // the same answer on every path
C -> R  SetPush { recv_id, token, endpoint: Option<String> }  // ADR-012
R -> C  PushSet                                  // or Error { Malformed } for a bad URL
```

- `Send` needs no login: knowing `send_id` is the right to write.
- The relay keeps an envelope in the queue until its subscriber sends `Ack`.
  Unacknowledged envelopes are delivered again on every subscription. The
  queue lives in SQLite with a TTL (ADR-005). Only a subscriber of the queue
  can acknowledge.
- `Accepted` means the relay took the envelope; the client shows it as "on the
  relay". There are no end-to-end read receipts.
- `Deliver` names no sender: the relay does not know one. The client knows the
  contact by the queue, and confirms it by decrypting and checking the inner
  message (section 7). From an invite queue it reads only pre-key messages.
- `Report` sends the disclosed text to the operator's journal. The relay cannot
  ban anyone: it knows no keys (ADR-023, section 6).
- `SetPush` stores a UnifiedPush endpoint with the queue (ADR-012), `None`
  forgets it: an `https` URL on port 443, up to 512 bytes. When an envelope is
  queued while nobody reads the queue, or the reader's link sleeps, the relay
  POSTs an empty body to the endpoint, at most once every 30 seconds, only to
  public addresses. Every queue has its own endpoint, or the relay could link
  them by it.
- A wipe sends goodbyes, then deletes every queue (ADR-009).

## 6. Envelope

```
Envelope {
    version: u16,
    olm_type: u8,             // 0 = pre-key, 1 = normal
    body: Vec<u8>,            // Olm ciphertext
}
```

Neither sender nor recipient: the relay addresses the queue, and both names are
inside the ciphertext (section 7).

## 7. Inner message

The plaintext before Olm encryption:

```
Inner {
    sender: [u8; 32],         // sender's ed25519
    recipient: [u8; 32],      // recipient's ed25519
    seq: u64,                 // message number in the sender -> recipient direction, from 1
    body: Body,
}

Body =
    ContactRequest { identity: Identity, nickname: String, note: String }  // first message to a stranger
  | ContactAccept { nickname: String }
  | Text { sent_at: u64, expires_in: Option<u32>, text: String }
  | Goodbye                                                                // ADR-009: the sender wiped their account
  | ContactRequestV2 { identity: Identity, nickname: String, note: String, code: [u8; 16] }  // ADR-013
  | File { id: u32, name: String, size: u32, index: u16, expires_in: Option<u32>, data: Vec<u8> }  // ADR-017
  | QueueOffer { relay_url: String, onion_url: Option<String>, send_id: [u8; 16] }  // ADR-023: write to me here
```

Limits: `text` up to 16000 bytes, `nickname` up to 64 bytes, `note` up to 256
bytes.

`File` is one chunk of a document of `size` bytes, at most 1 MiB (ADR-017).
Chunk `index` holds bytes from `index * 16000`: exactly 16000 of them, or the
rest in the last chunk. `name` is 1-255 bytes with no control or bidi
characters, no `/`, `\` or NUL, and is not `.` or `..`. The receiver takes
chunks of one file strictly in order, only from accepted contacts, and drops
the transfer on a gap.

`QueueOffer` gives the contact the `send_id` of the queue we made for them; the
URLs follow the invite's limits and must be `wss://` (or `ws://` for an onion
address). From then on they write there. Meeting goes in this order:

1. The one asking writes to the invite queue: `ContactRequestV2`, then a
   `QueueOffer` with their queue.
2. The one accepting writes to that queue: first their own `QueueOffer`, then
   `ContactAccept`. So the asker has the address before it may send an
   ordinary message.

A contact we already have asking again (a new device, or the move from
version 1) joins that contact: name, history and fingerprint stay.

Checks on receipt:
1. `recipient` equals our own key.
2. `sender` equals the contact whose queue the envelope came in, and the key the
   Olm session belongs to. For a pre-key message from a contact: the
   Curve25519 key in the message equals `curve25519` in their verified
   `Identity`. In the invite queue the sender is the contact holding that
   Curve25519 key; a stranger there cannot name a contact we have.
3. A pre-key message from a stranger is accepted only if it is a
   `ContactRequest` or `ContactRequestV2` with a live invite code (section 9),
   its `identity.ed25519` equals `sender`, its
   `identity.curve25519` equals the Curve25519 key in the message, and the
   `identity` signature is valid. Such a sender goes to "requests".
4. Anything else is dropped.
5. A pre-key message never builds the same session twice (ADR-019). One whose
   session id belongs to a session the contact already has, and which that
   session cannot decrypt, is dropped. So is one whose session id is in the
   list of inbound sessions this identity has built. The list holds at most
   200 ids per sender and 10 000 in all, and an id goes when the fallback key
   it could have used is forgotten. Without this, a pre-key message sent to a
   fallback key, which stays in the account, could be delivered and read again.

The nickname is the only thing a contact learns about a person. It travels only
here, inside the ciphertext: up to 64 bytes of UTF-8, not empty, no control
characters (otherwise a contact could send an escape sequence to someone's
terminal). The recipient keeps it in their encrypted file. The protocol has no
other fields about people.

Numbering (`seq`):
- Each Olm session and direction has its own counter, starting at 1. A new
  session (including after a restore from backup) starts at 1.
- The sender increments `seq` by 1 for every message and keeps the counter in the
  session state. Resending the same message reuses its `seq`.
- The recipient keeps, per session, the last `seq` and the set of missing numbers.
  - `seq` greater than the last: accept. Numbers in between go into the missing
    set and the client shows "N messages missing".
  - `seq` in the missing set: accept (it arrived out of order), remove it, update
    the warning.
  - otherwise: a duplicate, drop it.
- `seq` is inside the ciphertext; the relay cannot see or forge it.
- A gap that does not close within the relay's queue TTL counts as lost. The
  missing set holds at most 1000 numbers; beyond that they only add to the lost
  count. Olm (vodozemac 0.11) keeps at most 40 skipped message keys per chain and
  tolerates a gap of up to 2000: a missing message older than that can no longer
  be decrypted and stays lost.
- Limitation: if the relay drops the **last** messages and no new ones follow,
  the gap is not visible. The contact's next message reveals it.

Messages from keys that are not contacts appear only under "requests". The
client classifies them; the relay does not know anyone's contacts. Until a
request is accepted (ADR-007): its `Text` messages are not shown, the client
keeps at most 5 and shows them on acceptance; only a new `ContactRequest` can
update the nickname, and after acceptance the nickname no longer changes from
the network; at most 50 requests wait at a time; the client remembers rejected
keys and does not decrypt their messages.

## 8. Padding

The serialized `Inner` is padded up to the nearest bucket: `0x80`, then zeros
(ISO/IEC 7816-4). Buckets: **512, 4096 and 16384 bytes**. Larger than 16384 is a
`TooLarge` error.

- The `text` limit is **16000 bytes of UTF-8** (about 8000 Cyrillic or 16000
  Latin characters). The limit is in bytes, not characters, so the size is
  predictable in any alphabet.
- Service messages (`ContactRequest`, `ContactAccept`) are padded the same way
  and look like short texts.
- Few buckets leak less about length. A single fixed size was not chosen: it
  would make the relay's queue about 30 times larger.

## 9. Invite string

```
termchat:<base64url(postcard(Invite))>

Invite { version: 3, relay_url: String, onion_url: Option<String>, identity: Identity, queue: [u8; 16], code: [u8; 16] }
```

`queue` is the `send_id` of the owner's invite queue (ADR-023). Invites of
versions 1 and 2 named only the identity and are refused with
`UnsupportedVersion`; the client asks for a new one. The relay may be another
one than the owner's own.

`code` is random. A request echoes it in `ContactRequestV2`. The owner's client admits a stranger only with a live code:
a one-time code is spent on arrival, and the public code works until it is
renewed. Everything else is dropped silently. The relay never sees the code
(ADR-013).

The fallback key is not part of the invite: it rotates and would go stale. The
one-time key is claimed with `ClaimKey { send_id: queue }`; its signature
prevents substitution.

There is no short form for typing by hand: the string is copied, and a short
code would need the relay to store a "code → keys" mapping. Later: a QR code in
the terminal.

## 10. Fingerprint

Same scheme as Signal: every key has its own fingerprint; the safety number is
both fingerprints in a row.

- **Key fingerprint**, 30 digits: `h = SHA-512("termchat-fp-v1" || ed25519)`. The
  first 30 bytes of `h` are split into 6 chunks of 5 bytes. Each chunk is read as
  a big-endian number modulo 100000, giving a group of 5 digits.
- **Safety number** of a pair, 60 digits: the fingerprints of both keys in key
  sort order, 12 groups of 5 digits. The same on both sides.

Why not one hash over the pair of keys: in a MITM the attacker picks both of
their own keys and can search for matching numbers with a birthday attack
(~2^(n/2) tries). With separate fingerprints they need a key with the same
fingerprint as one specific person: ~2^99 tries.

## 11. Limits (relay defaults, configurable)

| What | Value |
|---|---|
| WS frame size | 64 KiB |
| Envelope `body` | 16384 plus Olm overhead, at most 20 KiB |
| `Send` into one queue, from everyone | token bucket: 30 in a burst, then 1 every 2 seconds |
| Connections per IP | 50 (CGNAT: many people behind one IP) |
| Any frame, per connection | token bucket: 2000 in a burst (a whole queue acknowledged after a reconnect), then 20 per second; past it the connection is closed |
| New queues per IP | 20 per hour (over onion: a shared budget of 200 per hour, ADR-004) |
| One-time keys per queue | an invite queue gets 50 (`Account::max_number_of_one_time_keys()`), a contact's queue 2; the relay stores at most the 100 newest |
| `PublishKeys` per queue | 30 per hour |
| `ClaimKey` per connection | 30 per hour |
| `ClaimKey` towards one queue, from everyone | 100 per day (past that, only the fallback key) |
| Time to send `Hello` | 10 seconds after connecting |
| Writing to a client | 30 seconds: a client that does not read is disconnected |
| Queue | 1000 envelopes and 10 MiB, TTL 14 days; unused queues go after 90 days |
| `Report` per connection | 10 per day |
| Full channel to an online client | the relay drops the connection; the client reconnects and gets the queue (ADR-007) |

Counters live in memory only. Strangers reach only an invite queue, so there is
no stranger share of a contact's queue any more (ADR-023, section 6).

Connections over Tor arrive from shared exit IPs, so per-IP limits would hit
them unfairly. For them the relay publishes an onion address: onion connections
have no per-IP limits, only per-queue and per-connection limits and the onion
port's overall budget. There is no proof-of-work of our own: Tor provides it
for onion services (ADR-004).

## 12. Error codes

`UnsupportedVersion`, `Malformed`, `TooLarge`, `RateLimited`, `NoKeys`,
`UnknownQueue`, `QueueFull`, `Internal`.

`UnknownQueue` covers a wrong token too. An error does not close the connection,
except `Malformed`.

## 13. Keepalive and reconnect

- The relay pings every 30 seconds. No pong within 60 seconds closes the
  connection.
- The client reconnects with exponential backoff and jitter, from 1 to 60
  seconds.
- The client resends unacknowledged `Send` frames after reconnecting. The
  recipient drops duplicates by `seq`.
