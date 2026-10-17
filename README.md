<p align="center"><img src="docs/logo/termchat-mark-dark.svg" alt="termchat logo" width="128"></p>

# termchat

По-русски: [README.ru.md](README.ru.md).

A private, text-only messenger for the terminal. End-to-end encrypted with
[vodozemac](https://github.com/matrix-org/vodozemac) (Olm: X3DH-style handshake and
Double Ratchet, the design behind Signal and Matrix). Optional Tor. No accounts,
phone numbers, emails or names: you are a key pair and a nickname you choose.
Website: [termchat.app](https://termchat.app).

**Status: not audited.** The cryptography comes from vodozemac, which was audited in
2022; termchat's own code has had no external review. Treat it as experimental.

termchat is a free, non-commercial, open-source project created and maintained as a
personal hobby. It has no advertising, subscriptions, paid features or commercial
monetization. Anyone can run their own relay server.

## What it protects, and what it does not

The goal is not "impossible to break" but "breaking the server is pointless".

**Protected**
- Message contents, from your provider, Wi-Fi, the relay operator and anyone who
  seizes the relay: it only ever holds ciphertext.
- Old messages if a key leaks later (forward secrecy), and new ones once the attacker
  loses access (post-compromise security).
- Tampering, replays and reordering; dropped messages are detected and shown.
- Impersonation: compare the safety number with your contact once (Ctrl+F).
- Your IP address, from the relay and your provider, when Tor is on.
- Your data at rest: everything on disk is encrypted with your password (Argon2id).

**Partly protected**
- **Metadata.** The relay sees which key writes to which and when, and message size
  rounded up to 512 / 4096 / 16384 bytes. Nicknames are never visible to it.
- **Someone with your unlocked computer.** Auto-lock after 5 minutes, Ctrl+K to lock
  now, disappearing messages, chats without history.
- **Being forced to give a password.** An optional duress password silently erases
  everything. It does not help if the disk was copied first, the code is public, and
  destroying data after a seizure can be a crime where you live.
- **A lost device.** Only with a backup of your identity (Ctrl+P, b).

**Not protected**
- Malware on your device, a keylogger, a compromised terminal or OS.
- A global observer correlating traffic timing (Tor does not stop that).
- Keys in swap or a hibernation file: use your OS's disk encryption.
- On Windows: no core-dump protection and no `0600` file permissions (Unix only);
  the state file is still encrypted with your password.

Full details: [docs/threat-model.md](docs/threat-model.md).

## Install

Download the archive for your system from the
[releases](https://github.com/prophecy1345/termchat/releases). Each holds `termchat`
(the client) and `termchat-relay` (the server). The quickest ways to install
(Homebrew, one command) are in the [user guide](#1-install).

### Verify a release

1. **Checksum:** `sha256sum -c SHA256SUMS --ignore-missing` (macOS:
   `shasum -a 256 -c SHA256SUMS --ignore-missing`).
2. **Provenance:** `gh attestation verify termchat-<version>-<target>.tar.gz --repo prophecy1345/termchat`
   proves the archive was built by this repository's release workflow from a
   specific commit (Sigstore, no keys to trust).
3. **Rebuild it yourself:** binaries are reproducible.

   ```sh
   git clone https://github.com/prophecy1345/termchat && cd termchat
   git checkout v<version>
   scripts/build-release.sh <target>        # Linux musl targets: BUILD=zigbuild
   shasum -a 256 target/<target>/release/termchat target/<target>/release/termchat-relay
   ```

   The hashes must match `BINARIES.sha256` inside the archive.
4. **Android APK:** steps 1 and 2 work for the `.apk` too. Its signing
   certificate must be this one (`apksigner verify --print-certs <apk>`, or the
   app's details in Android): SHA-256
   `BC:6A:74:2E:73:CF:B7:55:CD:24:6B:C0:F2:73:DF:FF:71:8A:78:77:7D:4C:CB:41:9A:8D:D4:A6:4A:76:BA:5E`.

Dependencies are checked with `cargo deny` (licences, known vulnerabilities) and
`cargo vet` (imported audits from Mozilla, Google and others; the rest is listed as
not yet reviewed in `supply-chain/`).

## User guide

termchat runs in a terminal window. Use the keyboard or the mouse; press **?** (or
**F1**) at any time for the list of keys, and **Ctrl+P** for the menu, which also
holds this guide.

По-русски: [README.ru.md](README.ru.md).

### 1. Install

Pick one way.

**Homebrew** (macOS, Linux):
```sh
brew install prophecy1345/tap/termchat
```
Update with `brew update && brew upgrade termchat` (without `brew update` Homebrew may not see the new version for up to a day).

**One command** (macOS, Linux): downloads the right build, checks its checksum and puts
`termchat` into `~/.local/bin`.
```sh
curl -fsSL https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.sh | sh
```

**One command** (Windows, in PowerShell): installs to
`%LOCALAPPDATA%\Programs\termchat` and adds it to your PATH.
```powershell
irm https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.ps1 | iex
```

Then open a new terminal and run `termchat`.

**Android** (8.0 and later, no Google services needed): download
`termchat-<version>-android-arm64.apk` from the
[releases](https://github.com/prophecy1345/termchat/releases) and open it;
Android asks once to allow installs from your browser. The app shows a notice
in the chat list when a newer version is out. An F-Droid build is on its way;
APKs from F-Droid and from GitHub are signed with different keys, so stay with
one source.

**By hand.** Download the archive for your computer from the
[releases](https://github.com/prophecy1345/termchat/releases):

| Your computer | Archive |
|---|---|
| Mac with Apple Silicon (M1 and later) | `termchat-<version>-aarch64-apple-darwin.tar.gz` |
| Mac with Intel | `termchat-<version>-x86_64-apple-darwin.tar.gz` |
| Linux, 64-bit PC | `termchat-<version>-x86_64-unknown-linux-musl.tar.gz` |
| Linux on ARM (Raspberry Pi 4/5, ARM servers) | `termchat-<version>-aarch64-unknown-linux-musl.tar.gz` |
| Windows 10/11 | `termchat-<version>-x86_64-pc-windows-msvc.zip` |

- **Windows:** open the `.zip`, extract it, double-click `termchat.exe` (Windows
  Terminal looks best). SmartScreen may warn about an unknown publisher: "More info" →
  "Run anyway".
- **macOS:** `tar -xzf termchat-*-apple-darwin.tar.gz`, then in that folder
  `xattr -d com.apple.quarantine termchat` (the binary is not signed by Apple; without
  this macOS says it "cannot be opened because the developer cannot be verified") and
  `./termchat`. The two ways above do not need this step.
- **Linux:** `tar -xzf termchat-*-linux-musl.tar.gz`, then `./termchat` in that folder.
  The binary is static: no libraries to install.

`termchat --version` prints the installed version. Before the first run you can check
that the download is genuine; see [Verify a release](#verify-a-release).

<!-- manual:start: shown inside termchat (menu, manual) -->
### 2. First run

The setup screen has six fields. **Tab** or **Enter** moves to the next one,
**Shift+Tab** back.

| Field | What to enter |
|---|---|
| Relay URL | Already filled in with the [public relay](#public-relay). Leave it, or type the address of another relay. |
| Relay onion URL | Already filled in. Needed only for Tor. |
| Use Tor? | `y` hides your IP address from the relay and your internet provider; the first connection takes 10–30 seconds. `n` connects directly and is faster. |
| Nickname | What your contacts will see. The only thing anyone learns about you; no real name needed. Up to 64 bytes. |
| Password | Encrypts everything on your disk. Must be strong; **Ctrl+G** generates six random words. Write it down: it cannot be recovered. |
| Repeat password | The same again. |

After setup termchat suggests making a backup right away (see
[Backup](#8-backup-and-restore)). Until you do, the status bar shows
`! no backup: Ctrl+P`.

### 3. The screen

```
 contacts                 │ alice·36335   ! not verified (Ctrl+F)
                          │
 ! alice·36335            │ alice·36335  2m
 ● bob              2     │ hi!
 ? carol·58121            │
                          │ you  now
                          │ hello ✓
                          │
                          │ › type here, Enter sends
 + add             ≡ menu │
 ● online  you  ! no backup: Ctrl+P         ^P menu  ? keys  ^K lock  ^Q quit
```

- **Left:** contacts, the most recent chat on top. Click one, or use **Up/Down**.
  The number on the right counts unread messages. Drag the line between the list
  and the chat to make the list wider or narrower. In a narrow window the list
  hides; Up/Down still switch chats.
- **Right:** the chat with the chosen contact. Messages are grouped under the
  sender's name and their age (`now`, `2m`, `3h`, `4d`). **PgUp/PgDn** or the mouse
  wheel scrolls back; `↓ PgDn` shows that you are not at the newest message.
- **Bottom:** where you type. **Enter** sends. Long text wraps. **←/→**, **Home/End**,
  **Alt+←/→** (by word), **Ctrl+W** (delete a word) and **Ctrl+U** (delete to the
  start) edit it.
- **Status bar:** `● online`, `● online via Tor`, or `○ connecting…`; your nickname;
  warnings. The hints on the right can be clicked.
- **Right-click** a contact for everything you can do with it (accept, rename,
  verify, disappearing messages, block, report, delete).
- **Menu** (**Ctrl+P** or `≡ menu`): settings, keys, this manual, your invite, add
  contact, backup.

Marks next to contacts:

| Mark | Meaning |
|---|---|
| `●` | Unread messages |
| `?` | A contact request you have not accepted yet |
| `!` | Safety number not verified yet |
| `×` | Blocked |
| `alice·36335` | Name plus the start of the fingerprint: shown for unverified contacts and whenever two names look the same. Names are chosen by the people themselves, so the digits are what tells two "alice"s apart. |

Marks next to your messages: `…` sending, `✓` accepted by the relay, `✗` failed
(the reason is shown next to it).

With the mouse on, the terminal's own text selection needs **Shift** held while
dragging (Option on macOS in some terminals), or turn the mouse off in settings.

### 4. Adding a contact

There is no search and no directory: you add people by exchanging **invites**.

1. **Ctrl+O** shows your invite: a long line starting with `termchat:`. Press **c**
   to copy it (iTerm2, kitty, WezTerm, Windows Terminal and others with OSC 52; not
   macOS Terminal), or select it with the mouse. It
   contains only public keys, so it is safe to send by email, another messenger or a
   QR code.
2. Your friend presses **Ctrl+A**, pastes the invite and presses **Enter**.
3. You now appear in each other's lists. On your side the friend arrives as a
   **request** (`?`). Press **Ctrl+Y** to accept, or **Ctrl+D** then **y** to reject.
4. Until you accept, their messages stay hidden (the chat title shows how many are
   waiting; up to 5 are kept and shown when you accept). This keeps strangers from
   filling your screen.

Both sides can add each other's invites; then neither is a request.

### 5. Verify the safety number (once per contact)

The relay cannot read messages, but someone who controls the network could try to
hand you their keys instead of your friend's when you first add them. Rule this out
once:

1. **Ctrl+F** shows a 60-digit safety number. Your friend opens the same screen.
2. Compare the numbers in person, on a video call or by phone (not over termchat).
3. If they match, press **v**. The `!` and the digits after the name disappear.

If the numbers differ, do not trust the contact: delete it (**Ctrl+D**) and exchange
invites again by another way.

### 6. Messages

- Type and press **Enter**. Text only, up to 16 000 bytes per message (about 8 000
  Cyrillic or 16 000 Latin characters). Pasting is safe: pasted text never acts as
  key presses.
- **Offline delivery:** if your friend is offline, the relay keeps the encrypted
  message for up to 14 days and delivers it when they come back.
- **"⚠ N message(s) missing"** means something sent to you did not arrive (lost, or
  dropped by the relay). It disappears if the message turns up later.
- **"⚠ they restarted the conversation"** means your friend restored from a backup or
  reinstalled. Messages sent around that moment may be lost; nothing else to do.

### 7. Chat options

All of these act on the chosen contact; right-clicking it offers the same list:

| Key | What it does |
|---|---|
| **Ctrl+T** | Disappearing messages: off → 5 min → 1 h → 1 day → 1 week. Applies to what you send from now on; the message disappears on both sides when the time is up. The chat title shows `⏱`. |
| **Ctrl+E** | No history: this chat is never written to disk. It is gone when you lock or quit. |
| **Ctrl+R** | Rename the contact. Only on your device; empty name resets it. |
| **Ctrl+B** | Block / unblock. Their messages are thrown away unread; you cannot write to them. |
| **Ctrl+D** | Delete the contact and the chat (asks for **y**). Deleting a request rejects it for good; adding that person's invite later takes the rejection back. |
| **Ctrl+X** | Report to the relay operator. Shows exactly which messages (their last 5) the operator will read; **y** sends the report and blocks the contact. A key reported by several different people it really wrote to is banned on that relay. |

### 8. Backup and restore

Your identity (keys) exists only on your device. **Without a backup, a lost or broken
device means a lost identity** and your contacts must add you again.

- **Make a backup:** **Ctrl+P**, then **b** (or settings → security). Type where to save it
  (a USB stick, for example) and a backup password. The backup holds your identity and
  contact list, not your messages. **Move it off this device** (USB stick, another
  computer).
- termchat remembers where it wrote a backup, or restored one from. Any wipe,
  the duress password included, erases a backup that is still at that path,
  so one left on this disk does not give you away. A copy moved or copied
  elsewhere is not touched; that copy is how you get your identity back.
- **Restore:** on the first-run screen of a new device press **Ctrl+B**, enter the path
  to the backup, then unlock with the backup's password. Chats start fresh; contacts
  see "they restarted the conversation".
- Use **one device at a time**: both devices would fight over the same key.

**Moving to another device** (say, from a Mac to a Linux laptop):

1. On the old device, make a **fresh** backup (**Ctrl+P**, **b**): an older one
   lacks the contacts you added since.
2. Copy the backup to the new device and quit termchat on the old one (**Ctrl+Q**).
3. Install termchat on the new device. On the first-run screen press **Ctrl+B**,
   enter the path to the backup and unlock with the backup's password. Change it
   later in settings → security if you like.
4. Check that your contacts are there and send each a message. Their next
   message starts a new conversation with this device; anything sent during the
   move may be lost.
5. On the old device: settings → security → wipe, and type **`MOVED`**, not
   `WIPE`. `MOVED` erases only this device and tells nobody. `WIPE` would delete
   your account: contacts would see that you left, on the new device too.
6. Delete the backup file wherever you no longer need it, or keep it somewhere
   safe as your next backup.

### 9. Locking, passwords and wiping

- termchat **locks itself after 5 minutes** without a key press or click; **Ctrl+K** locks at
  once. Locking removes keys and messages from memory; unlock with your password.

The rest is in **settings**: open them with **Ctrl+P**, then **s**, and go to the
**security** tab (**Tab** or a click):

- **Change password:** current password, new one twice.
- **Duress password:** a second password. Typing it at the unlock
  screen silently erases everything and shows the first-run screen, as if termchat
  had never been used. Read the warning on that screen first: it does not help if
  someone copied your disk beforehand, and destroying data after a seizure can be a
  crime in some countries. Leave the new field empty to turn it off.
- **Wipe everything:** type `WIPE`. Keys, contacts and history are
  erased from this device. There is no undo. Your contacts get a goodbye
  (their chat with you becomes read-only) and the relay forgets your key.
  Without a connection the client asks whether to wipe without telling them.
  The duress password never does this: it wipes silently.
  Moved to another device? Type `MOVED` instead: it erases only this device,
  silently (see [Moving to another device](#8-backup-and-restore)).
- **Tor on/off later:** the `Tor` row.

### 10. Where data lives

Everything is in one encrypted file, `~/.termchat/state` (Windows:
`C:\Users\<you>\.termchat\state`), plus Tor's cache in `~/.termchat/arti`. Set
`TERMCHAT_HOME` to keep a separate profile elsewhere, for example to try termchat with
two identities on one computer:

```sh
TERMCHAT_HOME=~/termchat-test ./termchat
```

### 11. If something goes wrong

| Problem | What to do |
|---|---|
| Stuck on `○ connecting…` | Check your internet. With Tor, the first connection can take up to a minute. If it never connects, turn Tor off in settings → security to go without Tor and see whether the relay is reachable. |
| `SAVE FAILED` in the status bar | The disk is full or not writable. Messages are not acknowledged until they are saved, so nothing is lost; free some space. |
| `wrong password` | Passwords are case-sensitive and depend on the keyboard layout. There is no recovery; restore from a backup if you have one. |
| `they use another relay` when adding a contact | Your friend's invite is for a different relay. Both of you must use the same one (for now). |
| **c** does not copy the invite | Your terminal does not support clipboard access (OSC 52). Select the text with the mouse instead. |
| The layout looks broken | Make the window larger (at least 80×24) and use a font with Unicode box characters. In an old terminal without true colour pick the `terminal` theme. |
| Selecting text with the mouse does not work | Hold **Shift** while dragging, or turn the mouse off in settings → interface. |
| No sound | macOS plays a system sound; Linux needs `canberra-gtk-play` or `paplay`; otherwise the terminal bell is used, which your terminal may mute. |

### 12. Settings, themes and notifications

**Ctrl+P**, then **s** opens settings. **Tab** or a click switches tabs, **Up/Down**
moves, **Enter** or a click changes the value.

- **theme:** 18 colour themes, light and dark; moving through the list shows each
  one at once. The chat keeps your terminal's background; the theme colours the
  bars and windows.
- **notifications:** when a message arrives for a chat you are not looking at, a
  card with the sender's name (never the text) shows in the corner for 5 seconds;
  click it to open the chat. **Sound** plays the system notification sound (on by
  default). **System notification** (off by default) asks the terminal to show a
  desktop notification that always says just "termchat: new message": the
  operating system keeps a history of notifications outside termchat, so names
  and text never go there. The window title shows only the number of unread
  messages, `termchat (3)`.
- **interface:** mouse on or off.
- **security:** backup, password, duress password, Tor, wipe.

These settings live in `~/.termchat/settings.json`, unencrypted so that the lock
screen can use your theme. They hold nothing about your contacts, and wiping
removes the file too.

### Keys at a glance

| Key | Action |
|---|---|
| Enter | send |
| Up / Down | choose contact |
| PgUp / PgDn | scroll the chat |
| Ctrl+A | add contact from an invite |
| Ctrl+O | show your invite (**c** copies) |
| Ctrl+Y | accept request |
| Ctrl+F | safety number (**v** marks verified) |
| Ctrl+T / Ctrl+E | disappearing messages / no history |
| Ctrl+R / Ctrl+B / Ctrl+D / Ctrl+X | rename / block / delete or reject / report |
| Ctrl+P | menu: **s** settings, **k** keys, **m** manual, **o** invite, **a** add, **b** backup |
| Ctrl+K | lock |
| Ctrl+Q or Ctrl+C | quit |
| ? or F1 | help |
| Esc | close a window |

<!-- manual:end -->

## Public relay

| | |
|---|---|
| Direct | `wss://relay.termchat.app/` |
| Tor | `ws://rgisnh6oixs4usvi544poxwzxstbxls74x4mtzlwl4a55uuncrpolpad.onion/` |

It stores only what [docs/self-host.md](docs/self-host.md#what-the-relay-stores) lists,
keeps no access log, and is not behind Cloudflare or any other TLS-terminating proxy.
The old address `wss://chat.46-224-214-19.sslip.io/` leads to the same relay and keeps
working for clients set up with it.

## Run a relay

See [docs/self-host.md](docs/self-host.md): Docker with Caddy, or a single static
binary with systemd, plus an onion address with Tor.

## Acceptable use

termchat is for private conversations. It is **not** for trafficking people, weapons or
drugs, planning violence, or pushing anyone towards self-harm, and using it for that is
forbidden. We do not scan messages: that cannot work with end-to-end encryption and
would be a backdoor for everyone. Instead termchat gives such activity no reach: no user
search, no directory, no public channels or broadcasts, strangers land in "requests",
you can block anyone (Ctrl+B) and report them to the relay operator (Ctrl+X). See
[ADR-006](docs/adr/006-abuse-and-reports.md).

**What a relay operator can hand over:** public keys, which key sent envelopes to which
and when, undelivered ciphertext (at most 14 days), and reports users chose to send.
Not message contents, not nicknames, not IP history.

**Need help now?** https://findahelpline.com lists helplines in every country.

## Documents

- [Threat model](docs/threat-model.md) · [Protocol](docs/protocol.md) · [Self-hosting](docs/self-host.md)
- Decisions: [ADR-001 stack](docs/adr/001-stack.md), [002 vodozemac](docs/adr/002-vodozemac-integration.md),
  [003 duress](docs/adr/003-duress-password.md), [004 Tor](docs/adr/004-tor.md),
  [005 relay storage](docs/adr/005-relay-storage.md), [006 abuse](docs/adr/006-abuse-and-reports.md),
  [007 abuse resistance](docs/adr/007-abuse-resistance.md),
  [008 interface and notifications](docs/adr/008-ui-settings-and-notifications.md),
  [009 leaving](docs/adr/009-leaving.md),
  [010 update notice](docs/adr/010-update-notice.md),
  [011 Android client](docs/adr/011-android-client.md), [012 push](docs/adr/012-push.md)
- [Contributing](CONTRIBUTING.md) · [Security policy: how to report a vulnerability](SECURITY.md)

## Licence

AGPL-3.0-or-later. If you run a modified relay as a service, you must publish your changes.
