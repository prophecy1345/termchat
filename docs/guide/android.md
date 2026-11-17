---
title: "termchat.app on Android: install, notifications, battery, privacy"
description: "The termchat.app Android app without Google services: installing the APK, UnifiedPush wake-ups or staying connected, locking, screenshots, and updates."
---

# Android

Русская версия: [android.ru.md](android.ru.md).

The Android app needs Android 8.0 or newer and no Google services. [Install](install.md) the APK from the releases; an F-Droid build is on its way with the same signature.

## The screens

- **Chats:** your contacts, newest on top. ⋮ holds **Your invite**, **Add contact**, **Settings**, **What's new** and **Lock**; **+** also adds a contact.
- **A chat:** ⋮ holds **Search**, **Safety number**, **Rename**, **Disappearing**, **No history**, **Block**, **Report to the relay operator** and **Delete**. Below are the timer (the drop), 📎 for a file, the field and **Send**.
- **Long press** a message to answer, react, copy it, and edit or delete your own; see [Messages](messages.md).
- **Your invite** shows a QR code for another phone or the desktop app to scan; **Add contact** has **Scan QR code**. See [Contacts](contacts.md).

## Messages while the app is closed

Messages arrive while the app is open. For new messages while it is closed, pick one way in **Settings → Notifications**:

- **Wake-ups through UnifiedPush** (lighter on battery). Install a distributor such as [ntfy](https://ntfy.sh) from F-Droid or Google Play, turn its battery optimisation off, then turn wake-ups on in termchat.app. When a message waits, the relay pings your push server, and the phone shows "New message". The relay keeps your push address; the push server sees when you get messages, not from whom.
- **Stay connected in the background.** No extra app, but it costs battery, shows a permanent "termchat.app is connected" notice and keeps your keys in memory: the app then no longer locks itself in the background.

**New messages** turns the notification on or off. It always says just "termchat: New message", with no name and no text, because Android keeps a history of notifications outside the app.

## Privacy on the phone

- The app's screen cannot be captured: screenshots and the recent-apps preview stay blank.
- It locks after 1, 5, 15 or 30 minutes without a touch (**Settings → Lock after**), and at once with ⋮ → **Lock**.
- Messages stay in the app. Copy (from the long press) marks the clip as sensitive, so keyboards and the clipboard preview keep it out of sight.

## Updates

The chat list shows a note when a newer version is out. Install the new APK over the old one; your data stays.
