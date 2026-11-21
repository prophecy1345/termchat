# What's new

Shown inside termchat.app: Ctrl+P, n in the terminal, Settings → What's new in
the desktop app.

## 0.21.0

- `termchat run -- COMMAND` runs a command and then sends one report to a
  contact or private group: exit status, duration, host and the last lines
  of output, with `--failures-only` for quiet successes (ADR-035).

## 0.20.2

- The `termchat watch` alert counts in whole minutes when it can, as in
  "backup-box has been silent for 15 minutes".

## 0.20.1

- `termchat ask` accepts a reply regardless of letter case and spaces at
  the ends, so `Yes CODE ` from a phone keyboard counts (ADR-034).
- `termchat watch send --every` takes at least 10 seconds and
  `watch monitor --after` at least 30.

## 0.20.0

- Scripts can list private groups, send text, and receive new texts and
  replies by group name or ID, directly or through the daemon (ADR-033).
- Group queue moves stop old-session sends and hold new text until the
  new queue and encryption session are ready.

## 0.19.0

- `termchat ask` lets scripts wait for an approval from a verified contact
  using an encrypted direct message (ADR-032, Linux and macOS).

## 0.18.0

- `termchat watch` sends an encrypted heartbeat from a server to a second
  host. The watcher messages a verified contact if the server goes silent
  (ADR-031, Linux and macOS).
- Android: What's new wraps release notes to the available screen width.

## 0.17.3

- Groups: your own devices no longer show in a group's invite list; they
  join your groups by themselves.
- Groups: while a member uses a version before 0.17, a group stays on its
  members' main devices. Once they update, the devices kept out now join;
  before, they never did.
- The status line no longer keeps saying "confirmed: linking finishes when
  the other screen agrees" after the device is linked.

## 0.17.2

0.17.1 was tagged but never released; 0.17.2 has everything below.

- Linking a device: the main device waits until the new one can show the
  fingerprint too (through Tor its answer takes up to half a minute), so
  both screens show it together and "The same" is not pressed alone. The
  new device no longer hides its fingerprint window behind a note.
- Android: a note said in a chat no longer stays over the chat list after
  you go back.

## 0.17.1

- Relay: once all queues together reach the relay's cap, it still takes
  messages for small queues and refuses only large ones, so a relay filled
  on purpose keeps carrying everyone else's messages (ADR-030).
- Android: the attach button is a paperclip drawn like the timer's
  envelope, instead of the coloured emoji.

## 0.17.0

The rest of the security fixes from the third source review (ADR-030).
After updating, link your other devices to your main one again: erase each linked device (wipe, MOVED) and link it from the first screen. The main device keeps its history.

- Each device signs that it joins your identity, and every device list is
  checked for it: a list can no longer name a key whose holder did not
  agree. Lists from earlier versions are dropped on the first unlock.
- Copies of what you send go to your other devices with the chat they are
  for, and land nowhere else.
- Groups show "N message(s) missing so far", as chats do, when the relay
  withholds or loses group messages.
- Contacts on versions before 0.17 hear only your main device until they
  update.

## 0.16.1

Security fixes from the third source review (review-2026-11c, ADR-030):

- A contact could plant a contact with someone else's fingerprint, or
  erase your sessions with another contact, through a device list naming
  a key that is not theirs. Such lists are now refused, and a key known
  only from a list never counts as a contact.
- A relay address from a contact (a group invite, or with Tor on) could
  make your state file refuse the right password. Addresses are checked
  when they arrive, and a relay that cannot be reached never stops
  unlocking.
- A group member who is not the admin could remove another member.
- A contact could point your app at a service on your own computer.
- Group members could fill your contact-request slots; the relay's report
  journal could be filled by reconnecting.
- The threat model now says what a contact's own relay learns without Tor,
  that the group admin holds the readers' tokens, and that groups do not
  show missing messages yet.

## 0.16.0

- Link a new device from its first screen (ADR-028): choose a password
  and link instead of creating an identity, no nickname to make up. The
  link code shows as soon as the relay is reached. Terminal: Ctrl+L;
  desktop: Link to my main device instead; Android: Link to my main
  device.
- Contacts you make after linking a device now reach it too. Before, only
  the contacts from the moment of the link did.
- A contact's first message to your new device no longer gets lost while
  their app sets up a session with it.
- Linked devices no longer ask for a backup: back up the main device; a
  lost linked one is removed and another linked.
- Android: the disappearing timer is a crumbling envelope, and a tap says
  what the timer became.

## 0.15.3

- A command, Ctrl+Q in the terminal, closing the desktop window or
  stopping `termchat daemon` now waits until the relay took what was just
  sent (up to `--timeout`, or 5 s on quit). Before, a contact request
  accepted right before quitting could stay unanswered for good, and the
  other side could see a false "1 message(s) missing".

## 0.15.2

- Desktop: Send a file has Choose… for the system's Open dialog, and a
  file dropped on the window opens the send dialog with it. Save has
  Choose… for the folder. The path fields stay for the keyboard.
- A file macOS keeps only in iCloud now says to download it in Finder
  first, instead of "Resource deadlock avoided".

## 0.15.1

0.15.0 was tagged but never released: in the terminal, Ctrl+L and Ctrl+G
did nothing in a group. 0.15.1 has the fix and everything below.

- Documents in groups (ADR-027): send one into a group as into a chat,
  up to 1 MiB and one at a time; every member gets it and saves it by
  hand, never opened. Terminal: Ctrl+L and Ctrl+G in the group; the
  file button on the desktop, 📎 on Android.
- `termchat daemon`: a command it turns away now says why, instead of
  "Broken pipe".

## 0.14.0

For scripts and servers (ADR-029, Linux and macOS):

- `termchat daemon` keeps a server identity open. While it runs, `send`,
  `send-file`, `recv`, `contacts`, `invite` and `add` go through it: no
  password per command, an answer within a second instead of a new
  connection (10-30 s over Tor), and several commands at once. A bot
  can now answer "status" from your phone. See Scripts and servers in
  the guide for a systemd unit.
- The daemon listens only on a socket in the state folder and lets in
  only its own user. While it runs it holds the password in memory:
  give it a server identity, never your main one.
- `send` no longer reports a message as sent too early when older
  messages in a chat with a timer disappear while it waits.

## 0.13.0

- Groups on every device (ADR-028): accept an invite on one device and
  the others join too; a device linked later joins by itself, and a
  removed one leaves. Each device reads and writes there; your
  messages from one show on the others. New groups are made on the main
  device. A group with a member on an app before 0.12 keeps to main
  devices.
- Documents reach every device of a contact; your other devices show
  that you sent one, without the file.
- Linked devices meet each other through the main one, so with three or
  four devices what you send shows on all of them.
- A new colour theme, vice-sunset: hot pink and orange on night purple.
- Your contact's new device no longer warns that they "restarted the
  conversation".

## 0.12.0

One identity on up to four devices (ADR-028).

- Link a new device to your main one with a link code: Ctrl+P, l in the
  terminal, Settings, Link to my main device on the desktop, the menu on
  Android. Add it on the main device under Devices, by pasting or
  scanning the code. Both screens show the identity's fingerprint;
  nothing goes further until you confirm it is the same on both.
- Each device has keys of its own; your fingerprint does not change.
  Contacts write to every device, and what you send from one appears on
  the others. Your contacts move to the new device; history does not.
- The main device names devices ("phone", "laptop") and removes a lost
  one. Labels stay on your own devices; contacts see only how many you
  have.
- For now groups stay on the main device, documents reach a contact's
  main device only, and with three or four devices a linked one's copies
  reach the main device but not the other linked ones.
- Contacts on an app before 0.12 hear only your main device.
- Groups: an invite taken while the group moved to a new queue left the
  member in the old one, unable to read; it now moves them.

## 0.11.1

- Groups: inviting a contact right after accepting their request said
  "their app is older" until they had answered. The invite now waits a
  moment and goes as soon as their app says it knows groups.

## 0.11.0

- Groups: each member's name has a colour of its own, taken from their
  key: the same on every device and in every theme.
- Groups: a new member's messages could stay unreadable for someone in
  the group for good. Their key could come before the admin's new member
  list and was dropped; it now waits for the list.

## 0.10.0

Small private groups (ADR-027).

- Up to 32 people. The one who makes a group is its admin: only they
  invite their own contacts and remove members. There are no links that
  anyone can use and no search: nobody can find a group.
- Megolm from vodozemac, the group ratchet Matrix uses, with keys shared
  over Olm one to one. A new member reads nothing said before they
  joined; a removed one nothing after: the group moves to a new queue and
  everyone starts a new session.
- Answers, edits, deletes and reactions as in a chat; reactions show how
  many chose each one. The admin sets the disappearing timer for all.
- Members who are not each other's contacts can still read each other:
  the admin signs the member list. Compare safety numbers to be sure.
- The relay sees a group queue and how many read it: a group's size is
  visible to its operator. It cannot read the name, the members or the
  messages.
- Terminal: Ctrl+P, g for a new group; in a group Ctrl+R shows the
  members. Desktop: + New above the groups; double-click a message to
  answer it. Android: New group in the menu.
- Groups carry text only for now, no documents.
- Desktop: text fields in dialogs were invisible against the window;
  they show now.

## 0.9.2

- Documentation: a user guide at termchat.app/docs, in English and
  Russian, with every step checked. The manual in the terminal client
  (Ctrl+P, m) shows the same guide.
- Terminal: a message with several lines keeps them, and the quote over
  an answer shows its first line. They were joined into one.
- macOS: the desktop app has a rounded Dock icon like other apps, not a
  full square.
- The wipe dialog says the relay deletes your queues; "forgets your key"
  was left over from before anonymous queues.

## 0.9.1

- Answers, edits, deletes and reactions now work when the two of you
  updated at different times. If one side reached 0.9.0 while the other
  was still on 0.8, the chat stayed without them on one phone and had
  them only for one's own messages on the other. The app that updated
  first needs 0.9.1; the chat mends itself at the next connection
  (ADR-026).

## 0.9.0

Answers, edits, deletes and reactions (ADR-025).

- Answer a particular message. The answer points at it and shows its first
  line; it never copies the text, so a disappearing message still goes on
  time.
- Edit or delete your own messages for both of you. Edits keep the timer;
  no history is kept. A delete is a request: a copy or screenshot the other
  side made stays.
- React with one of seven signs, drawn by termchat.app as arcade sprites:
  like, love, laugh, wow, sad, thanks, dislike.
- Terminal: Shift+Up or Ctrl+N picks a message; Enter answers, e edits,
  d deletes, 1 to 7 react. Desktop: right-click a message. Android: long
  press.
- `termchat recv --json` prints edits, deletes and reactions as events,
  and `id` and `reply_to` with each message.
- Contacts still on 0.8 see one "could not be decrypted" line and one
  "1 message lost" when your app first says what it reads. Nothing is
  lost. Answers and reactions work in a chat once both sides run 0.9.
- From now on a newer kind of message is skipped quietly by older apps.
- `termchat invite` waits until its keys are on the relay, so the first
  `termchat add` from it no longer times out.

## 0.8.2

- Desktop: buttons, text fields, drop-down lists and check boxes are
  readable in light themes. They used to stay dark grey with dark text.

## 0.8.1

- Android: accepting a contact request now answers them at once. Since
  0.7.0 the answer went out only after the app was locked and unlocked.
- A contact who takes a fresh invite right away no longer gets stuck when
  the relay has no key for them yet: the app asks again 15 s later.
- A contact deleted and added again no longer hangs.

## 0.8.0

Scripts and servers can now write to you (ADR-024).

- `termchat init`, `invite`, `add`, `contacts`, `send`, `send-file` and
  `recv` run without the screen, so a cron job, a CI run or the end of a
  long command can send you an end-to-end encrypted message, or read your
  answer. See "Scripts and servers" in the user guide (termchat.app/docs).
- Give each server an identity of its own: its password sits on that
  server. It cannot accept requests; you make the link from your app.

## 0.7.1

- A contact who deleted you, or moved to a new device, and adds your
  invite again now learns your name. Before, they saw only the
  fingerprint, and your first answer could go to a queue that was gone.

## 0.7.0

The relay no longer knows who talks to whom (ADR-023). This release
breaks with every earlier one: read this before you update.

- The relay knows anonymous queues, not people. It stores no keys and
  learns no senders: anyone who has a queue's address can write to it,
  and only the address's owner can read it. Each contact gets a queue of
  their own and, with Tor, its own circuit.
- **Every contact must exchange invites again.** Old contacts show "needs
  a new invite". Ask them for one, or send yours: the chat, its history
  and the contact's name stay. Old invites no longer work.
- Blocking a contact deletes their queue: they cannot put a single byte
  in it. Unblocking makes a new one.
- Reports still reach the relay operator, but the relay cannot ban
  anyone any more: it does not know who wrote.
- A contact on another relay can be added.
- Settings → security → connections: one connection for all contacts
  uses less battery, but lets the relay see your queues together. Off by
  default.
- The setting for requests without an invite code is gone: invites
  without a code no longer work, so such requests are always dropped.
- The relay still sees when each queue is written to and read; timing can
  link queues. Without Tor, your IP address links your connections.
- Relay operators: the upgrade erases the old database. `--host` is no
  longer needed. Clients older than 0.7.0 are told to update.

## 0.6.3

More fixes from the second source review (ADR-022).

- Android: without "stay connected", a backgrounded app now locks when the
  lock time is up, also when Android has frozen it. Before, it held its
  keys until you came back. The lock may come a few minutes late.
- Relay: logins of one key are limited (30 at once, then one a minute),
  the report journal stops growing at 10 MiB, and a slow reader holds less
  memory. Operators should update.

## 0.6.2

Fixes from a second source review of the code (not an independent audit;
the findings are in `docs/security/review-2026-11b.md`).

- Android: coming back to the app after the lock time has passed locks it
  at once. Before, the chats could show for a few seconds, and a touch in
  that time kept the app unlocked.
- Terminal and desktop: time asleep counts toward the idle lock, and a key
  pressed after the time is up locks the app instead of extending it.
- An onion relay address is used only through Tor, never through plain DNS
  (ADR-004).
- A deleted contact can no longer be brought back as a contact request by
  a relay that floods the list of sessions already seen (ADR-019).
- Relay: a WebSocket frame larger than 64 KiB is refused before it is read
  into memory. Operators should update.
- The note of a contact request is no longer shown.

## 0.6.1

- Android: the first message after the phone has slept arrives within
  a second or two of its notification, not 10-15 s later under Tor. The
  relay keeps the sleeping phone's connection open and wakes the phone
  with a push; the app reads the message already waiting on that
  connection instead of connecting again (ADR-012). Operators should
  update the relay; with an older relay the app waits 5 s more.
- Android: the "New message" notification goes away when you come back to
  the unlocked app, also when the chat was already open.

## 0.6.0

- **Choose when the app locks itself** (ADR-021): after 1, 5, 15 or 30
  minutes without input, in all three apps. The default stays 5 minutes,
  and there is no "never". Terminal: Settings, security tab. Desktop:
  Settings, "Lock after". Android: Settings, under "Stay connected". A
  longer time gives whoever finds the device unlocked more time to read.

## 0.5.7

- Relay: a message to someone who had just received everything waiting
  for them no longer stays on the relay until their next reconnect. It
  showed as messages that went missing and arrived later, on every
  client, since 0.5.3. Operators should update; the apps are unchanged.

## 0.5.6

- Android: a message no longer waits on the relay after its notification.
  The relay had closed the connection, but the phone did not hear it and
  kept waiting on it; the message came only with the next one. A wake-up
  now always connects again (ADR-012).
- Android: with "stay connected" on, coming back to the app no longer asks
  for the password within seconds when the screen was not touched first.
- Android: the app no longer crashes when it locks with a chat open. The
  crash, too, ended in the password prompt.

## 0.5.5

- Desktop: SEND sits above File and the timer and is as wide as the two;
  together they span the message field's height, and the field takes the
  rest of the row. The timer button keeps one
  width whatever it shows, and the field no longer shows a scroll bar on
  hover.

## 0.5.4

- A message that went missing no longer fills the chat with warnings. One
  "⚠ N message(s) missing" line stays per chat, changes with the count and
  disappears once the message turns up.
- A message or file chunk the relay refuses for now (sending too fast, a full
  queue) is sent again as it was, so it no longer goes missing on the other
  side. A chunk refused for good now fails its file instead of being lost
  without a word (ADR-020).
- Android: when a notification wakes the app or it comes on screen, it
  reconnects at once. Before, a message could arrive up to a minute and a half
  after its notification (ADR-012).
- Desktop: the message field is three lines tall, wraps long text and scrolls
  the rest. Enter sends, Shift+Enter starts a new line.

## 0.5.3

0.5.2 was tagged but never released: its build failed. 0.5.3 is the same
code with the release build fixed.

Fixes from a source review of the code (not an independent audit; the
findings are in `docs/security/review-2026-11.md`).

- A relay can no longer deliver a message a second time. This was possible
  for messages written before the other side's first reply, when the session
  had started on the fallback key (ADR-019).
- The fallback key is now replaced after 7 days on your device's own clock,
  whatever the relay reports (ADR-019).
- A received document is saved under a safe name: never as a hidden file, and
  on Windows never as a device or a stream of another file.
- Message text and document bytes are wiped from memory when the app locks or
  a message disappears.
- The client refuses relay frames larger than the protocol allows.
- A backup's temporary file is never written through a link someone planted.
- Relay: a queue is delivered in pages, so a client that logs in and does not
  read cannot make the relay hold whole queues in memory; push wake-ups in
  flight are capped. Operators should update.

## 0.5.1

- The desktop app shows What's new formatted (headings, lists, bold, code)
  instead of raw Markdown.

## 0.5.0

- **termchat.app in a window** for macOS, Linux and Windows (ADR-018), for
  those who prefer a window to a terminal. Same account, same encryption,
  same relay; the look of the Android app with the site's synthwave touches.
  Contacts and requests, chats with bubbles, disappearing messages, invites
  with a QR code for the phone, the safety number, documents, backup and
  restore, passwords and the duress password, Tor and bridges, wipe.
- Install: `curl -fsSL https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.sh | TERMCHAT_GUI=1 sh`
  (macOS, Linux), `brew install prophecy1345/tap/termchat-gui`, or on Windows
  `$env:TERMCHAT_GUI = 1; irm https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.ps1 | iex`.
  It is not signed with a paid certificate; installed these ways it opens
  without a warning. See the README for a browser download.
- A window shows more to the rest of the computer than a terminal:
  screenshots and screen recording see it, and the text passes through the
  window system and the graphics driver.
- The terminal client and the app share the account but not at the same
  time: whichever opens second says termchat.app is already open.

## 0.4.1

- **Security update:** vodozemac 0.11.2 for RUSTSEC-2026-0445 and -0451.
  Versions 0.11.0 and 0.11.1 of the encryption library kept more copies of
  session secrets in memory, for longer, than intended. Nothing changes on the
  wire, so your contacts do not need to update at the same time.
- The terminal client no longer starts twice on the same account: it says
  "termchat is already open in another window". Two copies would overwrite
  each other's saved state.

## 0.4.0

- **Documents** (ADR-017). Send a file of up to 1 MiB, end-to-end encrypted
  like a message: **Ctrl+L** in the terminal, the paperclip on Android. Save a
  received one with **Ctrl+G** (or Save on Android, through the system dialog),
  or discard it. termchat never opens, previews or renders a file, and never
  overwrites one on save. Files from contacts can be harmful: check them before
  you open them.
- 200 KB takes about 10 s, 1 MiB about 3 min: chunks are paced under the
  relay's send limit. The relay sees a run of 16 KB messages, so it can
  estimate the size of a document.
- Files work only between accepted contacts. **Both sides need 0.4.0**:
  older versions show "a message could not be decrypted" for each chunk.
- Dependencies: zstd, zstd-safe and zstd-sys updated for RUSTSEC-2026-0391,
  -0398, -0403 and -0410.

## 0.3.3

- **Android: new signing key** (RSA 4096, ADR-016), the one Google Play accepts,
  so the app is signed the same on GitHub, F-Droid and Google Play. New
  fingerprint: `9F:FB:61:1C:DD:1B:3A:AE:63:30:3F:B7:12:0B:85:F7:67:0D:08:BD:B8:24:D6:7C:7C:85:37:09:88:0C:2A:7F`.
  If you installed 0.3.2 or older, uninstall it first (back up your identity
  before), then install this one.

## 0.3.2

- Android: every release also ships an app bundle (.aab) for Google Play.
  The APKs are unchanged.
- The store icon now shows the termchat mark; it was blank.

## 0.3.1

- Android: "Generate a passphrase" now shows the passphrase under the fields
  with a Copy button, instead of a warning line that was easy to miss.

## 0.3.0

- **One-time invites.** Every invite you show carries a code that works once,
  so a forwarded or leaked invite cannot be reused. "Revoke unused" stops all
  the ones you handed out but nobody used.
- **Public invite** for a profile or a post: reusable until you renew it, and
  renewing stops the old one.
- Invites from older versions have no code. Requests from them are accepted
  while settings → security → "accept requests without an invite code" is on;
  turn it off once your old contacts are added. New accounts start with it off.
- **Incognito contacts.** An incognito invite, or "add incognito", talks to
  one contact from a new identity of your own: its own keys, name and relay
  account. Contacts of different identities cannot link you, and over Tor the
  relay cannot either. Deleting the contact deletes that identity.
- **Tor bridges** for where Tor itself is blocked: paste obfs4, webtunnel or
  snowflake bridge lines in settings → security, and Tor enters only through
  them. lyrebird, the Tor Project's transport program, now ships next to
  `termchat` and inside the Android app, built from source.
- **Search a chat**: Ctrl+S in the terminal, "Search" in the chat menu on
  Android. It runs on your device only.
- **Fixes from an external security review**, among them: a full queue no
  longer locks a slow (Tor) client out of the relay; strangers together get
  at most a quarter of anyone's queue; the fallback key is no longer rotated
  by claims alone; backups leave out a request's held messages; nicknames
  mixing alphabets always show fingerprint digits.
- Invites from this version need termchat 0.3.0 or later on the other side.

## 0.2.3

- Android: the QR code scanner opens upright and follows the phone's rotation,
  instead of always turning sideways.
- The install notes explain how to get messages while the Android app is
  closed: wake-ups through ntfy or another UnifiedPush app, or staying
  connected in the background.

## 0.2.2

- The app store texts, icon and screenshots move to `fastlane/` at the
  repository root, where F-Droid reads them. No changes in the app.

## 0.2.1

- The Android build works in F-Droid's build server: the signing setup no
  longer breaks when F-Droid strips signing blocks, and the version is written
  where F-Droid's update checker looks for it.

## 0.2.0

- **Reproducible Android builds**: the APK can be rebuilt from source to the
  same bytes, so F-Droid can ship our signed APK, and both sources update each
  other.
- Version numbers now follow the project's rules: new features raise the
  minor number (CONTRIBUTING.md). The Android app should have been 0.2.0;
  this is it.

## 0.1.14

Android, after a security review:
- Android no longer copies the app's data to a new phone (device-to-device
  transfer); moving goes through termchat's own backup.
- A report sends exactly the text shown, even if more arrives meanwhile.
- Restoring from a read-only place works.
- Passwords leave the settings screen's memory right after use.
- A wipe also removes the settings file and the cache.
- Message text is shown without characters that reorder it (bidi overrides).

## 0.1.13

- **termchat for Android**, built on the same engine as the terminal client
  (ADR-011): chats, invites with QR codes, backups, passwords, the duress
  password, WIPE and MOVED, Tor, and the same colour themes.
- **Push wake-ups** (ADR-012): the relay can ping a UnifiedPush server of your
  choice when a message waits, so the phone shows "New message". Off by default.
- New theme **termchat** in the terminal too: near-black with a lime accent.

## 0.1.12

- The update notice for Homebrew says `brew update && brew upgrade termchat`:
  without `brew update` Homebrew may not see the new version for up to a day.

## 0.1.11

- **What's new** in the menu (Ctrl+P, n): this list. After an update the
  status bar points here until you open it.
- **Update notice.** When your relay runs a newer termchat, the status bar
  says so, and "what's new" shows how to update: `brew upgrade termchat` for
  Homebrew, the install command otherwise. No extra connection: the relay
  reports its version when you connect (ADR-010).

## 0.1.10

- **Every wipe erases backups left on this device.** termchat remembers where
  it wrote a backup or restored one from; a wipe (duress password, `WIPE`,
  `MOVED`) erases a backup still at that path. Copies moved elsewhere are not
  touched. See ADR-003.
- The backup form no longer suggests `~/termchat-backup`: type where it goes,
  a USB stick for example.

## 0.1.9

- The chat menu says what Ctrl+T does: `disappearing: off → 5m`, `1h → 1d`,
  `7d → off`.

## 0.1.8

- **Moving to another device.** Type `MOVED` instead of `WIPE` to erase the
  old device silently, without telling contacts or leaving the relay. The
  manual has the steps (section 8).

## 0.1.7

- **Wiping tells your contacts.** `WIPE` sends a goodbye to every contact and
  asks the relay to forget your key. Their chat with you becomes read-only and
  is marked `-` in the list. The duress password still sends nothing.
- A message to a contact who is gone shows "contact is no longer on the relay".

## 0.1.6

- Wiping also removes the Tor directory.
- Security fixes in the relay and in relay URL parsing.
