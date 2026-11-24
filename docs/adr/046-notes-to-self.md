# ADR-046: notes to self, kept on the device and shared with one's own devices

Status: **accepted** (owner, 2026-11-24). Русская версия: [046-notes-to-self.ru.md](046-notes-to-self.ru.md). PLAN.md Q1. Builds on [ADR-028](028-second-device.md) (linked devices) and [ADR-030](030-keys-by-someones-word.md) (copies to one's own devices).

## Context

People keep a chat with themselves: a link to read later, an address, a draft, a file to move from the phone to the computer. In Telegram "Saved Messages" is one of the most used features. In termchat.app today the only way is a contact who does not exist.

What it must not become:

- **No server storage.** The relay keeps ciphertext for delivery only and deletes it once delivered (ADR-005, rule 14). Notes are not "stored in the cloud".
- **No new crypto** (rule 1). Notes travel, if at all, as ordinary Olm messages between devices that already know each other.

## Decision

### 1. A chat with yourself

- Every identity's main profile has one **Notes** chat. It is always there, first in the list, with its own pixel icon. Incognito identities (ADR-014) have none: they exist for one contact.
- It works like a chat: text, replies, edits, deletes, documents up to 1 MiB, the disappearing timer, search, pin, export, forward into it and out of it. No receipts, no reactions from anyone else, no typing anything to anyone.
- **On one device nothing leaves it.** Notes are written into the encrypted state file, like any chat history, and never go on the wire.

### 2. Shared with your own devices

With linked devices (ADR-028), each note goes to every other device of the same identity:

```rust
Body::Note { body: Box<Body> } // appended at the end of `Body`
```

- `body` is one of `Message`, `Edit`, `Delete` or `File` (a chunk of a document up to 1 MiB), checked in `proto` and fuzzed. Nothing else, and never a `Note` inside a `Note`.
- It is sent over Olm to each of our devices directly, the way devices already talk to each other (the `own` contacts of ADR-028), only to devices at `Understands` level **9**. Never to a contact, never in a group, never in a `Copy`.
- A device files a `Note` only if it comes from one of **its own** devices. From anyone else it is dropped: a contact cannot write into your notes.
- Ids are the `Message` ids of ADR-025, so an edit or delete made on the phone finds the same note on the computer.

### 3. What a device sees

- A device sees notes written **after** it was linked, on any device. Notes written before stay where they are, as with chat history today (ADR-028): there is no bulk copy.
- A device offline for more than 14 days misses what was sent meanwhile: the relay keeps an undelivered message that long (ADR-005).
- Clearing the Notes history, WIPE and the duress password work as for any chat. Clearing is per device, as for chats.

### 4. Interface

- **Desktop and Android:** "Notes" at the top of the chat list; **Forward** lists it first.
- **Terminal:** "notes" as the first row; Ctrl+K finds it by name.
- The header says "only on your devices".

## Consequences and honest limits

- **Metadata.** With one device: none. With linked devices the relay sees messages between their queues, as it already does for copies of what you send to contacts; it cannot tell a note from such a copy.
- **No history for a new device**, and gaps after two weeks offline (section 3).
- **Older devices** (before level 9) do not get notes; the app says so on the device that writes them.
- **Large documents** (ADR-043) cannot be sent to Notes: there is no one to download them from.

## Rejected

- **Notes kept on the relay** for every device to fetch: storage on the server, against rule 14 and ADR-005.
- **A `Copy` naming our own identity as the chat:** older clients would file such a copy into the chat with our primary device.
- **A group of one:** Megolm, a roster and a group queue for something that has no other member.
- **Copying old notes to a newly linked device:** a bulk transfer of history that chats do not have either; it can come later for both, in its own ADR.

## Dependencies

None new.
