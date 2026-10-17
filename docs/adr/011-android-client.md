# ADR-011: the Android client

Status: **accepted** (maintainer, 2026-10-17). Русская версия: [011-android-client.ru.md](011-android-client.ru.md).

## Context

termchat runs only in a terminal. An Android app should do everything the
terminal client does (PLAN.md, "after the MVP") without a second
implementation of the crypto, the protocol or the state file. ADR-001 already
chose UniFFI for mobile; this ADR fixes the rest.

## Decision

### 1. One Rust engine, two shells
- New crate `crates/engine`: everything in today's client that is not drawing.
  - The relay transport (`client/src/lib.rs`).
  - The app data and the state file (`data.rs`).
  - Password rules (`password.rs`).
  - The logic now inside `app.rs::Main`: persist, transmit, receive, requests,
    disappearing timers, backup, passwords, duress, `WIPE`/`MOVED`, update
    notice.
  - It exposes an `Engine` with methods and typed events and errors instead of
    status strings.
- `crates/client` (TUI) becomes a shell over `engine`; its behaviour must not
  change.
- New crate `crates/ffi`: UniFFI bindings over `engine` for Kotlin. Tokio runs
  inside Rust; events reach Kotlin through a callback interface.
- `core` and `proto` do not change.

### 2. The Android app
- Kotlin + Jetpack Compose, in `android/`. minSdk 26 (Android 8).
- No Google Play Services and no Firebase, so it runs on de-Googled phones
  (GrapheneOS, LineageOS). Distributed as a signed APK on GitHub
  releases (with build attestations) and through F-Droid.

### 3. On the device
- **State.**
  - The same encrypted state file (ADR-002, ADR-003) lives in the app's
    private directory.
  - The duress password works the same way.
  - Android Keystore is not used: the password stays the only key, so a backup
    restores on any device.
- **Screen.**
  - `FLAG_SECURE` on every window: no screenshots, no previews in the recent
    apps list.
  - Text fields ask the keyboard not to learn (`IME_FLAG_NO_PERSONALIZED_LEARNING`).
- **Lock.** Locks after 5 minutes idle and when the app has been in the
  background for 5 minutes, like the TUI auto-lock.
- **Notifications.** The same rule as ADR-008: "New message", with no name and
  no text, and off unless turned on.
- **Backups.**
  - Written and read through the Storage Access Framework, so the user picks a
    USB stick or a folder.
  - The ADR-003 rule "a wipe erases known backups" applies.
  - A document reached through a content URI can be deleted but not
    overwritten first. The ADR states that difference.
- **TLS roots.** Android has no certificate bundle that `rustls-native-certs`
  can read. On Android the transport uses the `webpki-roots` feature of
  tokio-tungstenite (Mozilla's list, updated with each release).
- **Tor.** Arti as on the desktop (ADR-004), on by default at setup. Android's
  app folder is group-writable, which Arti's permission check refuses; on
  Android that check is off for the Tor folder, since the sandbox keeps other
  apps out. The library grows from about 5 to 13 MB.

### 4. What the app does not do
Text only (rule 9):
- no attachments, calls or video;
- no contact book access;
- no phone number.

The camera is used only to scan an invite QR code, and only after the user
taps "scan".

### 5. Look
termchat's own look, in the spirit of the terminal client:
- **Colours and type.** A near-black background, a monospace font for
  messages, status lines and the input, and a bold sans for names. The accent
  colour is lime.
- **Chat header:**
  - a square avatar with initials;
  - the name;
  - a status line in lime capitals with the short fingerprint, e.g.
    `ENCRYPTED · 75392`;
  - an unverified contact shows `NOT VERIFIED · 75392` in the warning colour.
- **Bubbles.** Ours are dark olive, theirs are dark grey, both with small
  corners.
- **System lines** (request accepted, left, restarted) sit centred in a thin
  bordered box.
- **Input row:**
  - a square timer button (the drop icon) that cycles the disappearing timer,
    like Ctrl+T;
  - a bordered field `Encrypted message`;
  - a flat `SEND` button.
- No attachment or call buttons: text only (rule 9).

The TUI themes stay; the Android look becomes a TUI theme too, so both
clients can look alike.

### 6. New dependencies (each justified when added)
- `uniffi` (bindings).
- The `webpki-roots` feature of tokio-tungstenite (Android TLS roots).
- AndroidX/Compose.
- `zxing-core` for drawing and decoding the QR code. It is pure Java, without
  Google ML Kit.

## Metadata, honestly

The app talks to the same relay in the same way: nothing new for the relay or
a network observer.

Android itself knows the app is installed and when it uses the network. The
operating system and the installed apps are outside our threat model, as for
the desktop.

Background delivery is a separate decision (ADR-012).
