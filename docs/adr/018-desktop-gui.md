# ADR-018: the desktop GUI client

Status: **accepted** (maintainer, 2026-11-06). Русская версия: [018-desktop-gui.ru.md](018-desktop-gui.ru.md).

## Context

On the desktop termchat runs only in a terminal. Not everyone likes a
terminal interface. ADR-011 already split the client into `crates/engine`
(everything except drawing) and thin shells: the TUI and the Android app. A
desktop window is a third shell over the same engine. The crypto, the
protocol, the state file and the relay do not change.

The maintainer chose (2026-11-06) to write it in Rust, not to port the
Android Compose UI to Compose Desktop.

## Decision

### 1. A new crate, a separate binary
- `crates/desktop`, binary `termchat-gui`, a shell over `termchat-engine`
  like `crates/client`.
- The TUI binary stays as it is and does not pull any GUI dependency.
- Code both shells need moves from `crates/client` into `engine`:
  - `settings.rs` (ADR-008 settings file);
  - the sound part of `notify.rs`;
  - the auto-lock constant.
  The TUI behaviour must not change.
- `#![forbid(unsafe_code)]` as in the rest of the workspace.

### 2. Toolkit: egui (eframe)
| | egui / eframe | iced | slint |
|---|---|---|---|
| Licence | MIT / Apache-2.0 | MIT | GPLv3, royalty-free or commercial |
| Screen readers | yes (AccessKit) | no, at the time of writing | yes |
| UI code | plain Rust | plain Rust (Elm style) | own `.slint` language and compiler |
| Renderer | OpenGL (`glow`) or wgpu | wgpu or tiny-skia | own |

egui is chosen: plain Rust, permissive licence, screen reader support, and the
smallest amount of UI code for a chat window. It does not look native; it gets
termchat's own look anyway (section 5). The `glow` renderer is used, not wgpu,
for a smaller dependency tree. Exact features and versions are fixed when the
dependency is added and checked with `cargo deny`.

**Rejected.**
- Tauri or any webview: an HTML engine inside the messenger, a large attack
  surface and against the spirit of ADR-017 (no rendering).
- GTK (gtk4-rs): needs GTK installed on macOS and Windows.
- Compose Desktop: a JVM runtime in every archive (maintainer's decision).

### 3. On the computer
- **State.** The same encrypted state file and `settings.json` in
  `~/.termchat/`. The GUI and the TUI can open the same account, but not at
  the same time: two running clients would each advance the Olm sessions and
  overwrite each other's state. Today nothing prevents that (two TUIs can
  already do it). The engine takes an exclusive lock on the state file
  (`std::fs::File::try_lock`, no new dependency) and the second client
  reports "termchat is already open".
- **Password.** As in the TUI, the password is the only key. The OS keychain
  is not used, so a backup restores anywhere.
- **Window title.** Always just `termchat.app`, never a contact name: the title
  is visible to other apps, the taskbar and window switchers.
- **Lock.** Locks after 5 minutes idle, like the TUI. Minimising the window
  does not lock it.
- **Notifications.** First version: the in-window card and the sound from
  ADR-008 only. No system notifications: ADR-008 already keeps them off by
  default because the OS keeps their history. They can come later with their
  own decision.
- **Clipboard.** Only explicit copy and paste in text fields (egui handles
  it). termchat does not copy anything to the clipboard by itself.
- **Documents (ADR-017).** Saved and picked through a file dialog. No
  preview, no opening, no thumbnails.
- **Tor.** Arti through the engine's `tor` feature, lyrebird in the archives,
  as for the TUI (ADR-004, ADR-015).

### 4. What it does not do
The same limits as the other clients (rules 9, 14, 15): no rendering of
message text, no link previews, no images, no calls, no contact book, no
search for users.

### 5. Look
The Android look (ADR-011 section 5): near-black background, monospace
messages, lime accent, `ENCRYPTED · 75392` / `NOT VERIFIED · 75392` header,
the same bubbles and input row. The TUI themes from `engine::themes` are
offered as well.

### 6. Distribution
- Archives for macOS, Linux and Windows on GitHub releases, with build
  attestations, next to the TUI archives.
- **No paid signing** (maintainer, 2026-11-06). No Apple Developer ID
  ($99/year, no fee waiver for individuals) and no Windows code signing
  certificate. Builds are only ad-hoc signed on macOS, which the Rust linker
  already does and Apple Silicon requires.
- **The main way is the existing installer**, `scripts/install.sh` and
  `scripts/install.ps1`. They already check the archive against `SHA256SUMS`.
  For the GUI they also:
  - macOS: put `termchat.app` into `~/Applications`;
  - Linux: put `termchat-gui` into `~/.local/bin` and a `.desktop` entry with
    an icon into `~/.local/share`;
  - Windows: add a Start menu entry.
  Files fetched by `curl` carry no quarantine attribute, so Gatekeeper does not
  stop the first start. Herdr installs its app the same way (checked
  2026-11-06); unlike Herdr, termchat does not update itself (ADR-010: a
  notice only).
- **Homebrew.** A `termchat-gui` formula in our tap. Formulae are not
  quarantined either. Official Homebrew casks need notarisation, so no cask.
- **A plain download** of the macOS archive from the release page works too,
  but macOS refuses the first start. On macOS 15 and later the user opens
  System Settings → Privacy & Security → "Open Anyway" once. The README says
  so. No `.dmg` and no "Download for Mac" button: without notarisation it
  would lead straight into that refusal.
- **Windows.** An unsigned `.exe` shows a SmartScreen warning ("More info" →
  "Run anyway") when downloaded through a browser; the README says so.
- Flatpak and winget later, as separate steps.

### 7. New dependencies
- `eframe` / `egui` with the `glow` and `accesskit` features.
- `rfd` for the native file dialog (documents, backups), if egui's own
  dialog is not enough.
Each one is justified in its commit and checked with `cargo deny`.

## Decided with the maintainer (2026-11-06)
1. **No screen capture protection.** Android uses `FLAG_SECURE`. On the
   desktop the equivalents (`NSWindow.sharingType` on macOS,
   `SetWindowDisplayAffinity` on Windows; nothing on Linux/X11) need platform
   calls with `unsafe` or one more dependency, and recent macOS capture APIs
   do not always respect them. The GUI does not try; the README says that
   screenshots and screen recording see the window.
2. **No paid signing**, see section 6.

## Settled while building it (2026-11-06)
- **Linux builds use glibc 2.28, not musl.** A static musl program cannot load
  the system's OpenGL, X11 and Wayland libraries. The GUI is built for
  `*-unknown-linux-gnu` with glibc 2.28 (Debian 10, Ubuntu 20.04, RHEL 8) and
  loads libwayland at run time, like X11 (`winit/wayland-dlopen`), so it links
  to nothing but glibc. Its x86_64 build is checked for reproducibility like
  the terminal client. The terminal client stays static musl.
- **Archives** are `termchat-gui-<tag>-<target>`, with lyrebird. macOS holds
  `termchat.app`, ad-hoc signed by `codesign --sign -` (free; it does not
  replace notarisation). Linux holds a `.desktop` entry and an icon.
- **Installers**: `TERMCHAT_GUI=1` makes `install.sh` and `install.ps1` install
  the app instead of the terminal client. The release workflow runs both ways.
- **Homebrew**: the `termchat-gui` formula keeps lyrebird next to the program,
  out of `bin`, so it does not clash with the terminal client's.
- **Windows**: no console window behind the app; the `.exe` has no icon of its
  own yet (that needs a resource compiler, a later step).
- **Settings**: `settings.json` holds `desktop_theme` beside the terminal's
  `theme`, so each keeps its own look.
- **Shared code** moved into the engine as the GUI needed it: settings, the
  sound, auto-lock, the core dump guard, the state lock, short time labels,
  path expansion, reading a document to send, the release notes.

## Metadata, honestly

The GUI talks to the relay exactly like the TUI: nothing new for the relay or
a network observer.

A GUI leaves more traces on the computer than a terminal:
- the OS knows a `termchat-gui` window exists and when it is in front;
- text passes through the toolkit, the GPU driver and the window system,
  which keep their own copies in memory; `Zeroize` on our buffers does not
  reach them;
- screenshots and screen recording see the window (see above).

The operating system and other installed programs are outside the threat
model, as for the TUI and Android.
