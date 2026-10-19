# What's new

Shown inside termchat: Ctrl+P, n.

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
