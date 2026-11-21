<p align="center"><img src="docs/logo/termchat-mark-dark.svg" alt="termchat.app logo" width="128"></p>

# termchat.app

По-русски: [README.ru.md](README.ru.md).

A private messenger for the terminal: text, documents up to 1 MiB, small private groups, and one identity on up to four devices. End-to-end encrypted with
[vodozemac](https://github.com/matrix-org/vodozemac) (Olm: X3DH-style handshake and
Double Ratchet, the design behind Signal and Matrix). Optional Tor. No accounts,
phone numbers, emails or names: you are a key pair and a nickname you choose.
Website: [termchat.app](https://termchat.app).

**Status: not audited.** The cryptography comes from vodozemac, which was audited in
2022; termchat.app's own code has had no external review. Treat it as experimental.

Independent security audit: not yet. AI-assisted security review (Claude Fable 5.2,
November 2026): findings and fixes of the [first](docs/security/review-2026-11.md)
and [second](docs/security/review-2026-11b.md) pass.

termchat.app is a free, non-commercial, open-source project created and maintained as a
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
- **Metadata.** The relay knows anonymous queues, not people: no keys, no senders,
  no "who writes to whom" (ADR-023). It still sees when each queue is written to
  and read, which can link queues by timing, and message size rounded up to
  512 / 4096 / 16384 bytes. Each contact gets its own connection; without Tor,
  your IP address links them. Nicknames are never visible to it.
- **Someone with your unlocked computer.** Auto-lock after 1 to 30 idle minutes
  (5 by default), Ctrl+K to lock now, disappearing messages, chats without history.
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

```sh
brew install prophecy1345/tap/termchat                       # macOS, Linux
curl -fsSL https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.sh | sh
```

```powershell
irm https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.ps1 | iex   # Windows
```

Then run `termchat`. The desktop app (`TERMCHAT_GUI=1` before `sh`, or
`brew install prophecy1345/tap/termchat-gui`), the Android APK, archives by
hand and how to check that a download is genuine are in
[Install](docs/guide/install.md).

## Documentation

The user guide is at **[termchat.app/docs](https://termchat.app/docs/)**, and
the same pages are in [docs/guide](docs/guide/):

- [Install](docs/guide/install.md) · [First run](docs/guide/first-run.md) ·
  [Contacts](docs/guide/contacts.md) · [Messages](docs/guide/messages.md)
- [Terminal](docs/guide/terminal.md) · [Desktop app](docs/guide/desktop.md) ·
  [Android](docs/guide/android.md) · [Tor and bridges](docs/guide/tor.md)
- [Backups, locking, wiping](docs/guide/safety.md) ·
  [Settings and data](docs/guide/settings.md) ·
  [Scripts and servers](docs/guide/scripts.md) ·
  [Run a relay](docs/self-host.md) · [If something goes wrong](docs/guide/trouble.md)

In the terminal client, **?** lists the keys and **Ctrl+P**, **m** opens the
same guide.

Run `termchat watch` on two hosts to get a private message when a server
goes quiet. [Set up server monitoring](docs/guide/scripts.md#get-a-message-when-a-server-goes-quiet)

Use `termchat ask` to pause a script or CI job for approval from a verified
contact. [Set up script approvals](docs/guide/scripts.md#ask-for-approval-before-a-script-step)

Wrap a cron job in `termchat run` to get its exit status, duration and last
lines of output when it ends. [Report how a command ended](docs/guide/scripts.md#report-how-a-command-ended)

Send reports to a private group and read replies as JSON from your script.
[Use groups from scripts](docs/guide/scripts.md#private-groups)

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

termchat.app is for private conversations. It is **not** for trafficking people, weapons or
drugs, planning violence, or pushing anyone towards self-harm, and using it for that is
forbidden. We do not scan messages: that cannot work with end-to-end encryption and
would be a backdoor for everyone. Instead termchat.app gives such activity no reach: no user
search, no directory, no public channels or broadcasts, strangers land in "requests",
you can block anyone (Ctrl+B) and report them to the relay operator (Ctrl+X). See
[ADR-006](docs/adr/006-abuse-and-reports.md).

**What a relay operator can hand over:** anonymous queues and when each was written
to and read, undelivered ciphertext (at most 14 days), and reports users chose to
send. Not message contents, not nicknames, not who talks to whom, not IP history
(ADR-023).

**Need help now?** https://findahelpline.com lists helplines in every country.

## Documents

- [User guide](docs/guide/) · [Threat model](docs/threat-model.md) ·
  [Protocol](docs/protocol.md) · [Running a relay](docs/self-host.md)
- [Decisions (ADRs)](docs/adr/): every protocol and security choice, with what it
  costs.
- [Contributing](CONTRIBUTING.md) · [Security policy: how to report a vulnerability](SECURITY.md)

## Support the project

termchat.app is free, with no ads or paid features. If you want to help, [sponsor it on GitHub](https://github.com/sponsors/prophecy1345):
it is built by one person, and donations let me keep working on it and pay for the public relay, signing
certificates for the apps and, one day, an independent security audit. Sponsors get nothing extra in the app.

## Licence

AGPL-3.0-or-later. If you run a modified relay as a service, you must publish your changes.
