# AI-assisted security review, November 2026 (second pass)

Status: findings 1 to 7 fixed, 8 partly fixed, 9 left as it is (see "Fixes"
below).
Reviewed commit: `d49d78f` (0.6.1). Date: 2026-11-14.
The first pass: [review-2026-11.md](review-2026-11.md).

**This is not an independent security audit.** It is a source review done by
an AI model (Claude Fable 5.2) at the maintainer's request. A model can miss
whole classes of bugs and cannot vouch for what it did not flag. termchat.app
stays experimental until people who are independent of the project audit it.

## Scope

Read in full: `crates/core`, `crates/proto`, `crates/engine`, `crates/relay`,
`crates/ffi`, the Android manifest, `Model.kt`, `MainActivity.kt`, the
Android services, CI, the install and build scripts, `Dockerfile`, `deploy/`.

Read in part (searched for untrusted-text rendering, passwords, file
handling): `crates/client`, `crates/desktop`, the rest of the Kotlin sources.

Run: `cargo test --workspace --locked`, `cargo deny check advisories bans
sources`, `cargo vet --locked`; all passed.

Not reviewed: vodozemac, Arti, egui and every other dependency; lyrebird;
the website; the live relay. No fuzzing was run. Duress-password timing was
not measured.

Findings 1 and 3 were confirmed with temporary tests. The others come from
reading the code.

## Findings

| # | Severity | Where | Summary |
|---|---|---|---|
| 1 | Medium (relay availability) | `relay` `upgrade` | A WebSocket frame of up to 16 MiB is buffered, though messages are capped at 64 KiB |
| 2 | Medium (physical access) | Android `Model.kt`, `MainActivity.kt` | The idle lock can be skipped on coming back to the app |
| 3 | Low | `core` `Client::remember` | The relay can push out ADR-019 entries and deliver an old contact request again |
| 4 | Low | `engine` `relay_url_ok`, `Relay::connect_after` | `ws://….onion` is allowed with Tor off |
| 5 | Low | terminal and desktop clients | The idle timer stops while the computer sleeps |
| 6 | Low | `relay` `handshake`, report journal, delivery channel | Three remaining load vectors |
| 7 | Low | `engine` `receive` | A stranger's text ends up inside a system line |
| 8 | Info | `deploy/docker-compose.yml`, `deny.toml` | Open ends of the first pass |
| 9 | Info | `engine` `on_frame` | `KeyCount` from the relay is not rate-limited |

### 1. The relay buffers frames of up to 16 MiB (Medium, availability)

`upgrade` set only `max_message_size(MAX_FRAME_BYTES)`. `max_frame_size`
stayed at its default of 16 MiB, and tungstenite reads a whole frame before
it checks the message size.

- A test relay took a single 15 MiB frame without complaint and kept the
  connection; a 17 MiB frame was refused at once, by its header.
- After login an unfinished frame lives until `idle_timeout` (an hour since
  0.6.1), because `last_seen` changes only on a complete message.
- 50 connections per IP times 16 MiB is about 800 MiB, for the same amount
  sent once. It needs 50 keys (20 registrations per hour per IP). Through the
  onion port the cap is 2000 connections.
- Before login the window is 10 seconds.

### 2. Skipping the Android idle lock (Medium, physical access)

`lockIfIdle()` ran only from a `Handler.postDelayed` every 10 seconds. That
timer counts uptime and stands still while the phone sleeps or the process
is frozen. `onStart` did not check.

- After hours idle the app opened unlocked, and the chat list showed for up
  to 10 seconds.
- Any touch in that time called `touch()`, reset `lastInput`, and the lock
  never came.
- While the process is frozen in the background, the keys stay in memory
  longer than the chosen time.

### 3. Pushing out the `seen` list (ADR-019) (Low)

`remember(id, *sender)` filed the session under the key the relay named, and
did so before `open_inner` checked it. The cap is 200 entries per sender.

- Test: Bob removed Alice, and a replay of her request was refused. After 200
  pre-key messages from another key labelled "from Alice" (each refused with
  `SenderMismatch`, but remembered), the replay was accepted.
- Only the relay can do this, and only while the fallback key lives. For a
  current contact the Curve25519 check catches the false label.
- Effect: a removed contact comes back as a request, if their code is public
  or `codes_only` is off. The one-time code is already spent.

### 4. `ws://….onion` without Tor (Low)

`relay_url_ok` and `connect_after` allowed plain `ws://` to an `.onion` host
without looking at `Dial`. With an onion address in the relay field and Tor
off (or in a build without Tor), the client resolved `.onion` through
ordinary DNS and connected in the clear.

- A network that forges the DNS answer sees the keys and metadata and stands
  in for the relay. The login signature is bound to the same host name, so it
  can be passed on to the real relay.
- Content stays end-to-end encrypted.

### 5. The idle timer of the terminal and desktop clients (Low)

`last_input` was an `Instant`, which does not move while macOS or Linux
sleeps. After 4 minutes idle and a night with the lid closed, the app still
had a minute left, and any key extended it. Only the system's own screen
lock helped.

### 6. Remaining load on the relay (Low)

- Logins of an existing key are not rate-limited. Each costs a signature
  check and a database write under the shared lock; only new keys and 50
  simultaneous connections are limited.
- The report journal has no size cap. A pair of one's own keys that wrote to
  each other 3 envelopes each can write 10 reports a day per key, up to 16 KB
  each.
- The live delivery channel holds up to 256 frames (about 5 MiB) per slow
  reader. This is what is left of finding 3 of the first pass.

### 7. A request note inside a system line (Low)

The `note` of a `ContactRequest` (up to 256 bytes) went into the line
"contact request: …" with status `System`. termchat.app always sends an empty
note, so only a modified client fills it. Control and bidi characters were
stripped, but the text looked like the app speaking. The desktop and Android
apps kept line breaks; the terminal did not.

### 8. Open ends of the first pass (Info)

- `caddy:2` in `docker-compose.yml` is still not pinned by digest.
- RUSTSEC-2026-0459 (SQLite) is still in `ignore`: Arti 0.47 does not allow
  `rusqlite` 0.41.
- The notes of item 10 are unchanged: prekeys carry no type or date, and
  `Unregister` resets limits and the report count.

### 9. `KeyCount` is not rate-limited (Info)

Each `KeyCount { one_time_left: 0 }` makes the client generate and sign 50
keys and rewrite its state file. The relay can deny service anyway, but this
is a cheap way to load a phone.

## Fixes of the first pass, checked

Items 1, 2, 3, 5, 6 and 9 are in the code and work as described; item 1 had
the gap of finding 3 above. Item 4 is closed by a semaphore of 16 wake-ups in
flight, without a per-host cap, as recorded.

## Fixes

Made on the day of the review, and not looked at by anyone else yet.

| # | State | What changed |
|---|---|---|
| 1 | fixed | The relay sets `max_frame_size` to the same 64 KiB; a larger frame is refused by its header |
| 2 | fixed | The idle time is checked on `onStart`, before anything shows, and a touch after the time is up locks first. Without "stay connected" an alarm wakes the frozen app and locks it when the time is up ([ADR-022](../adr/022-review-hardening.md)); the alarm is inexact and may come some minutes late |
| 3 | fixed | [ADR-019](../adr/019-prekey-replay.md), amended: entries are counted under the Curve25519 key Olm vouches for. The 10 000 cap overall can still be filled by whoever reaches the fallback key |
| 4 | fixed | [ADR-004](../adr/004-tor.md), amended: an `.onion` host only through Tor; the relay field refuses onion addresses |
| 5 | fixed | Idle time counts on both the monotonic and the wall clock, the longer one wins; input after the time is up locks instead of resetting it |
| 6 | fixed | ADR-022: logins limited to 30 at once, then one a minute per key; the report journal stops at 10 MiB (reports still count); the delivery channel holds 32 frames |
| 7 | fixed | The request note is not shown |
| 8 | partly | `caddy` pinned by digest. The SQLite advisory stays ignored until Arti allows `rusqlite` 0.41; the notes of item 10 are unchanged |
| 9 | not changed | Only the relay can send `KeyCount`, and the relay can deny service anyway |
