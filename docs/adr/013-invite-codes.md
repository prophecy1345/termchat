# ADR-013: one-time and revocable invites

Status: **accepted** (maintainer, 2026-10-19). Русская версия: [013-invite-codes.ru.md](013-invite-codes.ru.md).

## Context

An invite today is the relay address plus the identity keys
(`proto::Invite`). It holds no secret and never expires. So:

- anyone who ever saw it can send a contact request, forever;
- a leaked or posted invite cannot be withdrawn short of wiping the account;
- the identity key is enough to send a request, and the key also travels
  inside every contact request.

ADR-007 limits how much a stranger can queue (20 envelopes per pair), but it
cannot stop requests from reaching the user. SimpleX and Briar address this
with one-time links.

## Decision

### 1. An invite code inside the invite
- `Invite` version 2 adds `code: [u8; 16]`, a random code from the OS RNG.
- `Body::ContactRequestV2 { identity, nickname, note, code }` is a new variant
  at the end of `Body`. It carries the code from the invite the sender used,
  inside Olm. The relay never sees it.
- **No relay change.** This is a check made by the recipient's client, after
  decryption.

### 2. Two kinds of invite
- **One-time** (the default for the QR code and the copy button):
  - a new code each time the invite is shown;
  - the code works for one request, then it is gone.
  - The device keeps the 20 newest unused one-time codes; older ones stop
    working.
- **Public** (for a profile or a post):
  - one reusable code;
  - **"New public invite"** replaces it, and every earlier public invite stops
    working.
- **"Revoke unused invites"** drops all unused one-time codes.
- No names, labels or dates are stored per code. There is nothing to list but
  the counts.

### 3. Checking a request
A request from a stranger (no contact yet) is accepted only if:
- its code matches a live code; or
- it carries no code (`ContactRequest`, an old client or an old invite), and
  the setting **"Accept requests without an invite code"** is on.

Otherwise the client drops it silently: no contact, no session, no
notification. The relay gets the usual `Ack`, so the queue is cleared.

Other rules:
- A one-time code is spent when its request arrives, before the user accepts
  or rejects it.
- A request from someone already in the contact list needs no code. This
  covers both people adding each other at the same time.
- Codes are compared in constant time (`subtle`).

### 4. Compatibility
- New clients read version 1 and version 2 invites.
- A version 2 invite goes with `ContactRequestV2`; a version 1 invite goes with
  `ContactRequest`.
- Old clients reject a version 2 invite as `UnsupportedVersion`. The app says
  "this invite needs a newer termchat".
- "Accept requests without an invite code":
  - **on** for accounts that existed before this release, because their old
    invites are already out;
  - **off** for new accounts;
  - a notice in "What's new" suggests turning it off once old contacts are
    added.
- The `Invite.version` check is separate from the envelope `VERSION`, which
  stays 1.

### 5. Backups and moving
- Codes live in the encrypted state file, so a backup and `MOVED` carry them.
- A duress wipe removes them along with everything else.

## Not a new crypto protocol (rule 1)

The code is a random bearer secret, compared for equality inside an Olm
message that is already authenticated. No primitives are combined. What an
invite proves stays as it is: who owns the key.

## What changes for metadata

- **Relay:** nothing. It sees the same envelopes.
- **Someone holding a revoked or spent invite:**
  - can still claim prekeys and send a request, which costs a little queue
    space under the ADR-007 caps;
  - gets no answer, and cannot tell "dropped" from "not yet accepted";
  - still knows the identity key, so they can recognise this person's key
    elsewhere. Only a new identity changes that.

## Rejected

- **Checking the code on the relay:** the relay would learn which invite
  brought which contact, and a code per contact links people together.
- **Expiry dates per code:** dates kept in a file are more metadata. The cap
  of 20 newest one-time codes does the same job.
- **Labels per invite** ("for Alice"): that is personal data (rule 14), and
  it is not needed to revoke.

## Dependencies

None new. `getrandom` is already used by `core` and `engine`. `subtle` is already in `Cargo.lock` through vodozemac, and becomes a direct dependency of `engine`, which makes the check.
