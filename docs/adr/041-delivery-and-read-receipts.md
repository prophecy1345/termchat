# ADR-041: delivery and read receipts

Status: **accepted** (maintainer, 2026-11-23). Русская версия: [041-delivery-and-read-receipts.ru.md](041-delivery-and-read-receipts.ru.md). Builds on [ADR-025](025-replies-edits-reactions.md) (message ids, `Understands`) and [ADR-028](028-second-device.md) (devices).

## Context

Today a message of ours shows one state after it leaves: "sent", meaning the relay took it. Whether it reached the contact's phone is unknown: it may sit in the queue for days because the phone is off, the app was removed, or the contact wiped their account without a `Goodbye`. People expect two marks from any messenger (PLAN.md P9):

- **delivered** (✓✓): the message is on one of the contact's devices, decrypted;
- **read**: the contact had the chat open on screen with that message in it. Separate, and **off by default**.

Both are said by the contact's app, inside the chat's encryption. The relay cannot say either: it does not know who reads a queue (ADR-023), and it must not learn more.

The cost is metadata, and it is the reason for most of the rules below. A receipt is a message the contact sends without writing anything. The relay sees a small message go into our queue shortly after one of ours left theirs. That pairs the two queues as one conversation by timing, and tells when the contact's device was online. A read receipt adds when they looked at the chat.

## Decision

### 1. One new kind

```rust
Body::Receipt {
    messages: Vec<u64>,   // ids of our Message (ADR-025), at most 64
    files: Vec<u32>,      // ids of our File, whole, at most 64
    read: bool,           // false: delivered; true: read (implies delivered)
}
```

- Ids are always ours, seen from the receiver of the receipt: a contact can only mark messages we sent them. An id we do not know among our messages to this contact is ignored, as is a receipt for a message that is gone (expired, deleted).
- More than 64 ids, or a receipt that does not decode, is ignored whole. Several receipts may come for one message; the marks only go forward (sent, delivered, read), never back.
- A file counts as delivered when its last chunk is in and the document is whole.
- No receipt for a receipt, nor for `Edit`, `Delete`, `React`, system lines or the old `Text`.

### 2. When a receipt goes, and when it does not

- **Delivered**: after a `Message` or a whole `File` from the contact is decrypted and saved on this device.
- **Read**: when the chat is open on screen with the message in it, and only if the read setting is on (section 4).
- **Never** to a contact request not yet accepted (a stranger would learn that the address is alive and when the phone is on), to a blocked contact, or to a contact that has not said `Understands` level 5 (section 6).
- In a muted chat: delivered yes (it is about the device), read as usual.

### 3. Fewer, later receipts

To weaken the timing link the relay could draw:

- Receipts are **gathered per contact** and sent as one: up to 64 ids.
- They wait a **random 5 to 30 seconds** after the first id is gathered. If we send the contact anything meanwhile (a reply, a reaction), the gathered receipt goes right before it, so the relay sees one burst instead of two.
- A receipt sits in the outbox like any message: if the device goes offline, it goes when it is back.

This hides neither that the contact's device is online, nor that the two queues talk. It only blurs the moment, and it saves messages on a busy chat.

### 4. Settings

- **Send delivery receipts**: on by default. Off: none are sent, and ours from others are still shown; the chat with an old client looks the same.
- **Send read receipts**: **off by default**. **Mutual**: with it off, we neither send read receipts nor show the contact's, so "read" is only seen by people who show it themselves. With it on, we send to every contact that understands them.
- Both settings are global, the same on every device of the person (each device keeps its own; the settings already work this way). No per-chat switch in this ADR.

### 5. Devices (ADR-028)

- The contact's device that receives a message sends the delivered receipt; the first one to arrive makes ✓✓. A receipt goes to all our devices, like every message to us.
- A read receipt goes from the device where the chat was opened. Telling the person's other devices that the chat was read there is not in this ADR.
- Our own copies (`Copy`, ADR-030) get no receipts: they are not from the contact.

### 6. Old clients: `Understands` level 5

- `UNDERSTANDS` becomes 5. We send receipts only to contacts at level 5 or more, and show ✓✓ only for chats with them; a chat with an older contact keeps "sent".
- An older client gets nothing new: section 7 of ADR-025 skips unknown kinds quietly since 0.9.0, and it is not sent receipts anyway.

### 7. Groups are out

No receipts in groups in this ADR. In a group of up to 32 every message would cause up to 31 receipts, and everyone in the group would see when everyone else is online and reading. If wanted later, it gets its own ADR.

### 8. Storage, apps and the scriptable mode

- `Msg` gets `delivered: bool` and `read: bool` (`#[serde(default)]`: older messages stay "sent").
- The apps draw the marks as the arcade pixel sprites, not as font glyphs: one ✓ sent, two ✓✓ delivered, two ✓✓ in the accent colour read. The terminal uses one-column glyphs.
- `termchat recv --json` prints `{"kind": "delivered" | "read", "id": ...}` events for our messages. No new `send` options in this ADR.

## What the relay sees, honestly

- A receipt is a short message in the smallest padding bucket, like "ok". The relay cannot tell it from a text.
- **New**: more messages with a fixed shape. After a message lands in one queue, a small one goes into another within about half a minute. Over time this links the two queues as a conversation and shows when the contact's device is online, even though the relay does not know whose queues they are. Read receipts, if on, also show roughly when they looked at the chat.
- Turning delivery receipts off takes this away for what we send; the docs and the setting say so.

## This is not a new cryptographic protocol (rule 1)

A new variant of the plaintext inside Olm, pointing at the ids from ADR-025. Olm, vodozemac and the relay do not change.

## Rejected

- **Delivery from the relay** (it knows when a queue item was fetched). It would need the relay to tell one queue's owner about another queue, which is exactly the link ADR-023 removed.
- **Read receipts on by default.** Being seen reading is a pressure many people do not want; the default stays private.
- **One-sided read receipts** (see others' without sending one's own). The mutual rule is the usual one and does not reward hiding.
- **A time in the receipt.** The mark is enough; a time adds metadata for the contact, and the arrival time already says roughly the same.
- **Receipts in groups** (section 7).
- **"Typing…"**: rejected already in PLAN.md (metadata).

## Maintainer's decisions (2026-11-23)

All five as proposed: delivered on by default with a switch off; read receipts off by default and mutual; files included now; a random 5 to 30 second delay; no receipts in groups.
