# ADR-025: replies, edits, deletes and reactions

Status: **accepted** (maintainer, 2026-11-17). Русская версия: [025-replies-edits-reactions.ru.md](025-replies-edits-reactions.ru.md).

## Context

People expect four things from any messenger, and termchat.app has none of them:

- **reply** to a particular message;
- **edit** a message already sent (a typo, a wrong number);
- **delete** one's own message on the other side too;
- **react** to a message without writing a new one.

All four happen between two people, inside the encryption the chat already has. The relay must learn nothing new, and no new cryptography is needed. Two problems must be solved first:

1. **A message has no name.** `Inner.seq` counts per Olm session and per direction, and it starts again with every new session, so it cannot point at a message. `Body::Text` has no id at all.
2. **Old clients cannot read new kinds.** `Inner` is decoded by postcard as a whole. A body variant it does not know makes the whole message "malformed": the client shows "a message could not be decrypted" in the status bar, and the next message reports "1 message lost", because that `seq` was never accepted.

Rule 9 (ADR-017) stays: no markup, no previews, no pictures. A reply is a pointer, not quoted text; a reaction is one of a fixed few signs.

## Decision

### 1. A new kind of text, with an id

```rust
Body::Message {
    id: u64,                 // random, chosen by the sender
    sent_at: u64,
    expires_in: Option<u32>,
    reply_to: Option<Ref>,
    text: String,            // MAX_TEXT_BYTES, as Text
}

struct Ref { id: u64, theirs: bool }
```

- `id` comes from the operating system's random source (`getrandom`), 64 bits. It is a name, not a secret.
- Every party names its own messages. `Ref.theirs` says whose message is meant, seen from the sender of the `Ref`: `false` for its own, `true` for the recipient's. So a contact cannot point at our message by reusing its id among theirs, and two random ids from different sides never clash.
- `Body::Text` stays for contacts with old clients (section 6).

### 2. Replies

`reply_to` points at a message; **the quoted text is not copied**. The app shows the quote from its own copy. If that copy is gone (expired, deleted, never arrived), it shows "reply to a message that is gone". So a reply never carries a disappearing message's text past its timer.

### 3. Edits

```rust
Body::Edit { id: u64, text: String }
```

- Only one's own `Message`. The recipient looks for `id` only among the sender's messages; anything else is ignored silently.
- The edited message keeps its timer (`expires_at` does not move) and gets an "edited" mark. **No history is kept**: the old text is wiped (`zeroize`), as with every message that goes.
- An edit for a message that has already expired or was deleted is ignored.
- Files cannot be edited.

### 4. Deletes

```rust
Body::Delete { id: u64 }
```

- Only one's own `Message`; files come later (decision 3).
- The recipient wipes the text and leaves a line "message deleted", so the other side sees that something was removed.
- Honestly, in the app and the docs: **a delete is a request**. A modified client, a screenshot or a backup keeps what it kept. The text says "deleted for both", never "deleted everywhere".

### 5. Reactions

```rust
Body::React { target: Ref, reaction: Option<u8> }
```

- A **fixed set** of seven, by index: like, love, laugh, wow, sad, thanks, dislike (in the sense of 👍 ❤️ 😂 😮 😢 🙏 👎). `None` takes one's reaction back. An index outside the set is ignored.
- **Drawn by termchat.app, not by an emoji font**, as 1980s arcade pixel sprites with a soft glow, in the theme's colours (dislike is a red arrow down): the same picture on every device. The terminal shows one-column glyphs (▲ ♥ ☺ ◎ ☹ ✧ ▼). The wire carries only the index, so the drawing can change without a protocol change.
- One reaction per person per message; a new one replaces the old.
- Free text or any emoji as a reaction is not allowed: it would be a second text channel past every limit, and arbitrary emoji break the terminal's column widths.
- A reaction goes with its message: it expires and is deleted with it.

### 6. Old clients: say what you understand, once

```rust
Body::Understands { level: u16 }    // 1 = this ADR
```

- An updated client sends `Understands { level: 1 }` once to every contact: on the first connection after the update, and to a new contact right after `ContactRequest` / `ContactAccept`. The level is stored per contact. ("Once" was not enough when the two sides update at different times: [ADR-026](026-understands-again.md).) ("Once" was not enough when the two sides update at different times: [ADR-026](026-understands-again.md).)
- To a contact that has not said level 1, the client sends only `Text`, and the app does not offer reply, edit, delete or react for that chat. It says why, once: "their app is older; replies and reactions come after they update".
- **The cost:** a contact on 0.8 or older gets one status-bar line "a message could not be decrypted (malformed)" and one "1 message lost" line in the chat, when `Understands` arrives. Nothing is lost in fact. This is said in the release notes.

### 7. From now on, unknown kinds are skipped quietly

The client decodes the head of `Inner` (`sender`, `recipient`, `seq`) apart from the body. If the body is a kind it does not know, it accepts the `seq`, shows nothing and goes on. From the next release on, a new kind of message costs older clients nothing, and section 6's cost is paid once only.

### 8. Storage and the scriptable mode

- `Msg` gains `id`, `reply_to`, `edited` and the two reactions (`#[serde(default)]`: older files read as before; their messages simply cannot be replied to).
- `termchat recv --json` gains `"id"` and `"reply_to"`, and prints edits, deletes and reactions as events of their own (`"kind"`: `message`, `edit`, `delete`, `react`). `send` gets no new options yet.

## What the relay sees, honestly

- Nothing new. All four kinds are inside Olm and padded to the usual buckets.
- An edit, a delete or a reaction is a short message: the relay sees a message in the smallest bucket, just like "ok". It sees that something was sent, and when, as with every message.

## This is not a new cryptographic protocol (rule 1)

New variants of the plaintext inside Olm, and random ids for naming messages. Olm, vodozemac and the relay do not change.

## Rejected

- **Quoting the text inside the reply.** Carries a disappearing message's text past its timer, and doubles the text on the wire.
- **Any emoji or free text as a reaction.** A second text channel past the limits; terminal widths.
- **Edit history.** Keeps what the author meant to replace; the app keeps nothing it does not need.
- **Fallback text for old clients** ("👍 to your message"): litters the chat with lines that look like messages.
- **A cut-off version instead of `Understands`** (everything new from 0.9 on, for everyone): contacts on 0.8 would lose messages without a word.
- **`seq` as the message id**: it starts again with every Olm session.

## Maintainer's decisions (2026-11-17)

1. No time limit for edits and deletes.
2. Six reactions plus dislike (a red arrow down), drawn by termchat.app as pixel sprites rather than as system emoji (section 5).
3. Texts only for now; replies, deletes and reactions for files come with the next change to `File`.
4. Section 6's one-time cost is accepted: `Understands` and section 7 ship together.
5. `recv --json` prints edits, deletes and reactions as events (section 8).
