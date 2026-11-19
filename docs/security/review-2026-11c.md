# AI-assisted security review, November 2026 (third pass)

Status: [ADR-030](../adr/030-keys-by-someones-word.md) accepted. Fixed in
0.16.1: findings 2, 3, 5, 7, 8, 9, and the attack of finding 1 (a planted
member no longer counts on the invite queue); finding 4 where a hostile
contact lists someone else's key. Fixed in 0.17.0 (level 4): device proofs (the
rest of finding 1), copies that name their chat (the rest of finding 4) and
gaps in groups (finding 6). Open: the relay-wide byte cap of finding 9. The proofs of concept run as regression tests in
`crates/engine/tests/review_2026_11c.rs`.
Reviewed commit: `934d427` (0.16.0). Date: 2026-11-19.
Earlier passes: [first](review-2026-11.md), [second](review-2026-11b.md).

**This is not an independent security audit.** It is a source review done by
an AI model (Claude Fable 5.2) at the maintainer's request. A model can miss
whole classes of bugs and cannot vouch for what it did not flag. termchat.app
stays experimental until people who are independent of the project audit it.

## Scope

The second pass looked at 0.6.1. Since then came anonymous queues (ADR-023),
scriptable mode and the daemon (ADR-024, ADR-029), replies and edits
(ADR-025), groups (ADR-027) and linked devices (ADR-028). This pass is mostly
about those.

Read in full: `crates/proto`, `crates/core`, `crates/relay`, `crates/engine`,
`crates/client/src/cli.rs`, `crates/client/src/cli/daemon.rs`,
`crates/client/src/main.rs`, ADR-022 to ADR-029, `docs/threat-model.md`.

Read in part (searched for untrusted-text rendering, file and process use,
and what changed since the second pass): the rest of `crates/client`,
`crates/desktop`, `crates/ffi`, the Android manifest and Kotlin sources,
`deny.toml`, `deploy/`, the fuzz targets.

Run: `cargo test --workspace --locked` (all passed), `cargo deny check
advisories bans sources licenses` (ok), `cargo vet --locked` (ok).

Not reviewed: vodozemac, Arti, egui and every other dependency; the interface
logic of the three clients line by line; the website; the install and release
scripts (unchanged since the second pass except `package-gui.sh`); the live
relay. No fuzzing was run. Duress-password timing was not measured.

Findings 1 to 4 were confirmed with temporary tests, kept in
[poc/review-2026-11c.rs](poc/review-2026-11c.rs). Each test states the safe
behaviour and fails on `934d427`. To run them, copy the file to
`crates/engine/tests/`, add `vodozemac = { version = "0.11.2",
default-features = false }` to the engine's dev-dependencies, and run
`cargo test -p termchat-engine --test review-2026-11c -- --test-threads=1`.
The tests hand envelopes to the victim's engine directly, as a relay would
deliver them. The other findings come from reading the code.

## Findings

| # | Severity | Where | Summary |
|---|---|---|---|
| 1 | Critical | `core` `add_member`, `decrypt`; `engine` `their_devices`, `know_members`, `receive` | A contact plants a contact that carries someone else's fingerprint and is accepted without a request |
| 2 | High | `engine` `their_devices` | A contact erases our sessions with any other contact |
| 3 | High | `engine` `start_links`, `group_from_contact`, `receive` (`QueueOffer`) | A relay address from a peer makes the state file impossible to open |
| 4 | Medium | `engine` `receive`, `writers_of`, `transmit_each`, `own_copy` | Messages can be moved between conversations |
| 5 | Medium | `engine` `group_devices_changed`, `start_move`, `take_member_devices` | A group member who is not the admin removes another member |
| 6 | Medium | `core` `group.rs`, `relay` `handle` (`Send`), `engine` `group_receive` | Group messages can be dropped without anyone noticing |
| 7 | Low | `engine` `addr_ok` | A peer's relay address may point at this machine; what a foreign relay learns is not in the threat model |
| 8 | Low | `engine` `group_receive` | Group members can fill the 50 contact-request slots |
| 9 | Low | `relay` `handle` (`Report`), `put` | The report journal and the relay-wide byte cap can be filled cheaply |
| 10 | Info | various | Smaller notes |

### 1. A planted contact with someone else's fingerprint (Critical)

Who: any accepted contact who also holds one of our invites that still works
(the public invite, or an unused one-time invite). A group admin can do the
same through a roster, and any group member through an honest admin, who
passes members' device lists on.

An `Identity` binds a Curve25519 key to an Ed25519 key with a signature, and
the fingerprint and safety number are computed from the Ed25519 key alone.
`Device` (ADR-028) and `RosterMember` (ADR-027) carry both keys with no proof
that the Ed25519 key's holder agreed: the list is signed only by its owner or
by the admin. `Client::add_member` stores such a pair as an ordinary contact
with `is_request: false`.

Steps, as in the test:

1. Mallory sends `Devices` with a list she signed: herself, and a "device"
   with Victor's Ed25519 key and a Curve25519 key of her own.
   `their_devices` calls `add_member` with that pair.
2. From an Olm account with that Curve25519 key she writes a pre-key message
   to our invite queue: `Inner.sender` is Victor's key, the body is
   `ContactRequestV2` with a valid invite code.
3. `Client::decrypt(None, …)` finds the planted entry by the Curve25519 key
   Olm vouches for, so the sender is "known" and the check that binds a
   stranger's request to its signed identity is skipped.
4. `Engine::receive` has no contact under that key, admits the code and adds
   one. `is_request` is false, so the branch for "a contact we have, asking
   again" runs: `accept_pending`, the line "connected", a queue of ours.

Result of the test:

```
contact "Victor·68005" fingerprint 68005 92137 48482 08809 34875 88027
request=false accept_pending=true curve_is_attackers=true
```

- The contact appears accepted; no request was shown.
- Its fingerprint and safety number are Victor's real ones. Comparing them
  with Victor in person succeeds, while every message goes to Mallory. This
  breaks threat-model item 3.
- A daemon identity (ADR-029), which has no command to accept anyone, gets a
  contact it can be written to and that can write to it.

Without a valid invite code the planted pair still does harm: Victor's real
request is refused with `SenderMismatch`, and so is adding his invite, for as
long as the pair stays in the state file.

### 2. A contact erases our sessions with another contact (High)

Who: any accepted contact who knows the other contact's Ed25519 key (it is in
every invite, and in every roster of a shared group).

`their_devices` accepts any key in a device list. When a later list drops a
key, it calls `forget_sessions` for it, whoever that key belongs to.

Test: Mallory sends a list naming Bob's key as her device, then list 2
without it. `Client::has_session(bob)` turns false.

- Messages from Bob that are on their way no longer decrypt; they are
  acknowledged and lost.
- Both sides see "restarted the conversation".
- The same works against our own linked devices (the channel between them)
  and against group members known only through a roster.
- It can be repeated at will.

### 3. A peer's relay address locks the account out (High)

`Engine::open` runs `start_links(true)?`. If any stored address cannot be
dialled, unlocking fails, with the right password, in every client. There is
no way back from the interface.

Two ways a peer puts such an address into the state file:

- **`GroupInvite` (confirmed).** Its `relay_url` and `onion_url` are never
  checked with `addr_ok`. The user accepts an invite to a group on
  `ws://relay.attacker.example/`; `take_group_invite` saves the group, then
  `sync_links` fails. From then on: `bad relay URL: …` at every unlock. Who:
  a contact, once the user accepts their group invite. On a linked device
  the invite is taken without a question when another device joined
  (`OwnGroupJoin`).
- **`QueueOffer` with Tor on (confirmed, with a caveat).** `addr_ok` allows an
  address without an onion URL. With Tor on, `start` refuses it: "Tor is on
  but the relay has no onion URL". The handler ignores the error and saves
  the address. Who: any accepted contact, with no action by the user. The
  same holds for `Devices`, `SiblingQueue` and `LinkedContact` addresses.
  Caveat: the test switched Tor on by setting the field, not through
  `toggle_tor`.

An honest contact on another relay without an onion address has the same
effect on a Tor user. `add` refuses such an invite; the later messages do not.

A failed link also stops `start_links` half way, so links that sort after it
are not started until the next call.

### 4. Messages can be moved between conversations (Medium)

`Inner` names the sending and the receiving device, not the conversation.
A queue of ours for contact X is read with the sessions of every key that
"may write there": X's listed devices and our own devices (`writers_of`).

- **Confirmed.** Mallory lists Bob's key as her device. An envelope Bob
  wrote to us, delivered into our queue for Mallory, decrypts with Bob's
  session and is shown in Mallory's chat as hers. Who: Mallory together with
  the relay, or Mallory running the relay. Bob's chat shows a gap.
- **From reading, same cause.** If Mallory also gives an address for that
  "device", `transmit_each` encrypts everything we write to Mallory with our
  session with Bob as well, and sends it to her queue. She cannot read it.
  Our sequence numbers with Bob advance, so Bob sees "messages missing"; with
  the relay's help the envelopes can be put into Bob's queue, where they read
  as our messages to Bob.
- **From reading, no hostile contact needed.** Copies between our own
  devices (`own_copy`) do not say which contact they are about. A relay that
  can tell which queues are one device's (shared link, no Tor, timing) can
  deliver a copy into another contact's queue, and the linked device files
  "we wrote this" under the wrong chat.

This weakens threat-model item 10 (misattribution).

### 5. A member who is not the admin removes another member (Medium)

From reading. Needs a group whose members all understand devices.

1. Mallory, a member, sends the admin a device list naming Bob's key (Bob is
   a member too). The admin's app asks the relay for a reader for that
   "device" and publishes the list to the group.
2. Mallory sends a list without it. `group_devices_changed` puts Bob's key
   into `gone`, and `start_move` leaves every roster entry with that key out
   of the new queue.
3. `finish_move` signs a roster without Bob and tells Bob "the admin removed
   you from the group".

On the other members, `take_member_devices` forgets Bob's Megolm sessions.
ADR-027 says only the admin removes members.

### 6. Group messages can be dropped without notice (Medium)

From reading.

- A chat counts `seq` inside the ciphertext and shows "messages missing".
  `Groups::decrypt` returns no message index and nothing looks for gaps, so
  a relay can withhold single group messages from single readers, and each
  member sees a different conversation. The threat model says a dropping
  relay "is noticed".
- The relay does this by itself: on a `Send` to a group, `put` runs for each
  reader and its refusal (`RateLimited`, `QueueFull`) is ignored; the sender
  gets `Accepted`. A reader who was away past the 10 MiB quota misses
  messages silently.
- Every member knows every other member's reader `send_id` (it is in the
  roster). Writing to it faster than one envelope per 2 seconds keeps that
  reader's send budget empty, so group messages for that member are dropped.
- The admin receives each reader's token from the relay before handing it
  on, so the admin can subscribe to a member's reader and acknowledge
  (delete) what waits there. ADR-027 does not say so.

### 7. Peer-supplied relay addresses (Low)

- `addr_ok` accepts `ws://localhost`, `ws://127.0.0.1` and `ws://[::1]` with
  any port and path from a peer. The client then sends a WebSocket upgrade
  (an HTTP `GET` with a path and query the peer chose) to a service on the
  user's own machine. A contact needs no user action for this.
- Without Tor, a contact who names a relay of their own learns the user's IP
  address and when the app is online. This follows from ADR-023, decision 4,
  but the threat model does not say it. With Tor on there is no direct
  connection.

### 8. Group members can fill the contact-request slots (Low)

From reading. Olm envelopes on a group reader go through
`Client::decrypt(None, …)`, the invite-queue path. A pre-key message with a
well-formed `ContactRequest` from a new key is stored in the core as a
pending request; the engine then drops the body but never removes the entry.
After 50 of them `TooManyRequests` refuses every real request on the invite
queue, for good. Who: anyone who knows the reader's `send_id`, so any member.

### 9. Relay: the report journal and the global cap (Low)

From reading.

- `reports_per_connection` starts full on every connection. One queue owner
  who reconnects can write 10 reports of 16 KB per connection, and fill the
  10 MiB journal with about 64 connections. After that the texts of real
  reports are not kept.
- All queues together may hold 10 GiB. About 1000 full queues reach it; the
  creation budget is 20 an hour per IP and 200 an hour for all onion
  connections, so some tens of addresses, or a few hours through the onion
  port, are enough. Then every `Send` on the relay gets `QueueFull` until
  the envelopes expire (14 days).

### 10. Smaller notes (Info)

- Megolm replays: `seen` keeps 1000 indexes per session and forgets the
  lowest. Honest senders rotate after 100 messages, so only a sender who does
  not rotate opens a window, and that sender can repeat itself anyway.
- A member's own nickname goes into the line "… joined the group" and a
  group's name into "invites you to the group …", both with status
  `System`. Control and bidi characters are stripped where drawn, but the
  text can imitate the app (compare finding 7 of the second pass).
- `MAX_UNSAVED` counts bytes received so far, checked at a file's first
  chunk. Files that start together from several contacts or group members
  can pass 8 MiB in total.
- The daemon and the commands add `timeout` and `wait` to `Instant::now()`
  unchecked; a huge value panics. Only the same user can send it.
- `Relay::mem` is not pruned when queues expire; it is bounded by the
  creation limit.
- Notes of item 10 of the first pass are unchanged: prekeys carry no type or
  date, `check_nickname` and `check_label` allow bidi characters.

## Fixes of the earlier passes, checked

The fixes listed in both earlier reviews are still in the code: ADR-019 in
`Client::decrypt` and `remember`, frame limits on both sides, queue pages,
the push cap, `safe_name`, `write_atomic`, the idle clocks, the onion-only-
through-Tor rule, the 10 MiB journal cap. `caddy` is pinned by digest. The
SQLite advisory is still ignored for the reason recorded in `deny.toml`.

## Checked, no problem found

- Relay: queue IDs and tokens are random and made by the relay; tokens are
  stored hashed and compared in constant time; an unknown queue and a wrong
  token answer alike; only the current subscriber acknowledges; SQL is
  parameterized; group and reader frames check the admin token.
- Wire format: the new kinds have size limits, canonical encoding is still
  enforced, an unknown kind keeps its `seq`.
- Invite and link codes: compared in constant time, one-time codes are
  spent, a link code and an invite cannot be taken for each other.
- Device linking: nothing secret is sent before both sides confirm; a linked
  device accepts lists only from its primary, signed and newer.
- Rosters: signed by the admin, numbered, taken only from the admin.
- Megolm: the plaintext names its sender and group and both are checked; a
  key is filed under the Olm sender that brought it.
- Daemon: the socket is in the `0700` state folder, `0600`, with a peer
  user check; it opens no paths; request lines are capped; text from
  contacts is cleaned; there is no command to accept a request.
- Password file: owner and mode are checked; the password never comes from
  the environment.
- Documents: names are made safe on save, files are created new with mode
  `0600`, nothing is opened or rendered.
- Android: no new exported components; the lock alarm receiver is not
  exported; `FLAG_SECURE` and the backup rules are as before.
- Dependencies: `cargo deny` and `cargo vet` pass.
