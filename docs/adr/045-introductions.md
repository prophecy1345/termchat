# ADR-045: introducing two contacts, with the consent of the one introduced

Status: **accepted** (owner, 2026-11-24). Русская версия: [045-introductions.ru.md](045-introductions.ru.md). PLAN.md P12. Builds on [ADR-013](013-invite-codes.md) (one-time invite codes), [ADR-014](014-incognito-contacts.md) (incognito identities) and [ADR-023](023-anonymous-queues.md) (anonymous queues).

## Context

Alice knows Bob and Carol, and wants them to talk. Today Carol has to make an invite and hand it to Bob some other way, often over a less private channel. People ask for "share contact", as in other messengers.

The constraints:

- **Rule 15: no reach.** Nobody can be found or written to without letting them in. A shared contact must not become a way to reach Carol without her saying yes.
- **ADR-023: queues are per contact.** Alice's queue to Carol is theirs alone. Handing it to Bob would let Bob write as if he were a contact Carol never accepted, and would link Alice to Bob on the relay. So Alice never passes on anything of her own.
- **No new crypto** (rule 1). Everything rides on existing messages and the existing invite.

## Decision

### 1. The flow

Alice opens her chat with Bob and picks **Introduce…** → Carol. Then:

1. **Alice → Carol: `IntroAsk`.** "Alice wants to introduce you to Bob", with Bob's nickname and identity key, so Carol sees whom she would meet (nickname and fingerprint).
2. **Carol decides.** In her chat with Alice: **Allow** or **Decline**. Allow can use her main identity or, if she wants, a new incognito one (ADR-014), so Bob cannot link her to her other contacts.
3. **Carol → Alice: `IntroGrant`**, with a fresh **one-time invite** of the identity she chose (ADR-013), made for this introduction only. Decline sends `IntroDecline`, and Alice's chat says so; Bob hears nothing.
4. **Alice → Bob: `IntroOffer`**, with Carol's nickname and her invite. Alice's app passes it on without a further tap: Alice already decided when she started.
5. **Bob decides.** In his chat with Alice: "Alice introduces Carol·67890: **Add** / **Ignore**". Add is the ordinary add from an invite: a contact request with Carol's one-time code.
6. **Carol's app accepts that request by itself** if it carries the code made for this introduction *and* comes from the identity key Alice named in step 1. Carol already said yes. Any other request with that code (the invite leaked) is an ordinary pending request.

Carol's consent comes first; Bob learns of her only if she allows it, and nothing reaches Carol from Bob unless Bob chooses to add her.

### 2. New `Body` variants

Appended at the end of `Body`, as for every new kind:

```rust
Body::IntroAsk     { intro: u64, nickname: String, identity: Identity } // about Bob, to Carol
Body::IntroGrant   { intro: u64, invite: Invite }                       // Carol's, to Alice
Body::IntroDecline { intro: u64 }
Body::IntroOffer   { intro: u64, nickname: String, invite: Invite }     // Carol's, to Bob
```

- `intro` is random, chosen by Alice, and names the introduction in all three chats.
- `nickname` follows the nickname rules; `invite` follows the invite rules (URLs, version), both checked in `proto` and fuzzed.
- Only between accepted contacts, never in a group or a `Copy`, primary devices only (the invite is made by Carol's primary).
- `Understands` level **8** for all three. An older Carol or Bob: the menu says their app cannot take introductions yet.

### 3. Checks on each side

- **Carol** accepts `IntroAsk` only from a contact; at most **5** waiting at a time per contact; each expires after **7 days**. An `IntroAsk` naming Carol's own key or one of her existing contacts says so ("you already know them") instead.
- **Alice** passes on an `IntroGrant` only for an `intro` she started, only to the contact she started it for, once.
- **Bob** shows an `IntroOffer` only from a contact, only once per `intro`. When he adds, his app checks that the identity reached through the invite is the one in the invite (as for any invite); the new contact is marked **not verified** and shows "introduced by Alice".
- **The one-time code** made for the introduction is kept with the `intro` and Bob's expected key. It is used up by the first request, like any one-time code, and dropped after 7 days if unused.

### 4. What Alice can and cannot do

Alice is trusted as far as an introducer can be: she sees nothing of the new conversation, which runs on its own sessions and queues. But she relays the invite, so she could swap Carol's invite for one of her own and sit in the middle. That is why the new contact starts **not verified**: Bob and Carol should compare their safety number (in person or by QR, ADR-037) before trusting it, as with any invite that came through someone else.

### 5. Interface

- **Desktop and Android:** in the chat menu, **Introduce…** picks a contact. System lines in the chats show what happened: "you introduced Bob to Carol; waiting for Carol", "Carol declined", "Carol·67890 added you, introduced by Alice".
- **Terminal:** the same from the chat menu (Ctrl+P), with the answers as keys in the chat, like a contact request.
- No list of anyone's contacts is ever shown to anyone; Alice picks from her own list.

## Consequences and honest limits

- **Bob's existence is revealed to Carol** in step 1 (his nickname and key), on Alice's decision, before Bob agrees to anything. This is the same as Alice telling Carol about Bob in a message; it is still worth saying in the menu: "Carol will see Bob's nickname and fingerprint".
- **The relay** sees nothing new until Bob adds Carol; then it sees an ordinary contact request to Carol's invite queue. It cannot tell an introduction from any other invite.
- **The introducer can sit in the middle** (section 4); safety numbers are the answer, as everywhere.
- **No chains.** Bob cannot pass Carol on: introductions always ask the one introduced.

## Rejected

- **Sending Carol's contact card** (her key and nickname) without an invite: Bob could not reach her, or Alice would have to hand over her own queue (ADR-023).
- **Alice's own queue to Carol:** links Alice and Bob on the relay and skips Carol's consent.
- **Carol's public invite** (ADR-013): reusable, so it spreads past Bob; and it does not ask Carol.
- **Bob asked first, then Carol:** reveals Carol to Bob before she agreed; Carol's consent matters more, because her address is what is shared.
- **Introductions in groups:** a group already brings its members together on the admin's invitation (ADR-027).

## Dependencies

None new.
