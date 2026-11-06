#!/bin/sh
# Packs the desktop GUI (ADR-018), already built by build-release.sh, into
# <dir>.tar.gz (<dir>.zip on Windows), with lyrebird next to it for Tor
# bridges (ADR-015). macOS gets termchat.app, ad-hoc signed: no paid
# signature (ADR-018, section 6). Linux gets a .desktop entry and an icon.
# Usage: scripts/package-gui.sh <target> <dir> <lyrebird binary>
set -eu
cd "$(dirname "$0")/.."
target=$1
dir=$2
lyrebird=$3
icon=crates/desktop/assets/termchat.png
case $target in *windows*) exe=.exe ;; *) exe= ;; esac
bin=target/$target/release/termchat-gui$exe
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
rm -rf "$dir"
mkdir -p "$dir"
cp LICENSE README.md "$dir/"
cp third_party/lyrebird/LICENSE "$dir/LICENSE.lyrebird"
cp third_party/lyrebird/LICENSE-GPL3.txt "$dir/LICENSE-GPL3.lyrebird.txt"
sum() { if command -v sha256sum >/dev/null; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }

case $target in
*apple-darwin)
    app=$dir/termchat.app
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp "$bin" "$app/Contents/MacOS/termchat-gui"
    cp "$lyrebird" "$app/Contents/MacOS/lyrebird"
    set_dir=$(mktemp -d)/termchat.iconset
    mkdir -p "$set_dir"
    for s in 16 32 128 256; do
        sips -z $s $s "$icon" --out "$set_dir/icon_${s}x${s}.png" >/dev/null
        sips -z $((s * 2)) $((s * 2)) "$icon" --out "$set_dir/icon_${s}x${s}@2x.png" >/dev/null
    done
    cp "$icon" "$set_dir/icon_512x512.png"
    iconutil -c icns "$set_dir" -o "$app/Contents/Resources/termchat.icns"
    cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>termchat-gui</string>
  <key>CFBundleIdentifier</key><string>app.termchat.desktop</string>
  <key>CFBundleName</key><string>termchat.app</string>
  <key>CFBundleDisplayName</key><string>termchat.app</string>
  <key>CFBundleIconFile</key><string>termchat</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
    codesign --force --deep --sign - "$app"
    (cd "$app/Contents/MacOS" && sum termchat-gui lyrebird) > "$dir/BINARIES.sha256"
    ;;
*linux*)
    cp "$bin" "$lyrebird" "$icon" "$dir/"
    # The installer points Exec at where it put the program.
    cat > "$dir/termchat.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=termchat.app
Comment=Private, end-to-end encrypted messenger
Exec=termchat-gui
Icon=termchat
Categories=Network;InstantMessaging;
Terminal=false
StartupWMClass=termchat
DESKTOP
    ;;
*windows*) cp "$bin" "$lyrebird" "$dir/" ;;
esac
[ -f "$dir/BINARIES.sha256" ] || (cd "$dir" && sum "termchat-gui$exe" "lyrebird$exe") > "$dir/BINARIES.sha256"

# The archive holds the folder by its name alone, wherever it was built.
cd "$(dirname "$dir")"
case $target in
*windows*) 7z a -tzip "$(basename "$dir").zip" "$(basename "$dir")" >/dev/null ;;
*) tar -czf "$(basename "$dir").tar.gz" "$(basename "$dir")" ;;
esac
