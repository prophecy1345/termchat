# AI-assisted security review, November 2026 (fourth pass)

Status: findings 1 to 5 fixed in 0.28.1, after amendment 4 to
[ADR-042](../adr/042-group-admins.md); the proofs of concept are regression
tests in `crates/engine/tests/review_2026_11d.rs`. Of the notes: the hook
log, the AI bot's key file and plain-http check, `.gitignore` and the README
links are fixed; a leave stopping the group is written into ADR-042 and the
threat model; the rest stand as written.
Reviewed commit: `b5a49c8` (0.28.0). Date: 2026-11-23.
Earlier passes: [first](review-2026-11.md), [second](review-2026-11b.md),
[third](review-2026-11c.md), and the [ADR-033 review](review-adr-033.md).

**This is not an independent security audit.** It is a source review done by
an AI model (Claude Fable 5.2) at the maintainer's request. A model can miss
whole classes of bugs and cannot vouch for what it did not flag. termchat.app
stays experimental until people who are independent of the project audit it.

## Scope

The third pass looked at 0.16.0. Since then came 19 releases (0.16.1 to
0.28.0, 130 commits, about 20 000 changed lines): the fixes of the third pass
(ADR-030), `watch`, `ask`, `run`, group commands, webhooks and paging
(ADR-031 to ADR-040), safety-number QR codes (ADR-037), receipts (ADR-041)
and several group admins (ADR-042). ADR-042 asked for a separate review
before release; it shipped in 0.27.0 the same day, so this pass is that
review, after the fact, and most of the time went there.

Read in full: `crates/core`, `crates/proto` (without its tests),
`crates/engine/src/group.rs`, `crates/engine/src/receipts.rs`,
`crates/client/src/cli/run.rs`, `crates/client/src/cli/unit.rs`,
`crates/client/src/cli/daemon/approval.rs`, `deploy/ai-bot/termchat-ai-bot`,
ADR-042, `docs/threat-model.md`, `deny.toml`.

Read as the change since `934d427`: `crates/relay/src/lib.rs` (and its
message handler in full), `crates/engine/src/engine.rs`, `devices.rs`,
`data.rs`, `relay.rs`, `crates/client/src/cli/daemon.rs`, the Android
manifest, `MainActivity.kt`, `Model.kt`, the crates' `Cargo.toml` files.

Read in part: `crates/client/src/cli/hook.rs` (all but its tests),
`crates/client/src/cli/daemon/paging.rs` (the first 330 lines),
`crates/client/src/alert.rs` (searched for cleaning and limits), the Android
share sheet (`Chats.kt`, `Chat.kt`), and a search of the changes in
`crates/desktop`, `crates/ffi` and the terminal client for files, processes
and `unsafe`.

Not reviewed: vodozemac, Arti, egui, hyper and every other dependency;
`crates/relay/src/db.rs` and `push.rs` (unchanged since the third pass);
`crates/client/src/cli/daemon/watch.rs`, `daemon/groups.rs`, `cli/group.rs`,
`cli.rs`; the interface code of the three clients line by line; the FFI
bindings; ADR-031 to ADR-041 other than as context; the install, build and
release scripts and workflows (unchanged since the second pass, except the
paths of `pages.yml`); the website; the live relay. No fuzzing was run.
Nothing was tried on an Android device. Duress-password timing was not
measured.

Run on `b5a49c8`: `cargo test --workspace --locked` (all passed),
`cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked
-- -D warnings`, `cargo deny check` (advisories, bans, licenses, sources ok),
`cargo vet --locked` (ok; 143 crates fully audited, 9 partly, 743 exempted).

Findings 1, 2 and 4 were confirmed with tests, kept in
[poc/review-2026-11d.rs](poc/review-2026-11d.rs). Each states the safe
behaviour and fails on `b5a49c8`. To run them, copy the file to
`crates/engine/tests/review_2026_11d.rs` and run
`cargo test -p termchat-engine --test review_2026_11d -- --test-threads=1`.
Findings 3 and 5 and the notes come from reading the code.

## Findings

| # | Severity | Where | Summary |
|---|---|---|---|
| 1 | Medium | `engine` `group_from` (`GroupAdminKeys`), `take_pending_grant` | Someone who gave up the admin role can replace an admin's queue token |
| 2 | Medium | `engine` `roster_lists`; `proto` `Roster::check` | A helper can make the group unchangeable for good, or fill it with entries nobody can remove |
| 3 | Low | `engine` `take_member_devices` | A helper drops members' linked devices and forces the group to move |
| 4 | Low | `core` `Group::add`, `Groups::decrypt` | A member files another member's Megolm session under their own name; the owner's messages are then refused |
| 5 | Low | Android `MainActivity.takeShare`, `readDocument` | A share from another app can point at termchat.app's own private files |
| 6 | Info | various | Smaller notes |

### 1. A former admin replaces an admin's queue token (Medium)

Who: a helper who stepped down, or a main admin who handed the group over,
for as long as their list is the group's last admin list (until the admin
list changes again, which may be never). They need an Olm channel to the
victim: a chat, or the victim's reader.

`GroupAdminKeys` is taken from the main admin, "or who just handed the group
over": `group.admins.last().is_some_and(|l| l.signer == from)`. The same test
is in `take_pending_grant`. Three things are missing:

- the last list's signer may have signed themselves *out* (a helper's
  step-down is signed by that helper), and stays a giver although they hold
  no role;
- "just" has no end: the grant's `roster_no` is only compared with the
  current roster (`no > at` waits), not with the list that made the sender a
  giver, so the right never lapses;
- a token we already hold is overwritten. ADR-042, section 3, also asks that
  `queue` be the group's current queue; the receiver cannot check that (it
  knows the `send_id`, not the admin id), and nothing else stands in.

Confirmed: Bob, a helper, steps down; Alice, the main admin, moves the group
as ADR-042 says. Bob then sends `GroupAdminKeys` with a queue and token of
his choice over their chat. Alice's `group.admin` becomes Bob's pair.

Effect, from reading:

- Alice no longer holds the token of the group's queue, and does not notice.
- Her next invite asks for a reader in the queue Bob named. If Bob made that
  queue on the same relay, the reader is in a queue he owns: he can read (as
  ciphertext) and delete what waits there, and the new member gets no group
  messages, since the group's `send_id` fans out to the real queue only.
- On the next move the "old queue" she retires is Bob's: `RemoveReaderAt` and
  `DeleteGroup` miss the real one. A removed member keeps their reader there
  and the queue lives until the relay expires it, not 24 hours (ADR-042,
  amendment 3). New Megolm sessions still keep them from reading what is sent
  after the move; what stragglers write with the old session they read.
- After a move Alice passes the token on to the helpers.
- The same works for a main admin who handed over, against their successor,
  after the successor's move made their own knowledge of the token worthless.

This is the case ADR-042 lists under A4 ("a forged `GroupAdminKeys` from a
non-admin") and under "what tests must cover". The engine has no test that
sends a forged `GroupAdminKeys` (from a non-admin, before the roster, to a
linked device, for another queue); only `proto` tests where the kind may
travel.

Suggested fix (ADR-042 amendment): take a token only from the main admin in
force, or from the signer of the last list if that list made *us* the main
admin and we hold no token for a roster at or after it; never replace a token
taken for the same or a newer roster.

### 2. A helper makes the group unchangeable (Medium)

Who: a helper. ADR-042 lets a helper invite and nothing else, and promises
that the main admin can always remove a helper.

A helper's roster is accepted when it keeps the name and timer and
`roster.members.starts_with(&current.members)`. Two things are not checked.

**The roster number.** Any `no` above the current one is taken. Confirmed: a
helper's roster with `no = u64::MAX` is accepted by a member and by the main
admin. After that `roster.no <= current.no` refuses every roster from
anyone: nobody can be added or removed, the helper included, the name, timer
and admins are fixed, and a move (which needs a roster) cannot happen, so a
member who leaves can no longer be cut off. On the main admin's device
`roster.no += 1` then overflows: a debug build panics
(`group.rs:2206`, confirmed), a release build wraps to 0 and signs a roster
everyone refuses while its own state runs ahead (confirmed). Admin lists
carry the roster's number and share the problem.

**Doubled keys.** `Roster::check` allows the same `ed25519` twice
(`AdminList` and `DeviceList` refuse that). Confirmed: a helper's roster with
a second entry for the main admin's key, with another reader, is accepted.
Lookups take the first entry, so messages still go right, but
`remove_from_group` refuses the main admin's own key, so such entries cannot
be removed, and at 32 entries the group is full for good. Entries with
made-up keys can be removed one by one, each at the cost of a move.

Suggested fix (ADR-042 amendment, since members who missed rosters must
still catch up): refuse a roster whose number is more than a fixed step
ahead (the chain holds 8 lists; a step of some thousands is ample), use
checked arithmetic where we sign, and refuse a roster that names a key twice,
in `proto` for every roster.

### 3. A helper drops linked devices and forces moves (Low)

From reading. Since ADR-042 `take_member_devices` takes `GroupDevices` from
any admin ("any admin vouches for the readers of those they invited"), for
any member. The member's list must be signed by the member and not older,
but the same list may come again with other `readers`.

A helper sends `GroupDevices { member: Carol, list: <Carol's current list>,
readers: [] }`. Every member computes `gone` = Carol's linked devices,
forgets their Megolm sessions and stops writing until a move
(`wait_for_group_move`); the main admin starts that move without those
devices. The same works against the main admin's own linked devices.

- For a member who is the main admin's contact the devices come back after
  the move (`group_devices_changed`), so the helper can repeat it: each time
  a new queue, new readers and new sessions for everyone.
- For a member only the helper knows, the main admin has no device list and
  the devices stay out.

ADR-042's table gives a helper no way to remove anyone. Reader ids a helper
makes up do no more harm: a key claimed there must be signed by the device.

Suggested fix: take `GroupDevices` for member M only from the main admin or
from the admin whose roster first named M; or let a helper's word add
devices, never drop them.

### 4. A Megolm session filed under whoever brings its key first (Low)

Who: any group member, against one other member's view of a third.

`Group::add` returns `Ok` without adding when a session with the same id is
already there, whoever it is filed under, and `Groups::decrypt` finds a
session by its id alone. Every member holds every other member's session
key. Confirmed: Mallory sends Victor the key Alice gave her, as her own
`GroupKey`, before Alice's own reaches him (a new member, or right after
Alice rotates). The session is filed under Mallory; Alice's key is then
ignored; Alice's messages decrypt and are refused with `SenderMismatch`.

- Victor reads nothing from Alice until her next session (100 messages or 7
  days), and sees no "messages missing" for it.
- Mallory cannot write as Alice: the plaintext names its sender, and Megolm
  messages are signed by a key only Alice holds.
- It needs a race against Alice's own key, which the relay can help win.

Suggested fix (no new cryptography): keep sessions per (member, id), and on
decrypt take the one whose member is the sender the plaintext names.

### 5. Android: a share can name the app's own files (Low)

From reading; not tried on a device. `MainActivity` now takes
`ACTION_SEND` with any type from any app. `takeShare` passes `EXTRA_STREAM`
to `readDocument`, which opens it with `contentResolver.openInputStream`
under termchat.app's own identity, before the app is unlocked, and keeps up
to 1 MiB in memory.

Another app can send a `file://` URI of a file in termchat.app's private
folder (the encrypted state file, the plain settings, Arti's folder with the
bridge lines). The dialog then shows "file (N KB)" and "It goes as soon as
you pick a chat", and one tap sends it to that chat.

- It needs the user's tap, and the file goes to a contact or group the user
  picks, so the attacker must be one of them or share one.
- The state file is encrypted with the password (Argon2id); the others are
  not secret by the threat model. So what leaks is material for offline
  password guessing, and the bridge lines.

Suggested fix: take only `content://` URIs, and refuse one whose authority
is the app's own or whose path resolves under the app's data folder.

### 6. Smaller notes (Info)

- **A leave pauses the group.** After any member's `GroupLeave` every other
  member stops writing until the main admin's move arrives. A member can
  send it and stay, and an honest leave does the same while the main admin
  is offline. If the main admin's device is lost, the first leave ends
  writing for good; ADR-042, section 4, says only that such a group "can no
  longer change". Say so in the ADR and the threat model, or put a time
  limit on the wait.
- **Names inside system lines** (third pass, item 10; second pass, finding
  7): still there, and ADR-042 adds lines ("… made … an admin", "… handed
  the group over to …"). A helper now chooses the nickname of everyone they
  add. Control and bidi characters are stripped; the text can still imitate
  the app.
- **`hook`** answers `401` before anything else and compares the token in
  constant time; the body, the rate and the connection count are bounded.
  It writes one log line per request before the token is checked (the path,
  64 characters at most), and its 16 connection slots can be held for 10
  seconds each without a token: by local users, or by the network if it
  listens off loopback.
- **`termchat-ai-bot`**: the docs call for a `0600` key file, the script
  does not check the mode (the client does for its own files). With a remote
  `AI_URL` over `http://` the key and the messages go in the clear.
- **A peer's relay address** may be any `wss://` host, private ranges
  included: a contact can make the client open a TLS connection to a host
  inside the user's network. Nothing comes back to the contact.
- **Helper programs** (`afplay`, `osascript`, `notify-send`, `powershell`)
  are found through `PATH`, with fixed arguments.
- **`termchat-relay.db`, `-shm`, `-wal`** lie in the repository root and
  `.gitignore` does not name them: a local relay's database could be
  committed by mistake.
- **`cargo vet` passes with 743 of 895 crates exempted.** It guards against
  a changed dependency, not against the ones never looked at.
- **RUSTSEC-2026-0459** (SQLite) is still ignored: Arti 0.48 still requires
  `rusqlite` below 0.41.
- The README links the first two passes only.
- Notes of the earlier passes that are unchanged: prekeys carry no type or
  date; `check_nickname` and `check_label` allow bidi characters;
  `KeyCount` from the relay is not rate-limited; `MAX_UNSAVED` counts at a
  file's first chunk.

## Fixes of the earlier passes, checked

Still in the code and covered by tests that pass:

- First and second pass: ADR-019 in `Client::decrypt` and `remember` (counted
  under the Curve25519 key), the 64 KiB frame and message limits on both
  sides, queue pages, `safe_name`, `write_atomic`, the onion-only-through-Tor
  rule, the journal cap.
- Third pass: `member` contacts never count on an invite queue (finding 1),
  device proofs in `verify_devices`, a list naming someone else's key is
  refused whole (2, 4), `peer_addr_ok` and links that skip an address they
  cannot dial (3, 7), `Copy` names its chat (4), `take_member_devices`
  refuses another member's key (5), group gaps are shown and the Megolm
  replay floor holds (6, 10), `decrypt_on_reader` (8), reports per queue and
  the fair share (9). The regression tests of
  `crates/engine/tests/review_2026_11c.rs` pass.
- Finding 3 above is next to the third pass's finding 5: the fix there stops
  a member; ADR-042 then gave the same lever to helpers.

## Checked, no problem found

- `check_admin_change`: the signer must be an admin of the list before, the
  signature covers group, number, signer, admins and resign; only the main
  admin adds, removes or reorders; a helper only removes themselves; a
  takeover needs the main admin's resign for the roster right before, with
  its own signing context. Contexts for rosters, admin lists, resigns,
  devices and device proofs differ.
- `roster_lists`: a helper's roster cannot rename, set the timer, drop or
  change an existing member; a chain over missed rosters is taken only with
  the main admin's roster; every admin in force must be a member.
- `GroupMove` and a removal (`GroupLeave` one to one) are taken only from the
  main admin in force, never from a linked device; an invite to a group we
  are in is checked as a move.
- `GroupAdminKeys` is refused by `proto` inside a group and inside a `Copy`;
  `AdminGrant` and `ReaderGrant` hide their token in `Debug`; only a primary
  device takes a token.
- Relay: `RemoveReaderAt` looks for the reader in the named group only, with
  the admin token, and answers like an unknown queue otherwise.
- Receipts: never to a request, a blocked or older contact, never in groups
  or copies; ids are bounded; marks only go forward; a receipt names only
  our own messages.
- `ask` and `page`: the answer must be the original text from the verified
  contact's primary device on its direct queue, with a 64-bit code; the
  contact is checked again when the answer comes.
- `run` reports the program's file name and cleaned output lines, bounded;
  `systemd-unit` refuses names and paths that would break out of the unit.
- Safety-number QR: only an exact match verifies; a mismatch changes nothing.
- No `unsafe` in any crate; no new exported Android component besides the
  share filter of finding 5.
