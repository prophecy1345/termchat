# ADR-020: retrying a send the relay refused for now

Status: **accepted** (maintainer, 2026-11-11). Русская версия: [020-send-retry.ru.md](020-send-retry.ru.md).

## Context

`Client::encrypt` gives every message the next `seq` (protocol.md, section 7)
and advances the Olm ratchet before the envelope leaves the device. If the
relay then refuses the envelope, that `seq` is spent and never arrives. The
recipient's tracker reports "⚠ 1 message(s) missing so far" for the rest of the
session.

Today a refused `Send` ends like this:

- **Text:** the message turns `Failed`. Sending it again encrypts a new
  envelope with a new `seq`, so the old one stays missing forever.
- **File chunk:** a chunk other than the last one has a `client_id` that no
  message carries. `set_status` finds nothing and the refusal is lost without
  a trace. The sender sees the file as sent, and the recipient gets
  "did not arrive whole", because chunks must arrive in order
  (`Data::take_chunk`).

Some refusals are temporary, and the same bytes would be accepted later:

| Code | Meaning | Temporary |
|---|---|---|
| `RateLimited` | the sender's `sends_per_key` or new-recipient bucket is empty | yes |
| `QueueFull` | the recipient's queue, or the relay's total, is full | yes |
| `Internal` | database error on the relay | yes |
| `UnknownRecipient`, `Banned` | the recipient is gone or banned | no |
| `TooLarge`, `Malformed` | the envelope itself is wrong | no |

A file is the easy way to empty the send bucket: ADR-017 sends a burst of 10
chunks, then one every 3 s, next to ordinary texts.

## Decision

### 1. A temporary refusal is retried with the same envelope
When the relay answers `Error { client_id: Some(id), code }` with a temporary
code, the relay link (`crates/engine/src/relay.rs`) keeps that `Send` frame in
`pending` and sends **the same frame** again later. It does not tell the engine,
so the message stays `Sending`.

- Wait 2 s, then double up to 60 s, with the same jitter as reconnects.
- No limit on attempts while the link lives. The relay keeps a queue for
  14 days. A message that never gets through stays `Sending` and is not
  marked `Failed` by mistake.
- A reconnect sends every pending frame again, as it does today (protocol.md,
  section 13).

The envelope is never encrypted again, so the `seq` and the ratchet step stay
the same, and nothing goes missing.

### 2. Order to one recipient is kept
Frames to a recipient whose envelope waits for a retry are held in the link,
in `client_id` order, until that envelope is `Accepted`. Frames to other
recipients go on.

Two reasons:

- File chunks are only accepted in order (`take_chunk`).
- vodozemac keeps at most 40 skipped message keys per receiving chain
  (`MAX_MESSAGE_KEYS`). An envelope overtaken by more than 40 later ones on the
  same chain can no longer be decrypted.

### 3. A permanent refusal fails the whole message
`UnknownRecipient`, `Banned`, `TooLarge` and `Malformed` mark the message
`Failed`, as now. For a file chunk, the engine remembers which file each chunk's
`client_id` belongs to. A refused chunk fails that file and stops its
remaining chunks. Today the refusal disappears.

## Not a new protocol (rule 1)

Nothing changes in the wire format, the relay or the cryptography. The client
sends the same bytes again after a refusal, as it already does after a
reconnect.

## Metadata, honestly

The relay sees each retry as one more `Send` of the same bytes from the same
key, a few seconds or minutes later. It already learns this from reconnect
resends. No new information leaves the device.

## What remains

- `pending` lives in memory. A send that is still unconfirmed when the app
  exits (for example when Android kills it in the background) becomes
  `Failed: not confirmed before exit` and leaves a gap, as today. Keeping
  `pending` on disk would close this. It is a separate decision, because the
  state file would then hold outgoing ciphertext.
- The relay may still drop a queued envelope (14-day expiry, a ban, a
  dishonest relay). The recipient sees the gap. That warning is the point of
  `seq`.
- Retrying does not help against a full queue that never empties. The message
  stays `Sending`, which is accurate.
