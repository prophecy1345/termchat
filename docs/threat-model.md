# termchat threat model

Status: accepted (M0, 2026-10-15). Русская версия: [threat-model.ru.md](threat-model.ru.md).

## What we protect

- **Message contents.** Only the sender and the recipient know them.
- **Identity keys.** Secret keys never leave the device, except in a
  password-encrypted export.
- **The person.** The project collects no personal data: no name, phone number
  or email. The nickname is visible only to contacts; the relay never learns it.
- **Integrity and authenticity.** The recipient knows who sent a message and
  that it was not altered, replayed or reordered.

## Who sees what

| Party | What it sees |
|---|---|
| Internet provider, Wi-Fi, the network | That the client talks to a relay, when, and how much. Not the contents or the recipients: everything is inside TLS. With Tor on, that the client uses Tor; with Tor bridges (ADR-015), the disguised transport (obfs4, webtunnel, snowflake) instead, which a determined censor can sometimes recognise. |
| Someone with the device's files, without the password | Nothing from the encrypted state file. While Tor bridges are on, the bridge lines in Arti's own folder (ADR-015), which Arti keeps in the clear. |
| Relay operator (or whoever seizes the server) | Users' public keys; who writes to whom and when (the relay knows the sender from the authenticated connection); message size rounded up to a bucket; IP addresses of live connections (not written to disk by default); the queue of undelivered ciphertext. The `Auth` signature is non-repudiable: an operator who keeps it can prove to others that a key connected at a given time. The stock relay does not keep it. When someone wipes their account (not with the duress password), that the key left and to whom it wrote last (ADR-009). |
| A contact | The conversation with them. |
| A stranger who knows a public key | Can send a contact request. Can try to spam or to exhaust prekeys. Can learn whether that key is registered on a given relay: `ClaimKey` and `Send` to an unknown key answer `UnknownRecipient`. Keys are random, so this tells nothing about anyone the stranger does not already know. |

## What is on the relay's disk (ADR-005)

Public keys, registration and last-seen times; public prekeys; undelivered
ciphertext with sender, recipient and time (at most 14 days old, deleted as soon
as it is delivered); banned keys; the report log (only what recipients chose to
disclose). No message contents, nicknames, IP addresses, history of delivered
messages, or "who wrote to whom" beyond the queue.

## Protected against

1. **Passive interception:** TLS to the relay and end-to-end encryption (Olm)
   between clients.
2. **A compromised relay:** it stores and forwards ciphertext only.
3. **A man in the middle at first contact:** manual comparison of the safety
   number. Until it is compared, the client marks the contact as not verified.
4. **A long-term key leak:** the Double Ratchet gives forward secrecy (old
   messages stay unreadable) and post-compromise security (the session heals).
5. **Forged, replayed or reordered messages:** authenticated encryption and the
   Olm ratchet state.
6. **Registering under someone else's key:** the relay demands a signature over
   a nonce with the Ed25519 key.
7. **Spam:** a stranger's first message lands in "requests". The relay rate-limits
   by key and IP and limits new recipients per day.
8. **Exhausting one-time prekeys:** a fallback key and a per-requester limit.
9. **Queue flooding:** a quota and a TTL per recipient.
10. **Forwarding someone else's ciphertext** (misattribution): the encrypted
    message carries the sender's and recipient's keys and the client checks them.
11. **Revealing the user's IP to the relay and the provider** (optional,
    ADR-004): the client reaches the relay's onion address through built-in Arti
    and never falls back to a direct connection, even if Tor fails to start.
    Without Tor the relay sees the IP.
12. **A tampered release or dependency** (supply chain): attested releases,
    reproducible builds (anyone can rebuild and compare hashes), `cargo vet` and
    `cargo deny` for dependencies.

## Partly protected

- **Metadata.** Size is hidden by padding to buckets, but the relay sees the
  social graph and timing of each identity. **Incognito contacts** (ADR-014)
  give one contact an identity of its own: its own keys, name and relay
  account. The relay then sees two keys that talk only to each other.
  - Under Tor, with separate circuits, the relay cannot link that pair to your
    other keys from the network, though it may guess from timing.
  - Without Tor, the shared IP address links them.
  - Contacts of different identities cannot link you by key or fingerprint.
  - Incognito is opt-in per contact; the main identity's contacts stay visible
    to the relay as one circle. Sealed sender is not planned (ADR-014, section 5).
- **The start of a new session.** An Olm pre-key message is longer than a normal
  one; the relay can notice.
- **Physical access to an unlocked device.** Auto-lock after idle time (5
  minutes by default) and a lock key: keys are wiped from memory and the
  password is needed. Disappearing messages and "no history" chats limit what
  can be read. No help if the device is taken within those minutes.
- **A relay that drops messages.** It cannot be prevented, but it is noticed:
  messages carry a sequence number inside the ciphertext, and a gap shows as
  "messages missing". Moving to another relay takes a new invite.
- **A lost device.** Encrypted identity export; the client reminds you on first
  run. Without it the identity cannot be recovered. After a restore, contacts
  see a new-session warning.
- **Being forced to give a password.** The duress password (ADR-003)
  irreversibly destroys the data key and shows the first-run screen. No help if
  the disk was copied before. The code is open, so an adversary knows about the
  feature. Destroying data after a seizure can be a crime and can increase the
  pressure.
- **Interface traces outside encryption (ADR-008).** The settings file (theme,
  sound, mouse) is plain and says only that termchat is installed; it is wiped
  with the data. The window title shows only the unread count. The system
  notification is off by default and always carries one fixed text, because the
  operating system keeps a notification history outside termchat. The card in
  the corner shows the sender's name to whoever sees the screen.
- **A weak storage password.** The client rates strength (`zxcvbn`), rejects weak
  passwords and offers a generated six-word phrase. Argon2id slows brute force.
  No help against a password written next to the computer.

## Not protected

- **Malware on the device**, a keylogger, a compromised terminal or OS. A program
  running as the user can read memory and keystrokes. We limit the damage: on
  disk everything is encrypted while the client is closed; core dumps are off;
  the TUI uses the alternate screen and leaves no conversation in scrollback.
  But terminal logging (`script`, tmux logs) and screen recording capture text.
- **A global observer** correlating traffic timing. Tor does not stop that; it
  needs a mixnet with cover traffic.
- **Keys in swap and hibernation files.** Locking memory (`mlock`) needs
  `unsafe`, which the project forbids. Use the OS's disk encryption (FileVault,
  BitLocker, LUKS).
- **Windows.** Core-dump suppression and `0600` permissions on the state file
  exist only on Unix. On Windows the file is protected by the password but is
  readable by other processes of the same user, like any file in the profile.

## Assumptions

- The cryptography of vodozemac and RustCrypto is correct. vodozemac had an
  external audit; our code has not.
- Users compare the safety number over an independent channel (in person, by
  phone).
- The OS's disk encryption is on (otherwise keys may end up in swap).
- The system's random number generator is sound.
