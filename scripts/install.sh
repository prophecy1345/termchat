#!/bin/sh
# termchat installer for macOS and Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.sh | sh
#
# Downloads the release archive for this system, checks it against the
# release's SHA256SUMS, and installs the `termchat` binary, with lyrebird next
# to it for Tor bridges (ADR-015). Nothing else is
# changed. The checksum catches broken or truncated downloads; to also check
# who built the archive, use `gh attestation verify` (README, "Verify a release").
#
# Settings (environment variables):
#   TERMCHAT_VERSION      release tag, e.g. v0.1.1 (default: latest)
#   TERMCHAT_INSTALL_DIR  where to put the binary (default: ~/.local/bin)
#   TERMCHAT_BASE_URL     download from here instead of GitHub (mirrors, tests)
set -eu

repo=prophecy1345/termchat
dest=${TERMCHAT_INSTALL_DIR:-$HOME/.local/bin}

case "$(uname -s)/$(uname -m)" in
    Darwin/arm64) target=aarch64-apple-darwin ;;
    Darwin/x86_64) target=x86_64-apple-darwin ;;
    Linux/x86_64 | Linux/amd64) target=x86_64-unknown-linux-musl ;;
    Linux/aarch64 | Linux/arm64) target=aarch64-unknown-linux-musl ;;
    *) echo "termchat: no build for $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac

tag=${TERMCHAT_VERSION:-latest}
if [ "$tag" = latest ]; then
    # GitHub redirects /releases/latest to /releases/tag/<tag>.
    tag=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest")
    tag=${tag##*/}
fi
base=${TERMCHAT_BASE_URL:-https://github.com/$repo/releases/download/$tag}
archive="termchat-$tag-$target.tar.gz"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
echo "Downloading $archive"
curl -fsSL "$base/$archive" -o "$tmp/$archive"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS"

# `*name` is how sha256sum marks binary mode on some systems.
expected=$(awk -v f="$archive" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")
if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp/$archive" | awk '{ print $1 }')
else
    actual=$(shasum -a 256 "$tmp/$archive" | awk '{ print $1 }')
fi
if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
    echo "termchat: checksum mismatch for $archive, nothing installed" >&2
    exit 1
fi

tar -xzf "$tmp/$archive" -C "$tmp"
mkdir -p "$dest"
install -m 755 "$tmp/termchat-$tag-$target/termchat" "$dest/termchat"
# Releases before 0.3.0 have no lyrebird.
lyrebird="$tmp/termchat-$tag-$target/lyrebird"
[ -f "$lyrebird" ] && install -m 755 "$lyrebird" "$dest/lyrebird"
echo "Installed termchat $tag to $dest/termchat"

case ":$PATH:" in
    *":$dest:"*) echo "Run: termchat" ;;
    *)
        echo "$dest is not in your PATH. Add this line to ~/.zshrc or ~/.bashrc:"
        echo "    export PATH=\"$dest:\$PATH\""
        echo "or run it directly: $dest/termchat"
        ;;
esac
