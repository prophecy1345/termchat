# ADR-002: vodozemac integration

Status: accepted. Date: 2026-10-15. Русская версия: [002-vodozemac-integration.ru.md](002-vodozemac-integration.ru.md).

## Context

vodozemac provides `Account` (identity, one-time and fallback keys) and
`Session` (Olm). We need to decide how keys are published and rotated and how
state is stored.

## Decisions

### Session version: `SessionConfig::version_1`
- `version_2` (untruncated MAC) exists only behind the
  `experimental-session-config` feature. Experimental features stay out of the
  crypto core.
- `version_1` truncates the MAC to 8 bytes. All of Matrix works this way, and it
  is the variant that was audited.
- Both sides use `version_1` only. Moving to `version_2` is a separate ADR once
  the feature is stable.

### One-time keys
- On first run and on every connection the client keeps a full stock on the
  relay: `Account::max_number_of_one_time_keys()` (50 in vodozemac 0.11).
- The relay reports what is left (`KeyCount`). Below half of the stock, the
  client generates new keys, publishes them and calls `mark_keys_as_published`.
- Every key is signed with Ed25519 (`Account::sign`); the signature format is in
  `docs/protocol.md`, section 4.

### Fallback key
- One active fallback key on the relay.
- When a pre-key message built on the fallback key arrives, the client generates
  a new one (`generate_fallback_key`) and publishes it.
- The old fallback key is forgotten (`forget_fallback_key`) **relay queue TTL + 1
  day** after the rotation (15 days by default): messages sent to it while the
  owner was offline can wait in the queue for the whole TTL. The delay is derived
  from the TTL setting, not a separate constant.

### One session per contact pair
- Each contact keeps a list of Olm sessions; the active one is the last that
  worked.
- A normal message is tried against that contact's sessions in turn.
- A pre-key message: first look for an existing session with the same
  `session_id()`, otherwise create an inbound session (`create_inbound_session`).
- Up to 5 old sessions are kept per contact.

### Storing state
- `Account::pickle()` and `Session::pickle()` are serialized together with the
  contacts into one file with `serde_json`: vodozemac pickles use internally
  tagged enums, which postcard does not support. `serde_json` is already in the
  tree through vodozemac.
- The whole file is encrypted with `XChaCha20Poly1305` under a random data key
  (DEK). The DEK sits in the header, encrypted with a key derived from the
  password (Argon2id). The header always has two slots: for the main and the
  duress password (see ADR-003).
- The libolm pickle format (`to_libolm_pickle`) is not used: it needs a unique key
  per pickle, which the outer file encryption already provides.
- The core only returns encrypted bytes; the client writes them to disk (the
  "core without IO" rule).
- Writes are atomic: temporary file, `fsync`, `rename`.
- State must be saved **after every** encryption and decryption: the ratchet
  moves, and rolling back breaks the session or allows a replay.

### Argon2id parameters
- 64 MiB of memory, 3 iterations, 1 lane (the RFC 9106 recommendation for
  memory-constrained systems).
- The parameters are stored in the file header, the same for both slots
  (ADR-003). They can be raised later without a format change.
- Because of the two slots, unlocking runs Argon2id twice. Expected about one
  second on a Raspberry Pi 4.

## Open questions
- Saving the file after every message may get slow with a long history. Then
  history and key state would move to separate files.
