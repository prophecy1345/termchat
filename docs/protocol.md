# termchat protocol, version 1

Status: accepted (M0, 2026-10-15). Format changes increase `version`.
Русская версия: [protocol.ru.md](protocol.ru.md).

## 1. Layers

1. **Transport:** WebSocket over TLS (`wss://`). A reverse proxy terminates
   TLS; the relay sees plain WS. Every message is one binary WS frame. Text
   frames are a protocol error and close the connection.
2. **Frames:** the `ClientFrame` and `ServerFrame` enums, encoded with
   `postcard`. Used for registration, keys, sending and delivery.
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

The client's first frame is `Hello { version: u16 }`; currently `version = 1`.
If the relay does not support it, it answers `Error { code: UnsupportedVersion }`
and closes the connection. The envelope carries `version` too, so clients of
different versions can understand each other or refuse explicitly.

## 3. Registration (challenge-response)

```
C -> R  Hello { version }
R -> C  Challenge { nonce: [u8; 32] }
C -> R  Auth { identity: Identity, signature }
R -> C  AuthOk | Error { code }
```

`signature` is an Ed25519 signature over:

```
"termchat-auth-v1" || relay_host || nonce
```

- `relay_host`, from the relay URL (for example `relay.example.org` or
  `xxxx.onion`), stops a signature from being reused on another relay. The relay
  accepts any of its addresses (`--host` can be given several times).
- The signature inside `identity` binds the exchange key to the signing key; the
  relay checks it.
- The nonce is single-use and tied to the connection; the whole exchange up to
  `Auth` has 10 seconds (`auth_timeout`).

Before `AuthOk` the relay accepts only `Hello` and `Auth`.

## 4. Keys (prekeys)

```
C -> R  PublishKeys { one_time: Vec<SignedKey>, fallback: Option<SignedKey> }
C -> R  ClaimKey { target: ed25519 }
R -> C  ClaimedKey { target, key: SignedKey } | Error { code: NoKeys }
R -> C  KeyCount { one_time_left: u32, fallback_used: bool }
```

- `KeyCount` arrives right after `AuthOk`, in reply to `PublishKeys`, and to the
  key owner (if online) when someone claims one of their keys.
- `fallback_used = true`: the relay handed out the fallback key; the client
  rotates it (ADR-002).
- An error on `ClaimKey` comes with `client_id: None`; the client claims keys one
  at a time.
- Past the limit on `ClaimKey` towards one target, the relay hands out only the
  fallback key and spends no one-time keys.

`SignedKey { key: [u8; 32], signature: [u8; 64] }`: the owner's Ed25519 signature
over `"termchat-otk-v1" || key`. No separate id is needed: vodozemac finds a
one-time key by its public part. The client receiving a key checks the signature
against `target`, so the relay cannot slip in a key of its own.

The relay hands out each one-time key once and deletes it. When they run out it
hands out the fallback key. How many to publish and when to top up: ADR-002.

## 5. Sending and delivery

```
C -> R  Send { client_id: u64, envelope: Envelope }
R -> C  Accepted { client_id } | Error { client_id, code }
R -> C  Deliver { relay_id: u64, sender: ed25519, envelope: Envelope }
C -> R  Ack { relay_id }
C -> R  Report { sender: ed25519, text: String }   // ADR-006, text up to 16000 bytes
R -> C  Reported                                  // the same answer on every path
C -> R  Unregister                                // ADR-009: forget my key, prekeys and queue
R -> C  Unregistered                              // then the relay closes the connection
```

- The relay keeps an envelope in the recipient's queue until the recipient sends
  `Ack`. Unacknowledged envelopes are delivered again on every connection. The
  queue lives in SQLite with a TTL (ADR-005).
- `Accepted` means the relay took the envelope; the client shows it as "on the
  relay". There are no end-to-end read receipts.
- The relay fills in `sender` in `Deliver` from the authenticated connection. The
  client does not trust this field: the real sender is confirmed by decrypting
  and checking the inner message (section 7).
- `Ack` lets the relay delete the envelope.
- `Unregister` is sent by a wipe after the goodbyes (ADR-009). The relay deletes
  the account row with its prekeys and incoming queue (bans stay), answers
  `Unregistered` and closes the connection. Afterwards `ClaimKey` and `Send` to
  that key answer `UnknownRecipient`.

## 6. Envelope

```
Envelope {
    version: u16,
    recipient: [u8; 32],      // recipient's ed25519
    olm_type: u8,             // 0 = pre-key, 1 = normal
    body: Vec<u8>,            // Olm ciphertext
}
```

There is no sender field, so sealed sender can be added later without changing
the format.

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
```

Limits: `text` up to 16000 bytes, `nickname` up to 64 bytes, `note` up to 256
bytes.

Checks on receipt:
1. `recipient` equals our own key.
2. `sender` equals the key the Olm session belongs to. For a pre-key message from
   a contact: the Curve25519 key in the message equals `curve25519` in their
   verified `Identity`.
3. A pre-key message from a stranger is accepted only if it is a
   `ContactRequest`, its `identity.ed25519` equals `sender`, its
   `identity.curve25519` equals the Curve25519 key in the message, and the
   `identity` signature is valid. Such a sender goes to "requests".
4. Anything else is dropped.

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

Invite { version: u16, relay_url: String, onion_url: Option<String>, identity: Identity }
```

The fallback key is not part of the invite: it rotates and would go stale. The
one-time key is claimed with `ClaimKey`; its signature prevents substitution.

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
| `Send` per key | token bucket: 30 in a burst, then 1 every 2 seconds |
| New recipients per key | 20 per day |
| Connections per IP | 50 (CGNAT: many people behind one IP) |
| Any frame after `Auth`, per key | token bucket: 2000 in a burst (a whole queue acknowledged after a reconnect), then 20 per second; past it the connection is closed |
| New registrations per IP | 20 per hour (over onion: a shared budget of 200 per hour, ADR-004) |
| One-time keys per key | the client keeps 50 on the relay (`Account::max_number_of_one_time_keys()`), the relay stores at most the 100 newest |
| `PublishKeys` per key | 30 per hour |
| `ClaimKey` per key | 30 per hour |
| `ClaimKey` towards one target, from everyone | 100 per day (past that, only the fallback key) |
| Time to authenticate | 10 seconds after connecting |
| Writing to a client | 30 seconds: a client that does not read is disconnected |
| Queue per recipient | 1000 envelopes and 10 MiB, TTL 14 days |
| A stranger's share of a queue (ADR-007) | 20 envelopes and 256 KiB from a sender the recipient never wrote to |
| Full channel to an online client | the relay drops the connection; the client reconnects and gets the queue (ADR-007) |

Counters live in memory only.

Connections over Tor arrive from shared exit IPs, so per-IP limits would hit
them unfairly. For them the relay publishes an onion address: onion connections
have no per-IP limits, only per-key limits and the onion port's overall budget.
There is no proof-of-work of our own: Tor provides it for onion services
(ADR-004).

## 12. Error codes

`UnsupportedVersion`, `Malformed`, `TooLarge`, `NotAuthenticated`,
`BadSignature`, `ChallengeExpired`, `RateLimited`, `NoKeys`,
`UnknownRecipient`, `QueueFull`, `Banned`, `Internal`.

An error with a specific `client_id` does not close the connection.
`Malformed`, `BadSignature` and `NotAuthenticated` do.

## 13. Keepalive and reconnect

- The relay pings every 30 seconds. No pong within 60 seconds closes the
  connection.
- The client reconnects with exponential backoff and jitter, from 1 to 60
  seconds.
- The client resends unacknowledged `Send` frames after re-authenticating. The
  recipient drops duplicates by `seq`.
