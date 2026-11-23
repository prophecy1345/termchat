---
title: "termchat.app on Android: install, notifications, battery, privacy"
description: "The termchat.app Android app without Google services: installing the APK, UnifiedPush wake-ups or staying connected, locking, screenshots, and updates."
---

# Android

Русская версия: [android.ru.md](android.ru.md).

The Android app needs Android 8.0 or newer and no Google services. [Install](install.md) the APK from the releases; an F-Droid build is on its way with the same signature.

## The screens

- **Chats:** archived chats first, then your contacts (pinned on top, then the newest), then your groups; tap a heading to fold its section. ⋮ holds **Your invite**, **Add contact**, **New group**, **Devices** (see [Devices](devices.md)), **Settings**, **What's new** and **Lock**, and on a new device **Link to my main device**; **+** also adds a contact.
- **A chat:** ⋮ holds **Search**, **Safety number**, **Rename**, **Disappearing**, **No history**, **Block**, **Report to the relay operator** and **Delete**. Below are the disappearing timer (a crumbling envelope: a tap moves to the next time and says it), the paperclip for a file, the field and **Send**.
- **Share from another app:** pick termchat.app in the system's share sheet, then a chat or group. Text goes into the message field for you to send; a file (up to 1 MiB) is sent as soon as you pick the chat. termchat.app never opens files.
- **Long press** a chat or group in the list for its menu without opening it: **Rename**, **Mute**, **Pin**, **Archive**, **Block** and **Delete** for a contact, **Mute** and **Leave** for a group.
- **Long press** a message to answer, react, copy it, and edit or delete your own; see [Messages](messages.md).
- **Your invite** shows a QR code for another phone or the desktop app to scan; **Add contact** has **Scan QR code**. See [Contacts](contacts.md).

## Groups

A small private group of up to 32 devices (32 people with one each; fewer when members use several), by invitation only. Nobody can find it.

- **Create:** ⋮ → **New group**, then a name. You are its admin.
- **Invite:** ⋮ → **Invite** in the group. Only your own contacts, on a version that knows groups; an invited one shows **Invited** with a check mark.
- **Join:** the contact shows **GROUP INVITE** in the list; their chat has **Join** and **Decline**.
- **In the group:** each message shows who wrote it, each name in a colour of its own; answers, edits, deletes and reactions work as in a chat. Only the admin sets the timer (⋮ → **Disappearing**).
- **Members:** tap the group's name, or ⋮ → **Members**. The admin removes people there (**Remove**): the group moves to a new queue, and they read nothing from then on.
- **Leave:** ⋮ → **Leave**. The others are told; the chat stays, read-only.

A new member does not see what was said before they joined. Documents go to the group as in a chat (up to 1 MiB, saved only by hand).

## Messages while the app is closed

Messages arrive while the app is open. For new messages while it is closed, pick one way in **Settings → Notifications**:

- **Wake-ups through UnifiedPush** (lighter on battery). Install a distributor such as [ntfy](https://ntfy.sh) from F-Droid or Google Play, turn its battery optimisation off, then turn wake-ups on in termchat.app. When a message waits, the relay pings your push server, and the phone shows "New message". The relay keeps your push address; the push server sees when you get messages, not from whom.
- **Stay connected in the background.** No extra app, but it costs battery, shows a permanent "termchat.app is connected" notice and keeps your keys in memory: the app then no longer locks itself in the background.

**New messages** turns the notification on or off. It always says just "termchat.app: New message", with no name and no text, because Android keeps a history of notifications outside the app.

## Privacy on the phone

- The app's screen cannot be captured: screenshots and the recent-apps preview stay blank.
- It locks after 1, 5, 15 or 30 minutes without a touch (**Settings → Lock after**), and at once with ⋮ → **Lock**.
- Messages stay in the app. Copy (from the long press) marks the clip as sensitive, so keyboards and the clipboard preview keep it out of sight.

## Updates

The chat list shows a note when a newer version is out. Install the new APK over the old one; your data stays.
