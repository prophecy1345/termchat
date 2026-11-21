# termchat.app threat model

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
| Relay operator (or whoever seizes the server) | Queues, not people (ADR-023): random queue IDs, when each is written to and read, message size rounded up to a bucket, and the queue of undelivered ciphertext. No user keys, no senders, no recipients by name. Which queues belong together only by a shared connection, IP address or timing: the client keeps a connection per contact, and with Tor a circuit per contact, so the first two are hidden; timing is not. IP addresses of live connections (not written to disk by default). When someone wipes their account (not with the duress password), that their queues were deleted together (ADR-009). |
| A contact | The conversation with them. |
| A stranger with an invite | Can send a contact request to that invite's queue, and nowhere else. Can try to spam it or to exhaust its prekeys; the owner can make a new invite queue, which revokes the old invites. Learns nothing from the relay about anyone else's queues. |

## What is on the relay's disk (ADR-005)

Queues: two random IDs each, the SHA-256 of the owner's token, creation and
last-use times, and its UnifiedPush endpoint if set; public prekeys per queue;
undelivered ciphertext per queue and time (at most 14 days old, deleted as soon
as it is acknowledged); the report log (only texts recipients chose to disclose,
with no queue and no sender). No user keys, message contents, nicknames, IP
addresses, senders, history of delivered messages, or "who wrote to whom".
A relay upgraded from version 1 erases its old database, which held all of
that (ADR-023).

## Protected against

1. **Passive interception:** TLS to the relay and end-to-end encryption (Olm)
   between clients.
2. **A compromised relay:** it stores and forwards ciphertext only.
3. **A man in the middle at first contact:** manual comparison of the safety
   number. Until it is compared, the client marks the contact as not verified.
4. **A long-term key leak:** the Double Ratchet gives forward secrecy (old
   messages stay unreadable) and post-compromise security (the session heals).
5. **Forged, replayed or reordered messages:** authenticated encryption and the
   Olm ratchet state; a session is never built twice from the same first
   message (ADR-019).
6. **Reading or deleting someone else's queue:** it takes the queue's random
   token, which the relay keeps only as a hash (ADR-023).
7. **Spam:** strangers reach only an invite queue, and their first message lands
   in "requests". A contact's queue takes only that contact; blocking deletes
   it. The relay rate-limits writes per queue and new queues per IP.
8. **Exhausting one-time prekeys:** a fallback key on the invite queue and a
   per-queue limit.
9. **Queue flooding:** a quota and a TTL per queue.
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

- **Metadata.** The relay knows queues, not people (ADR-023), but it still sees:
  - **timing**: when a queue is written to and when it is read, so a message
    into one queue followed by a read from another can link them. Only a
    mixnet with cover traffic hides this (see "Not protected"). When the two
    people use different relays, each relay holds one side only, and linking
    the two takes both operators;
  - **links by connection or IP**: each contact has its own connection and,
    with Tor, its own circuit. Without Tor, the shared IP address shows which
    queues are one person's, and so does the "one connection for all
    contacts" setting, which is off by default;
  - **size** rounded up to a bucket; a document (ADR-017) goes as a run of full
    16 KB messages, so its size shows to about 16 KB;
  - the number of contacts, roughly, from the number of connections that
    come and go together at unlock;
  - **a contact's own relay**: a contact may give an address on a relay they
    run (ADR-023, decision 4). Without Tor, that relay learns your IP address
    and when you are online. With Tor it sees a Tor exit, as ours does.
  **Incognito contacts** (ADR-014) still give one contact an identity of its
  own, so contacts cannot link you to each other by key or fingerprint.
- **Server liveness monitoring** (ADR-031). Regular `watch` heartbeats can
  reveal when a server is active or goes quiet through traffic timing. The
  relay sees encrypted messages and can delay or drop both heartbeats and
  alerts. The watcher needs a separate awake host and synchronized clocks.
- **Script approvals** (ADR-032). A response must come from the verified
  contact's primary device on its direct queue and match the current
  request's code. Anyone who can send as that device can approve a pending
  request. The script acts on the exit status and defines the step in
  advance. Questions and answers appear as ordinary chat messages. The
  question may also appear in the calling shell or CI logs.
- **Groups** (ADR-027). The relay sees a group queue, how many readers it
  has, and when each message is written and read by each reader: a group's
  size is visible to its operator. Key shares come in bursts when someone
  joins or leaves, which shows membership changes. It cannot read the name,
  the member list or the messages, and does not know who the admin is.
  Members who are not each other's contacts trust the admin's signed list
  for who the others are, until they compare safety numbers. A new member
  reads nothing from before they joined; a removed one nothing after.
  With linked devices, a reader is a device, so the operator sees how many
  devices the members have. A document sent to a group is a run of
  full-size envelopes from one member, stored once per reader until read.
  The admin gets every reader's token from the relay before handing it on,
  so the admin could read (as ciphertext) and delete what waits for a
  member. A reader past its quota or its rate limit loses group messages;
  as in a chat, the gap shows as "messages missing" (ADR-030).
- **Several devices** (ADR-028). Every device has keys of its own and reads
  everything sent to you, so each one is a way in: a stolen unlocked laptop
  reads like your phone would. Remove a lost device from the main one; it then
  gets nothing new, but keeps what it had. Contacts see how many devices you
  have and their keys. The relay sees a message to you as one write per
  device, and can tie those queues together as yours; without Tor, so can
  your IP addresses. Linking is checked by eye: both screens show the
  identity's fingerprint, and a copied link code shows another one. Each
  device signs that it joins the identity, so a list cannot name a key whose
  holder did not agree (ADR-030). Only the
  main device adds or removes devices; if it is lost, restore it from its
  backup.
- **An identity on a server** (scriptable mode, ADR-024). Its password sits
  next to its state file, so whoever gets root there reads that identity's
  messages and can write as it. It is a separate key with separate queues,
  so it is no way into your main identity; give each server its own. A
  daemon (ADR-029) keeps the password and keys in memory while it runs, and
  any process of its user can send through its socket; other users cannot.
- **Group scripts** (ADR-033). The server has access to the private groups
  its device joins. Same-user processes can read pending group texts through
  the daemon. Scripts can retain plaintext and author fingerprints in logs.
  Known departures and device revocations stop queued old-session group
  sends. The relay can still accept bytes already in transit, and a device
  only acts on membership changes it has learned. A successful send
  confirms relay acceptance. Read markers are saved before output, so a
  crash, broken pipe or slow subscriber can lose script output.
- **CI notifications** (ADR-036). CI keeps no identity: it reaches a
  server's CI identity over SSH with a key bound to one forced command and
  recipient. A leaked key sends texts to that recipient, or starts the
  fixed `ask`, but reads nothing. An identity restored from CI secrets on
  every run would reuse Olm message keys, so the guide rules it out.
- **Command reports** (ADR-035). `run` sends the last lines of a command's
  output, so a token or password the command prints ends up in the chat
  history, notifications and, for a group, on every member's device. The
  report leaves out arguments and the environment; it does not look for
  secrets in the output. Use `--lines 0` or `--failures-only` for such
  commands.
- **The start of a new session.** An Olm pre-key message is longer than a normal
  one; the relay can notice. The relay also chooses whether a new session
  starts on a one-time key or on the fallback key. On the fallback key, the
  messages sent before the first reply depend on a key that lives about 30
  days (ADR-019), so a leak of that key within that time exposes them.
- **Physical access to an unlocked device.** Auto-lock after idle time (1 to
  30 minutes, 5 by default, ADR-021) and a lock key: keys and message text are wiped from
  memory and the password is needed. This is best effort: the copies termchat.app
  holds itself are wiped, but libraries (JSON, vodozemac's serialization, the
  interface toolkit, the terminal) make short-lived copies of their own that
  are freed without being wiped. Disappearing messages and "no history" chats
  limit what can be read. No help if the device is taken within those minutes.
- **A relay that drops messages.** It cannot be prevented, but in a chat it is
  noticed: messages carry a sequence number inside the ciphertext, and a gap
  shows as "messages missing", in a chat and in a group (ADR-030). Moving to another relay takes a new invite.
- **A lost device.** Encrypted identity export; the client reminds you on first
  run. Without it the identity cannot be recovered. After a restore, contacts
  see a new-session warning.
- **Being forced to give a password.** The duress password (ADR-003)
  irreversibly destroys the data key and shows the first-run screen. No help if
  the disk was copied before. The code is open, so an adversary knows about the
  feature. Destroying data after a seizure can be a crime and can increase the
  pressure.
- **Interface traces outside encryption (ADR-008).** The settings file (theme,
  sound, mouse) is plain and says only that termchat.app is installed; it is wiped
  with the data. The window title shows only the unread count. The system
  notification is off by default and always carries one fixed text, because the
  operating system keeps a notification history outside termchat. The card in
  the corner shows the sender's name to whoever sees the screen.
- **A weak storage password.** The client rates strength (`zxcvbn`), rejects weak
  passwords and offers a generated six-word phrase. Argon2id slows brute force.
  No help against a password written next to the computer.

## Not protected

- **Google, for installs from Google Play.** Play App Signing holds a copy of
  the release key (ADR-016), so Google could sign an update, or an APK that
  installs from elsewhere would accept. Install from F-Droid or GitHub and
  compare the fingerprint if that trust is too much.
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
- **Android memory.** Passwords, message text and invites pass through Kotlin
  strings and Compose state, which the JVM does not let the app wipe; Rust
  wipes its own copies. Debug builds allow screenshots (`FLAG_SECURE` is on in
  release builds only). "Stay connected in the background" turns off the
  auto-lock, as the setting says.
- **Copies on someone else's servers.** A backup saved through the Android file
  picker to a cloud folder is erased by a wipe only while the app still has
  access to it, and the provider may keep its own copies. Keep backups on
  storage you control.
- **Saved documents and what opens them** (ADR-017). termchat.app never opens
  a received file, but the program that later opens it may have holes, and
  the file may be harmful. A saved file lives outside the encrypted state, so
  a wipe or the duress password does not erase it.
- **What a report contains.** The reporter chooses the text it sends (ADR-006);
  the relay cannot prove the reported key wrote it. Operators should treat a
  report as a claim.

## Assumptions

- The cryptography of vodozemac and RustCrypto is correct. vodozemac had an
  external audit; our code has not.
- Users compare the safety number over an independent channel (in person, by
  phone).
- The OS's disk encryption is on (otherwise keys may end up in swap).
- The system's random number generator is sound.
