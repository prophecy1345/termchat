# ADR-043: documents up to 25 MiB, sent only when the receiver asks

Status: **accepted** (owner, 2026-11-24). Русская версия: [043-large-documents.ru.md](043-large-documents.ru.md). Extends [ADR-017](017-documents.md) (documents as opaque bytes) and uses [ADR-041](041-delivery-and-read-receipts.md) level gating. PLAN.md P11.

## Context

ADR-017 caps a document at 1 MiB (`MAX_FILE_BYTES`). People hit the cap with an ordinary scan, a photo or a slide deck, and then leave termchat for a less private channel.

ADR-017 rejected simply raising the cap inside Olm. Three things stand in the way:

- **Pace.** The relay takes 30 `Send` into one queue in a burst, then 1 every 2 s (`sends_per_queue`). One chunk is 16 000 bytes in the 16 KiB bucket. 25 MiB would be about 1 640 chunks, close to an hour.
- **Queue quota.** A queue holds 10 MiB (`queue_bytes`). A 25 MiB file pushed at someone who is offline fills the queue and blocks their text.
- **Unwanted bytes.** Today the sender pushes and the receiver gets every byte, wanted or not. At 1 MiB that is tolerable. At 25 MiB it costs the receiver's data plan and battery, and it is an easy way to annoy someone.

Rule 1 rules out the Matrix way (an encrypted blob on the relay with the key sent over Olm): vodozemac has no attachment encryption, and we may not combine primitives ourselves. So the bytes stay inside ordinary Olm messages, and the ADR changes how they are sent, not how they are encrypted.

## Decision

### 1. Two ways to send

- **Up to 1 MiB: push, as now** (ADR-017, unchanged).
- **Above 1 MiB, up to 25 MiB: offer, then send on request.** The sender sends a short offer. The bytes go only after the receiver presses "Download".

### 2. New `Body` variants

Appended at the end of `Body`, so existing variants keep their encoding:

```rust
Body::FileOffer {
    id: u32,                  // random, one per transfer
    name: String,             // same rules as ADR-017
    size: u32,                // 1 ..= MAX_LARGE_FILE_BYTES
    expires_in: Option<u32>,  // the chat's disappearing timer
}
Body::FileWant  { id: u32 }            // receiver: send it
Body::FileAck   { id: u32, next: u16 } // receiver: all chunks below `next` arrived
Body::FileStop  { id: u32 }            // either side: cancel, forget the transfer
Body::FilePart  { id: u32, index: u16, data: Vec<u8> }
```

Checked in `proto`, so they are fuzzed:

- `MAX_LARGE_FILE_BYTES = 26_214_400` (25 MiB).
- `LARGE_CHUNK = 64_000`, so at most 410 chunks. `index < ceil(size / LARGE_CHUNK)`.
- `data.len()` is exactly `LARGE_CHUNK`, except the last chunk, which holds exactly the remainder.
- A `FilePart` is valid only for an `id` the receiver asked for with `FileWant`, and only in order (`index == next`). Anything else is dropped, and a gap cancels the transfer with a system line, as in ADR-017 section 3.

### 3. A bigger bucket for parts

- `BUCKETS` gets a fourth size, **65 536**. Only `FilePart` uses it. Text and everything else keep the three existing sizes.
- `MAX_FRAME_BYTES` grows from 64 KiB to **72 KiB** to carry the 64 KiB envelope with its header. The relay takes the new limit from `proto`.

### 4. Pace and flow control

- The sender keeps at most **64 parts (4 MiB) unacknowledged**. The receiver sends `FileAck` every 16 parts. So a transfer never holds more than 4 MiB of the receiver's 10 MiB queue, and text always fits.
- **The relay's rate changes**: `sends_per_queue` goes from 1 every 2 s to **1 per second** (burst stays 30). The queue quota, not the rate, is what protects a queue from flooding, and the quota does not change.
- The sender sends one part every 1.5 s, leaving room for text, as ADR-017 does.
- 25 MiB takes about **10 minutes**. 4 MiB takes about 1.5 minutes. Progress shows on both sides.
- The transfer needs both devices to come online at some point, not at the same time: parts wait in the queue, acks wait in the sender's queue. A paused transfer resumes after a restart on either side.

### 5. Who can send and receive

- Only accepted contacts, as in ADR-017 section 5. An offer from a stranger, a pending request or a blocked contact is dropped.
- Offers go only to contacts at **`Understands` level 7**. For an older contact the app says "their app takes documents up to 1 MiB" and falls back to push for files that fit.
- **Groups stay at 1 MiB** in this ADR. A group shares one queue, so parts sent for one member reach all 32. Large files in groups need their own ADR.
- **Linked devices** (ADR-028) do not get a copy of a large file you sent; they get the history line only.

### 6. The sender keeps a copy until it is taken

- On "Send", the app reads the file once (the caller does the IO, rule 11) and keeps an encrypted copy in its spool (section 7). Later changes to the original do not matter.
- An offer lives **7 days**. After that, or after `FileStop`, the sender deletes its copy, and both chats show "not taken".
- At most **4 offers** waiting per sender at once, and at most **100 MiB** spooled in total.

### 7. Spool on disk: sealed records next to the state file

A 25 MiB file cannot live in the JSON state file: that file is rewritten whole on every change.

- Each transfer gets a spool file next to the state file, with a random name.
- Each part is stored as one record: `seal(file_key, id ‖ index ‖ data)`, the same `core::store::seal` (XChaCha20-Poly1305 with a fresh random nonce) that already protects the state file. No new primitive and no new key.
- When the file is assembled, each record is opened and checked: right `id`, `index` in order, no record missing or repeated. Only then is the file offered for saving.
- Limits on the receiver: one incoming large transfer per contact, at most **64 MiB** of unsaved files in total (was 8 MiB). Past the limit the app does not send `FileWant` and says why.
- **WIPE, the duress password and deleting the chat delete the spool files too.** The disappearing timer applies to them, as to everything else.
- The engine stays without IO: it hands the caller a record to write or a list of records to read.

### 8. Saving

Same as ADR-017 section 7: only on an explicit action, `create_new`, mode `0600`, never executable, Android through SAF. The file is written to its target from the spool record by record, without holding all 25 MiB in memory.

### 9. Protocol level

`UNDERSTANDS` becomes 7.

## Consequences and honest limits

- **Metadata.** The relay sees the 64 KiB bucket and knows a large file is moving between two queues. It can estimate the size to within about 64 KB and sees when the receiver asked for it. threat-model.md, the README and the site will say so.
- **Speed.** 10 minutes for 25 MiB is slow next to Signal or Telegram. It is the price of having no file storage on the relay.
- **Relay.** Older relays reject 72 KiB frames. The app then cancels the transfer with "this relay does not take large documents". Our relay is redeployed with the release (ADR-010).
- **Disk.** The receiver's spool can hold up to 64 MiB, encrypted, until the files are saved or discarded.
- **A saved file lives outside termchat**, as in ADR-017.

## Rejected

- **An encrypted blob on the relay** (Matrix style, `age`): it needs attachment encryption that vodozemac lacks (rule 1), blob storage and an API on the relay, and an exception to rule 14. It is still the way to go above 25 MiB, in its own ADR.
- **Pushing large files as now**: it fills the receiver's queue and costs them data they may not want.
- **Keeping parts in the JSON state file**: the file would be rewritten whole, up to 25 MiB, after every part.
- **Large files in groups**: one shared queue; it needs a separate design.

## Dependencies

None new.

## Implementation notes (2026-11-24)

Where the code differs from the text above:

- **Spool IO lives in `engine`**, which already reads and writes the state file. `core` and `proto` stay without IO (rule 11). The records are sealed by `core::store::seal_record`: the state file's data key and AEAD, with the transfer id and part number as associated data. The state file's header is not part of the record, so a new password keeps the spool readable.
- **Saving assembles the file in memory**, 25 MiB at most, from the checked records. Android gets the bytes through FFI, as for small files.
- **Offers and parts go to the contact's primary device only**, and our own linked devices get nothing, not even the history line: a `FileOffer` is never put in a `Copy`.
- **`termchat send-file` stays at 1 MiB**: a one-shot command cannot wait for the contact to ask. `termchat recv` does not show offers.
- The spool folder is `<state file>.spool` next to the state file. Files no message needs are shredded on the next tick, so clearing or deleting a chat frees them too. WIPE shreds the folder.
