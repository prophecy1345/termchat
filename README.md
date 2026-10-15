# termchat

A private, text-only messenger for the terminal. End-to-end encrypted with
[vodozemac](https://github.com/matrix-org/vodozemac) (Olm: X3DH-style handshake and
Double Ratchet, the design behind Signal and Matrix). Optional Tor. No accounts,
phone numbers, emails or names: you are a key pair and a nickname you choose.

**Status: not audited.** The cryptography comes from vodozemac, which was audited in
2022; termchat's own code has had no external review. Treat it as experimental.

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
(the client) and `termchat-relay` (the server).

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

Dependencies are checked with `cargo deny` (licences, known vulnerabilities) and
`cargo vet` (imported audits from Mozilla, Google and others; the rest is listed as
not yet reviewed in `supply-chain/`).

## Use

```sh
termchat
```

First run asks for the relay URL (and its onion URL if you want Tor), a nickname and a
password (Ctrl+G suggests a six-word passphrase). Then:

- **Ctrl+O** shows your invite. Send it to a friend any way you like: it holds only
  public keys.
- **Ctrl+A** adds a friend from their invite.
- **Ctrl+F** shows the safety number. Compare it in person or by phone.
- **Ctrl+P** for backup, password change, duress password, Tor, wipe. **?** lists every key.
- Names come from the people themselves, so unverified contacts show the start of
  their fingerprint (`alice·36335`). Messages from someone you have not accepted
  stay hidden until you press Ctrl+Y.
- Paste with your terminal's paste command: pasted text is taken literally and
  never triggers hotkeys (bracketed paste).

Data lives in `~/.termchat` (override with `TERMCHAT_HOME`).

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
  [007 abuse resistance](docs/adr/007-abuse-resistance.md)
- [PLAN.md](PLAN.md)

## Licence

AGPL-3.0-or-later. If you run a modified relay as a service, you must publish your changes.
