---
title: "termchat.app backups, moving devices, locking, duress password, wiping"
description: "Back up your termchat.app identity, restore it, move to a new device, lock the app, set a duress password and wipe everything, with what each one does and does not protect."
---

# Backups, locking, wiping

Русская версия: [safety.ru.md](safety.ru.md).

Your identity, the keys, exists only on your device. **Without a backup, a lost or broken device means a lost identity**, and your contacts must add you again.

## Backup

- **Make one:** **Ctrl+P**, then **b** in the terminal; **Back up your identity** in the settings of the desktop app and on Android. Choose where to save it and a password for the backup (the terminal's **Ctrl+G** makes one up).
- It holds your identity and contact list, not your messages.
- **Move it off this device:** a USB stick, another computer. termchat.app remembers where it wrote a backup, and any wipe, the duress password included, erases the backup still at that path, so one left on this disk does not give you away. A copy elsewhere is not touched; that copy is how you get your identity back.
- Until you make one, the app reminds you: "no backup" in the status bar, a dot by the menu item.

## Restore

On the first-run screen of a new device: **Ctrl+B** in the terminal, **Restore from a backup** in the desktop app, **Restore** on Android. Give the backup file, then unlock with the **backup's** password. Chats start fresh, and contacts see that you restarted the conversation.

Use one device at a time: two devices with the same identity would fight over the same keys.

## Moving to another device

1. On the old device, make a **fresh** backup: an older one lacks the contacts you added since.
2. Copy it to the new device, and quit termchat.app on the old one.
3. Install termchat.app on the new device and restore from the backup, as above. Change the password later in settings if you like.
4. Check that your contacts are there and send each a message. Their next message starts a new conversation with this device; anything sent during the move may be lost.
5. On the old device: settings → security → wipe, and type **`MOVED`**, not `WIPE`. `MOVED` erases only that device and tells nobody. `WIPE` would delete your account: contacts would see that you left, on the new device too.
6. Delete the backup file wherever you no longer need it, or keep it somewhere safe as your next backup.

## Locking

termchat.app locks itself after a few minutes without a key press, click or touch: 5 by default, 1, 5, 15 or 30 in settings (security tab in the terminal, **Lock after** in the apps). **Ctrl+K** in the terminal, **Lock** in the desktop app and on Android lock at once. Locking removes keys and messages from memory; unlock with your password.

## Change password

Settings → security → **change password**: the current one, then the new one twice.

## Duress password

A second password. Typing it at the unlock screen silently erases everything, the backup still at its remembered path included, and shows the first-run screen, as if termchat.app had never been used.

Know its limits before you rely on it:

- it does not help if someone copied your disk before;
- the code is public, so whoever makes you unlock may know the feature exists;
- destroying data after a seizure can be a crime where you live.

Set it in settings → security (**duress password**, **Set a duress password**, **Duress password**). Leave the new password empty to turn it off.

## Wipe everything

Settings → security → wipe, then type:

- **`WIPE`** to delete the account: keys, contacts and history are erased from this device, with no undo. Your contacts get a goodbye (their chat with you becomes read-only), and the relay deletes your queues. Without a connection, the app asks whether to wipe without telling them.
- **`MOVED`** after restoring on another device: erases only this device and tells nobody.

The duress password never says goodbye: it wipes silently.

Files you saved from chats live outside termchat.app; a wipe does not erase them.
