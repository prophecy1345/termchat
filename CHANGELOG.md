# What's new

Shown inside termchat: Ctrl+P, n.

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
