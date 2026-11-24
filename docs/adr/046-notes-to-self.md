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

## Amendment 1 (2026-11-24): keeping Notes in order

Proposed by the owner, 2026-11-24: in Telegram "Saved Messages" turns into a heap nobody can find anything in. Borrowing from Raindrop.io what fits a messenger:

1. **Tags from the text.** A word starting with `#` (letters, digits, `_` and `-`, compared without case) is a tag. Tags are part of the note, so they reach our other devices with it; nothing new on the wire.
2. **Filters**, one at a time: **All**, **Untagged** (the inbox: notes with no tag), **Favorites**, **Links** (a note with an `http://`, `https://` or `www.` address), and one per tag with its count. Worked out on the device from what it holds.
3. **Favorites (★).** A note is a favorite while our own reaction is set on it. Starring sends `Note { React }` to our other devices, so `Note` may now carry `React` as well; the receiving device takes it only for one of our notes. The apps draw it as a star, not as a reaction.
4. **Notes stands apart from the contacts**: its own row above the Contacts heading in every app, never counted or listed with people.

Not taken from Raindrop: previews or fetched titles of links. Fetching a page would tell the site, and whoever watches the network, what you saved (rule 9). Folders (collections) wait until tags prove too little; they need a field of their own on the wire.

## Amendment 2 (2026-11-24, proposed): notes missed while a device was not ready

Found by the owner on 0.33.0. A note goes only to our devices that already said they read notes (`Understands` level 9). If none has, the note is still marked sent and is never sent again. So the notes written on the phone between its update and the moment it heard that the computer was updated too never reach the computer. Devices are rarely updated in the same minute, so this hits nearly everyone who links two.

**Decision.** When one of our own devices says it reads notes for the first time (its level goes from below 9 to 9 or more), this device sends it the notes it should have had:

- every note here written **after that device was linked** (`at` not before the device's `added_at`, which section 3 already promises), not deleted and not yet expired;
- each as `Note { Message }` with its own id and the time it was written, its current text and the disappearing time that is **left**, so both copies still go at the same moment; then `Note { React }` for each starred one;
- oldest first, **at most 30 messages** in all: the relay's burst for one queue (1 a second after that). Older ones stay where they are; a later note goes as usual.

The receiving device already drops a `Note { Message }` whose id it holds, so a note sent twice shows once, and both devices may do this to each other at the same time.

**Nothing new on the wire**: the same `Note` bodies, the same Olm sessions, only to our own devices. The relay sees up to 30 messages between two of our queues once, as when that many notes are written in a row.

**Not changed:** notes written before a device was linked are still not copied to it (the rejected bulk copy above). A device that was offline for more than 14 days still misses what the relay dropped meanwhile: that loss is the relay's, not a level mismatch.

**Rejected:** keeping notes queued until every device reads them: one device left on an old version would hold every note back for the others, and show them as unsent forever.

## Implementation notes (2026-11-24)

- **Notes is a contact that is no one** (`notes`, key `NOTES_KEY`, all 0xff), so every chat feature works on it in all three apps. It has no identity slot, so no queue, key or session is ever made for it; sending to it marks the note sent and passes it to our devices.
- **Not pinned**: the apps draw it on a row of its own above the people (amendment 1).
- **Documents are not in Notes yet:** a document we send is dropped once sent, and Notes needs a copy kept here to save later. That comes in a follow-up.
- A device holding only Notes still counts as new and can be linked.
- **Menus fit a chat with yourself** (owner, 2026-11-24, after 0.33.0): Search, Export, the disappearing timer and Clear history; no answer or reactions on a note, "Delete" instead of "Delete for both"; no mute, rename, pin, archive or sending a file. The long press on its row on Android offers the same.

## Dependencies

None new.
