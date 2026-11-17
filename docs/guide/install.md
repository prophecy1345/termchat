---
title: "Install termchat.app: Homebrew, one command, Windows, Android"
description: "Install the termchat.app encrypted messenger on macOS, Linux, Windows and Android, update it, and check that a download is genuine."
---

# Install

Русская версия: [install.ru.md](install.ru.md).

termchat.app comes as three programs that open the same kind of account:

- `termchat`, for the terminal;
- `termchat-gui`, the desktop app, for those who prefer a window;
- the Android app.

Pick what you need. One account opens in one program at a time.

## Terminal client

**Homebrew** (macOS, Linux):

```sh
brew install prophecy1345/tap/termchat
```

**One command** (macOS, Linux). It downloads the build for your computer, checks it against the release's `SHA256SUMS` and puts `termchat` into `~/.local/bin`:

```sh
curl -fsSL https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.sh | sh
```

If `~/.local/bin` is not in your `PATH`, the script prints the line to add to `~/.zshrc` or `~/.bashrc`. Another folder: put `TERMCHAT_INSTALL_DIR=/some/folder` before `sh`.

**One command** (Windows, in PowerShell). It installs to `%LOCALAPPDATA%\Programs\termchat` and adds that folder to your `PATH`:

```powershell
irm https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.ps1 | iex
```

Then open a new terminal and run `termchat`. `termchat --version` prints the version.

## Desktop app

The same account and encryption in a window, for macOS, Linux and Windows.

```sh
# macOS: termchat.app goes into ~/Applications.
# Linux: termchat-gui goes into ~/.local/bin, with a menu entry.
curl -fsSL https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.sh | TERMCHAT_GUI=1 sh

# or with Homebrew
brew install prophecy1345/tap/termchat-gui
```

```powershell
# Windows: next to termchat.exe, with a Start menu entry
$env:TERMCHAT_GUI = 1; irm https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.ps1 | iex
```

The app is not signed with a paid Apple or Microsoft certificate. Installed as above, it opens normally. Downloaded through a browser instead:

- macOS refuses the first start. Open System Settings → Privacy & Security and click "Open Anyway" once.
- Windows SmartScreen asks once too: "More info" → "Run anyway".

On Linux the app needs glibc 2.28 or newer (Debian 10, Ubuntu 20.04, RHEL 8) and X11 or Wayland.

A window shows more to the rest of the computer than a terminal does: screenshots and screen recording see it, and the text passes through the window system and the graphics driver.

## Android

Android 8.0 or newer; no Google services needed.

1. Download `termchat-<version>-android-arm64.apk` from the [releases](https://github.com/prophecy1345/termchat/releases).
2. Open it. Android asks once to allow installs from your browser.

The app shows a note in the chat list when a newer version is out. An F-Droid build is on its way, with the same signature.

## By hand

Download the archive for your computer from the [releases](https://github.com/prophecy1345/termchat/releases). Each holds `termchat` and `termchat-relay`, the server.

| Your computer | Archive |
|---|---|
| Mac with Apple Silicon (M1 and later) | `termchat-<version>-aarch64-apple-darwin.tar.gz` |
| Mac with Intel | `termchat-<version>-x86_64-apple-darwin.tar.gz` |
| Linux, 64-bit PC | `termchat-<version>-x86_64-unknown-linux-musl.tar.gz` |
| Linux on ARM (Raspberry Pi 4/5, ARM servers) | `termchat-<version>-aarch64-unknown-linux-musl.tar.gz` |
| Windows 10/11 | `termchat-<version>-x86_64-pc-windows-msvc.zip` |

- **Linux:** `tar -xzf termchat-*-linux-musl.tar.gz`, then `./termchat` in that folder. The binary is static: nothing else to install.
- **macOS:** `tar -xzf termchat-*-apple-darwin.tar.gz`, then `./termchat` in that folder. If macOS says it "cannot be opened because the developer cannot be verified" (it does for files downloaded with a browser), run `xattr -d com.apple.quarantine termchat` once.
- **Windows:** extract the `.zip` and double-click `termchat.exe`; Windows Terminal looks best. SmartScreen may warn about an unknown publisher: "More info" → "Run anyway".

## Updating

- Homebrew: `brew update && brew upgrade termchat` (and `termchat-gui`). Without `brew update`, Homebrew may not see a new version for up to a day.
- One command: run it again.
- Android: install the new APK over the old one; your data stays.

The apps tell you when the relay runs a newer version than yours.

## Check that a download is genuine

Do this before the first run if it matters to you.

1. **Checksum.** In the folder with the downloads and the release's `SHA256SUMS`:

   ```sh
   shasum -a 256 -c SHA256SUMS --ignore-missing    # macOS
   sha256sum -c SHA256SUMS --ignore-missing        # Linux
   ```

   Every file you downloaded must say `OK`.

2. **Provenance.** With the GitHub CLI:

   ```sh
   gh attestation verify termchat-<version>-<target>.tar.gz --repo prophecy1345/termchat
   ```

   This proves the file was built by this repository's release workflow from a known commit (Sigstore; no keys to trust). It works for the archives, the APK and the Android bundle.

3. **Android signature.** The APK must be signed with this certificate (`apksigner verify --print-certs <apk>`, or the app's details in Android), SHA-256:

   `9F:FB:61:1C:DD:1B:3A:AE:63:30:3F:B7:12:0B:85:F7:67:0D:08:BD:B8:24:D6:7C:7C:85:37:09:88:0C:2A:7F`

4. **Rebuild it yourself.** The Linux builds and the Android app are reproducible: every release is built twice on separate machines, and it is published only if the bytes match. To check a Linux build yourself you need [zig](https://ziglang.org) and `cargo install cargo-zigbuild`:

   ```sh
   git clone https://github.com/prophecy1345/termchat && cd termchat
   git checkout v<version> && git submodule update --init
   BUILD=zigbuild scripts/build-release.sh x86_64-unknown-linux-musl
   sha256sum target/x86_64-unknown-linux-musl/release/termchat target/x86_64-unknown-linux-musl/release/termchat-relay
   ```

   The hashes must match `BINARIES.sha256` inside the archive. The macOS and Windows builds are not reproducible yet: a rebuild there gives other bytes.

Dependencies are checked with `cargo deny` (licences, known vulnerabilities) and `cargo vet` (audits imported from Mozilla, Google and others; the rest is listed as not yet reviewed in `supply-chain/`).
