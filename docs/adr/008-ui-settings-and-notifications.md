# ADR-008: interface settings, sound and notifications

Status: **accepted** (maintainer, 2026-10-16). Date: 2026-10-16. Русская версия: [008-ui-settings-and-notifications.ru.md](008-ui-settings-and-notifications.ru.md).

## Context

The client's interface was rebuilt after herdr: themes, mouse, menus, a contact
list with unread counts, sound and pop-up notifications. Some of this reaches
beyond the encrypted state file or beyond the terminal itself, which changes
what is visible on disk and in the system. So under rule 2 it needs the
maintainer's decision.

## 1. Interface settings live in a plain file

**Proposal.** `~/.termchat/settings.json` (mode 0600, written with
`data::write_atomic`). It holds only:

- the theme;
- sound on/off;
- system notifications on/off;
- the contact list width;
- mouse on/off.

No keys, names, contacts, times or counters. A broken or foreign file reads as
the defaults.

**Why plain.** The unlock and first-run screens should use the chosen theme, and
before the password is typed the encrypted data is not available.

**Cost.** The file shows that termchat is installed, which `~/.termchat/state`
shows anyway. A theme choice says nothing about contacts.

**Erasing.** `data::wipe` (the wipe action and the duress password, ADR-003)
erases this file too. After erasing, the folder looks like a fresh install.

## 2. Unread counts are stored in the encrypted file

The `Contact::unread` counter used to be lost on exit. It is now saved with the
rest of the data inside the encrypted file. It never reaches the relay or any
plain file.

## 3. Sound

An external system player plays the sound in the background:

- macOS: `afplay /System/Library/Sounds/Glass.aiff`;
- Linux: `canberra-gtk-play -i message-new-instant`, otherwise `paplay` with the
  standard freedesktop sound;
- Windows: `powershell -NoProfile -Command [System.Media.SystemSounds]::Asterisk.Play()`;
- with no player, the terminal bell `\x07`.

The arguments are fixed in the code. No message text, contact name or other
outside input goes into the command. At most once every 2 seconds, so a burst of
messages does not turn into noise. On by default, can be turned off in the
settings.

**Rejected.** A bundled sound file (licensing, binary size) and an audio library
(a new dependency for one sound).

## 4. Notifications

- **Inside termchat:** a card in the bottom-right corner for 5 seconds with the
  contact's name and "new message" or "contact request". The message text is not
  shown. It does not appear if that chat is open and the terminal window has
  focus.
- **System notifications** through the terminal (OSC 9; OSC 99 in kitty). **Off**
  by default. When on, they show only the fixed text "termchat: new message",
  with no name or text. Reason: the operating system keeps a notification
  history outside termchat (macOS in a plain database in the user's profile), and
  it survives erasing termchat's data.
- **The terminal window title:** `termchat` or `termchat (N)`, where N is the
  total unread count. No names: the title is visible to the task bar, the window
  switcher and screen recordings.

## 5. Mouse

Mouse capture is on by default, as in herdr. While it is on, ordinary text
selection with the mouse is captured; in most terminals selection works with
Shift held. Capture can be turned off in the settings.

## What does not change

The protocol, the relay, the encrypted file format (apart from saving `unread`)
and the cryptography. Message contents are still never scanned (ADR-006).
