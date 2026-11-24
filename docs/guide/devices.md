---
title: "termchat.app on a second device: link a laptop to your phone"
description: "Use one termchat.app identity on up to four devices: link a new device with a code and a fingerprint check, give devices labels, remove a lost one. What moves over, what does not, and what the relay sees."
---

# Devices

Русская версия: [devices.ru.md](devices.ru.md).

One identity can live on up to four devices, for example your phone and your laptop. Contacts see one person with one fingerprint, and their messages reach every device. Each device has keys of its own; the first one, your **main device**, vouches for the others ([ADR-028](https://github.com/prophecy1345/termchat/blob/main/docs/adr/028-second-device.md)).

## Link a new device

1. Install termchat.app on the new device. On its first screen, choose a password for this device and link instead of creating an identity; no nickname is asked, your main one's comes over:
   - terminal: **Ctrl+L**;
   - desktop: **Link to my main device instead**;
   - Android: **Link to my main device**.

   The link code appears as soon as the relay is reached. A device already set up, with no contacts on it, asks for one from its menu: **Ctrl+P**, then **l** in the terminal; **Settings**, **Link to my main device** on the desktop; **⋮**, **Link to my main device** on Android.
2. On your main device, open the devices and add it:
   - terminal: **Ctrl+P**, **v**, then **a**, and paste the code;
   - desktop: **Settings**, **Devices**, **Add a device**;
   - Android: **⋮**, **Devices**, then scan its QR code or paste the code.
3. Both screens now show one fingerprint. **Go on only if it is the same on both**: someone who copied the link code would show another one. Confirm on both devices.
4. The new device says "This device is linked", takes your nickname and gets your contacts.

After the update to 0.17, link your other devices again: device lists now carry each device's own proof (ADR-030), and older lists are dropped. On each linked device, erase it (wipe, MOVED) and link it from the first screen; its chats start empty, the main device keeps its history.

## What moves over, and what does not

- **Your contacts move**, with your names for them and their verified marks. Each device then talks to each contact on its own.
- **History does not.** A new device starts with empty chats; older messages stay on the devices that have them.
- **Incognito contacts stay** on the main device: each is an identity of its own.
- **Groups** reach every device: accept an invite on one, and your other devices join too. New groups are made on the main device. If someone in a group runs an older version, it cannot read what your linked devices write there.
- **Documents** reach every device of the contact. Your other devices show that you sent one, but the file itself stays on the device that sent it.

## What you see on each device

- Messages from a contact appear on every device.
- What you send from one device appears on your other devices as yours.
- A contact whose app is older than 0.12 hears only your main device. Writing to them from a linked device says they need to update first.

## Labels, removing, losing a device

- **Labels**, like "phone" or "laptop", are set on the main device: in the device list, **r** in the terminal, **Rename** on the desktop and on Android. Only your own devices see them, never your contacts.
- **Removing a device**: on the main device, in the device list, **x** in the terminal, **Remove** on the desktop and on Android. The removed device keeps what it already had, says so, and reads and sends nothing more. Contacts stop writing to it.
- **A lost linked device**: remove it from the main one, and link another. Backups are made on the main device only; a linked one does not ask for one.
- **A lost main device**: the linked devices keep working, but devices can no longer be added or removed. Restore the main device from its backup ([Backups, locking, wiping](safety.md)); its sessions with contacts start anew.
- **The duress password** wipes only the device it is typed on.

## What others learn

- **Contacts** see how many devices you have and their keys, never their labels.
- **The relay** sees that a message to you is several writes at once, one per device, and can tie those queues together as one person's devices. Without Tor, IP addresses tie your devices to each other too. It still cannot read anything. The [threat model](https://termchat.app/threat-model/) has the details.
