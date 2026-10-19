# ADR-014: incognito contacts, a separate identity per contact

Status: **accepted** (maintainer, 2026-10-19). Русская версия: [014-incognito-contacts.ru.md](014-incognito-contacts.ru.md).

## Context

One account has one identity key. That key is in every invite, every request
and every fingerprint. Two consequences:
- **Contacts can link you.** Two contacts who compare safety numbers know they
  talk to the same person, whatever nickname each of them sees.
- **The relay can link your contacts.** It sees every contact talk to the same
  key, so it knows your whole circle.

A different nickname per contact would hide neither, so it is not done
(owner, 2026-10-19). SimpleX's incognito mode gives each contact its own
identity. This ADR does the same, using only what termchat already has: an
identity is a vodozemac Olm account and an account on the relay.

## Decision

### 1. Profiles
- A **profile** is one identity:
  - its own Ed25519 and Curve25519 keys (its own Olm account);
  - its own prekeys;
  - its own nickname;
  - its own account and connection on the relay.
- Every contact belongs to exactly one profile.
- **The main profile** is the identity the account has today. Normal invites,
  public invites and every existing contact use it. Nothing changes for them.
- **An incognito profile** is created for one contact and used only with that
  contact.

### 2. Incognito invites (opt-in)
- The invite window gets a third kind: **incognito**. Showing one:
  - creates a new profile with a new nickname, which the user types; empty
    means a random word pair;
  - publishes the profile's prekeys on the relay;
  - shows a one-time invite of that profile (ADR-013), with the profile's own
    code.
- When the friend adds it, the request arrives at that profile only.
- Adding someone else's invite gets the same choice: **"add as incognito"**
  creates a new profile and sends the request from it.
- An incognito profile with no contact is deleted (`Unregister`, ADR-009)
  when its invite is revoked or replaced by newer ones. At most 5 such
  waiting profiles exist.

### 3. Keeping profiles apart on the relay
- Each profile has its own WebSocket connection and authenticates only its own
  key. No frame ever names two profiles.
- **Under Tor**, each profile's connection uses its own circuit (arti stream
  isolation). The relay then cannot tell from the network that two profiles
  belong to one device.
- **Without Tor**, every profile connects from the same IP address, and the
  relay can link them that way. The app says this next to the switch, and
  recommends Tor for incognito contacts.
- **Timing.** Profiles connect with a random delay of 0–30 s each after
  unlock, and again before every reconnect, so they do not all come and go
  within the same second.
- **Numbering.** Each profile numbers its `Send` frames itself, from a random
  start. One shared counter would show the relay which keys belong together.
  (Found by the security review at implementation.)
- **Push (ADR-012).** Each profile registers its own UnifiedPush instance with
  its own endpoint. A shared endpoint would link the profiles on the relay.

### 4. Lifecycle
- **Deleting an incognito contact** sends a goodbye (ADR-009), then
  `Unregister` for its profile, and erases the profile. The relay forgets that
  key.
- **WIPE / MOVED:** every profile says goodbye to its contacts and
  unregisters, the same as the main one today.
- **The duress password** erases every profile with the state file. Nothing is
  sent, as today.
- **Backups** carry every profile, its keys and its contacts, in the same
  encrypted file.
- **A report or a block** acts within the profile of that contact.

### 5. What is not done
- Incognito is **not the default**.
  - Every profile is one more connection: under Tor, one more circuit. With
    many contacts that costs battery and start-up time on a phone.
  - The user decides per contact.
- **No sealed sender (B3) for now.**
  - Sealed sender would hide the sender from the relay inside a pair of keys.
  - With incognito profiles, each pair is already two keys used for nothing
    else, so hiding one side adds little.
  - For main-profile contacts the relay keeps seeing the graph. Incognito is
    the answer for people who need more.
  - This replaces plan stage B3. The owner may bring it back later.

## Metadata, honestly

- **The relay** sees a pair of keys that talk only to each other. Under Tor,
  with separate circuits, it cannot link the pair to your other keys from the
  network. Across profiles it may still guess from coincidences:
  - connection times, even with random delays;
  - an app version that only a few people run;
  - message timing.
- **Locking the app** closes every identity's connection at the same moment:
  the keys must leave memory. A relay that watches can link identities that
  repeatedly disconnect together.
- **WIPE** waits until every identity is connected, and finishes only after
  the relay has confirmed forgetting each of them, or after the timeout.
- **The contact** sees a key, a nickname and a fingerprint used nowhere else.
  They cannot link it to your other contacts unless you tell them, or the text
  itself gives you away.
- **The device** holds every profile. Anyone who unlocks it sees them all.

## Not a new crypto protocol (rule 1)

- Each profile is an ordinary vodozemac account, used exactly as the main one
  is.
- Nothing is derived from the main key, and nothing links the keys
  cryptographically.
- No new primitives are combined.

## Rejected

- **A different nickname per contact, one key:** it hides nothing from
  contacts who compare keys, and nothing from the relay.
- **Deriving profile keys from the main key:** whoever has one key could find
  the others, and it would be homemade crypto (rule 1).
- **Many profiles over one connection:** the relay would see them together.
- **Incognito for everyone by default:** the cost on phones is too high for a
  default.

## Dependencies

None new. arti already supports stream isolation (`IsolationToken`), and
UnifiedPush supports several instances per app.
