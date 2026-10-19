# ADR-015: Tor bridges and pluggable transports

Status: **accepted** (maintainer, 2026-10-19). Amends ADR-004. Русская версия: [015-tor-bridges.ru.md](015-tor-bridges.ru.md).

## Context

termchat reaches the relay's onion service through Arti (ADR-004). Where Tor
itself is blocked, including much of Russia since 2024, Arti cannot even
bootstrap, and "Tor on" means "no connection at all". Tor's answer is
**bridges**: unlisted entry relays, reached through a **pluggable transport**
that disguises the traffic:
- **obfs4** looks like random bytes;
- **webtunnel** looks like an ordinary HTTPS site;
- **snowflake** runs over WebRTC through volunteer browser proxies.

Facts that shape the decision (checked 2026-10-19):
- **Arti supports bridges and managed transports.** These are the
  `arti-client` features `bridge-client` and `pt-client`. Arti starts the
  transport program itself and talks to it over the PT protocol.
- **The transports are Go programs.** lyrebird, the successor of
  obfs4proxy, carries obfs4, webtunnel, meek and snowflake in one binary.
  There is no production Rust implementation; `lyrebird-rs` is a community
  port and incomplete.
- **Arti's bridge support had trouble in Russia in 2026.**
  - After new blocks in June 2026, Arti 2.4.0 could not build circuits
    through snowflake or obfs4 in Russia, while C tor could with the same
    bridges.
  - By October 2026 Arti 2.7.0 works on Linux but hangs on Windows (Arti
    issues #2678, #2725, #2726).
  - Source: Tor Project forum, "[Arti 2.4.0] Unable to connect with
    pluggable transport (RU)".

## Decision

### 1. Bridges in the settings
- A new setting holds **bridge lines**, exactly as Tor prints them, for
  example `obfs4 1.2.3.4:443 FINGERPRINT cert=… iat-mode=0`.
- The user gets them from bridges.torproject.org, from the Telegram bot
  @GetBridgesBot, or from someone they trust. termchat ships no bridges and
  fetches none: a built-in list is the first thing a censor blocks, and
  fetching would contact a third party.
- With bridge lines set, Arti uses only those bridges as entry points. With
  none set, nothing changes.
- termchat checks each line's format before saving it and shows the
  transport name. The secrets inside the line (`cert=`) never go to logs.
- Bridge lines live in the encrypted state file. The duress wipe, WIPE and
  MOVED erase them with everything else. Backups carry them.

### 2. Where the transport program comes from
- **Desktop.**
  - Release archives, installers and the Homebrew formula ship lyrebird next
    to `termchat`, built from source in the same CI job as everything else.
    Its hash goes into `SHA256SUMS`, and it gets build attestations.
  - Packages for it are not yet usable: Homebrew has only the older
    `obfs4proxy` (obfs4 only), and Debian has lyrebird only in unstable.
  - termchat looks for it next to its own executable first, then on `PATH`.
    `obfs4proxy` is accepted for obfs4 lines. A setting can point to another
    file.
  - termchat never downloads a program at run time.
- **Android.**
  - The app ships lyrebird, built from source with Go, as a native
    executable (`liblyrebird.so` in the APK's native libraries folder, the
    only place Android lets an app run its own programs). Arti starts it
    from there.
  - `build-rust.sh` builds it with `-trimpath -buildvcs=false` and pinned
    module hashes, so the reproducible-build check (CI and F-Droid) still
    matches.
  - The F-Droid recipe installs the Go toolchain and builds lyrebird from
    its tagged source; nothing prebuilt goes in.
- **The Arti version** moves to the newest release with working transports
  before this ships. The release notes say plainly which platforms were
  tested through bridges, and that bridges may not work everywhere.

### 3. Which transports
- **obfs4 and webtunnel** first: plain bridge lines, one program.
- **snowflake** is included because lyrebird carries it, but the settings
  say it may be slow. It depends on the broker being reachable, which
  censors attack.
- **meek** is left out: it needs a CDN willing to front it, and the
  remaining ones are expensive and slow.

## What changes for metadata and safety

- **The network** sees the chosen transport instead of a Tor connection.
  It does not see Tor, though a censor with enough effort can sometimes
  detect obfs4 or snowflake.
- **The bridge operator** sees the user's IP address and that they use Tor,
  as a Tor guard does. It does not see the relay or the messages.
- **lyrebird is new code inside the app.**
  - It is maintained by the Tor Project.
  - It runs as a separate process with the app's own permissions, and only
    while bridges are on.
  - Its version is pinned and goes through the same dependency review as
    Rust crates: noted in the commit, checked against the Tor Project's
    signed tags.
- **Size.** About 10 MB more in the APK and in each desktop archive
  (estimate, to be measured).

## At implementation (2026-10-19)

- **Source and signature.** lyrebird 0.8.1 is a git submodule in
  `third_party/lyrebird`, at commit `0b10edbb`. Its tag is signed by
  Shelikhoo <shelikhoo@torproject.org>, checked with the key that the Tor
  Project's GitLab publishes for that account.
- **Build.** `scripts/build-lyrebird.sh` uses Go `go1.27.2` (pinned with
  `GOTOOLCHAIN`), `-trimpath`, no VCS stamp or build id. Two builds gave the
  same bytes.
  - On Android it links against bionic (cgo, with the NDK's clang). A static
    Go binary finds no DNS server there, and webtunnel and snowflake need one.
  - `-checklinkname=0` is needed for snowflake's `anet` dependency with
    Go 1.23 and later.
- **Size.** About 17 MB uncompressed per platform.
- **Arti stores bridge lines in the clear.** Arti keeps the lines it uses,
  `cert=` included, in its own folder: the directory cache and the guard
  state. termchat cannot encrypt that folder without changing Arti. So:
  - changing the bridge list stops Tor and moves to a new folder (`arti/g1`,
    `g2`, …), and the old one is erased. Reusing one folder while the old
    client was shutting down stalled the new one in testing;
  - WIPE, MOVED and the duress password erase it, as before;
  - while bridges are on, anyone who reads the device's files without the
    password can see them. This was found by the security review, and the
    threat model says it.
- **Tested on 2026-10-19 from outside Russia.** Three obfs4 bridges from Tor
  Browser's built-in list reached the public relay's onion service in about
  2.5 minutes. Only lyrebird connected out, and only to the bridges.
  Russia itself is not tested.

## Rejected

- **A built-in bridge list:** it is blocked first, and it would go stale.
- **Fetching bridges automatically** (Moat, as Tor Browser does): a request
  to a third party from inside the app, for every user. It is worth a
  separate decision later, if asked for.
- **IPtProxy (gomobile library):** it runs the transports inside the app
  process through JNI and needs Arti's unmanaged-transport mode. A separate
  process is simpler and easier to audit.
- **A Rust transport (lyrebird-rs):** not complete, not reviewed.
- **Falling back to a direct connection when bridges fail:** never, as in
  ADR-004. The IP must not leak.

## Dependencies

- `arti-client` features `bridge-client` and `pt-client`, which pull in
  `tor-ptmgr`. They are in the same tree, from the Tor Project.
- **Android:** lyrebird (Go; BSD-3-Clause, some parts GPL-3.0, which is
  compatible with termchat's AGPL) and the Go toolchain at build time.
- **Desktop:** lyrebird in the release archives, built from source in CI.
