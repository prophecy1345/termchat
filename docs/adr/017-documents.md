# ADR-017: documents as opaque bytes inside Olm messages

Status: **accepted** (maintainer, 2026-11-01). Русская версия: [017-documents.ru.md](017-documents.ru.md).

## Context

termchat was text only (rule 9). People need to hand each other a document now
and then: a PDF, a config, a public key, a contract draft. Today they leave
termchat for that and use a channel that is usually less private.

The owner decided (2026-11-01) to allow **documents as opaque bytes**. termchat
does not render, preview or open them, and it does not show images inline.

Rule 1 forbids homemade crypto. vodozemac 0.11 has no attachment encryption.
In Matrix, attachments are handled by matrix-sdk-crypto (AES-CTR + SHA-256,
with the key sent over Olm/Megolm).

## Decision

### 1. Transport: chunks in ordinary Olm messages
A file is cut into chunks. Each chunk is one ordinary `Inner` message with a
new body variant, encrypted by the existing Olm session and padded to the
16384-byte bucket. There is no new primitive, no new dependency, and the relay
does not change.

```rust
Body::File {
    id: u32,              // random, one per transfer
    name: String,
    size: u32,            // whole file, bytes
    index: u16,           // chunk number, from 0
    expires_in: Option<u32>,
    data: Vec<u8>,
}
```

The variant is appended at the end of `Body` (postcard index 5), so existing
variants keep their encoding. Name and size go in every chunk, so no separate
"start" message is needed.

### 2. Limits (checked in `proto`, so they are fuzzed)
- `MAX_FILE_BYTES = 1_048_576` (1 MiB). `FILE_CHUNK = 16_000`, so at most 66 chunks.
- `0 < size <= MAX_FILE_BYTES`, and `index < ceil(size / FILE_CHUNK)`.
- `data.len()` is exactly `FILE_CHUNK`. Only the last chunk is shorter: it holds
  exactly the remainder.
- `name`: 1 to 255 bytes. It has no control or bidi characters, no `/`, no `\`
  and no NUL, and it is not `.` or `..`.

### 3. Integrity and order
- Olm authenticates every chunk.
- Gaps already show through `seq`.
- The receiver takes chunks only in order (`index == next`). A gap, a different
  `size` or `name` under the same `id`, or a new `id` mid-transfer cancels the
  transfer, with a system line in the chat.
- No hash is needed: the chunks are authenticated and their order is checked.

### 4. Sending pace
The relay allows 30 `Send` in a burst, then 1 every 2 s. A refused `Send`
would come after encryption and leave a hole in `seq`. So the client paces
itself:
- The first 10 chunks go at once, then one chunk every 3 s.
- The rest of the budget stays free for text.
- 200 KB takes about 10 s. 1 MiB takes about 3 min.
- Progress is shown in the chat.
- An unfinished transfer stays in the state file and resumes after a restart.

### 5. Who can send
- Only accepted contacts.
- Chunks that arrive while a contact request is pending, or from a stranger,
  are dropped. A stranger's queue quota is 256 KiB anyway.
- No sending to blocked contacts or to contacts who left.

### 6. Storage on the receiver
- An assembled file stays inside the encrypted state file until the user saves
  or discards it.
- After that only the history line stays: `file name (size)`.
- One incoming transfer per contact at a time.
- At most 8 MiB of unsaved files in total. A file past the limit is dropped,
  with a system line.
- The chat's disappearing-message timer applies to files too.
- Deleting the chat, WIPE and the duress password erase files with everything else.

### 7. Saving
- A file is saved only on an explicit user action.
- Terminal: into a folder the user picks (`~/Downloads` by default).
  - The name passes through `clean()`.
  - The file is created with `create_new`, mode `0600`, never executable. An
    existing file is never overwritten; ` (1)`, ` (2)` and so on are added.
- Android: through the system "Save as" dialog (SAF). No storage permission
  is needed.
- termchat never opens a file and never guesses its type.
- The save dialog warns: "files from contacts can be harmful; termchat does
  not open them".

## Consequences and honest limits
- **Metadata.** The relay sees a series of 16 KB envelopes from one key to
  another, so it can estimate the file size to within about 16 KB. Write this
  in threat-model.md, the README and the site.
- **A saved file lives outside termchat.** WIPE and the duress password do not
  erase it.
- **Older clients** (before 0.4.0) cannot parse the new variant. They show "a
  message could not be decrypted" once per chunk. Both sides need 0.4.0.
- **Rule 9 becomes:** "Text, and documents as opaque bytes (ADR-017). No
  rendering, previews, auto-opening, inline images or message formatting."

## Rejected
- **Encrypted blob on the relay, key over Olm** (Matrix style, or `age`):
  - it is fast and allows large files;
  - but it needs a new crypto format and dependency, blob storage and an API
    on the relay, and an exception to rule 14;
  - it can come back later in its own ADR, if 1 MiB proves too little.
- **A larger limit inside Olm:** the recipient's queue quota (10 MiB) and the
  send rate would make 4 MiB take about 10 minutes and fill half the queue.
- **Showing images:** decoders are a classic attack surface, and terminals
  cannot show them consistently.

## Dependencies
None new. `base64` is already in `engine`; it keeps unsaved bytes in the JSON
state.
