#!/bin/sh
# Prints the Homebrew formula for a release (docs: https://docs.brew.sh/Taps).
# Usage: scripts/homebrew-formula.sh <tag> <SHA256SUMS file>
# The release workflow attaches the result as termchat.rb; it lives in
# github.com/prophecy1345/homebrew-tap as Formula/termchat.rb.
set -eu
tag=$1
sums=$2
url="https://github.com/prophecy1345/termchat/releases/download/$tag"
sha() {
    s=$(awk -v f="termchat-$tag-$1.tar.gz" '$2 == f { print $1 }' "$sums")
    [ -n "$s" ] || { echo "no checksum for $1" >&2; exit 1; }
    echo "$s"
}
cat <<RUBY
class Termchat < Formula
  desc "Private, text-only, end-to-end encrypted messenger for the terminal"
  homepage "https://github.com/prophecy1345/termchat"
  license "AGPL-3.0-or-later"

  on_macos do
    on_arm do
      url "$url/termchat-$tag-aarch64-apple-darwin.tar.gz"
      sha256 "$(sha aarch64-apple-darwin)"
    end
    on_intel do
      url "$url/termchat-$tag-x86_64-apple-darwin.tar.gz"
      sha256 "$(sha x86_64-apple-darwin)"
    end
  end

  on_linux do
    on_arm do
      url "$url/termchat-$tag-aarch64-unknown-linux-musl.tar.gz"
      sha256 "$(sha aarch64-unknown-linux-musl)"
    end
    on_intel do
      url "$url/termchat-$tag-x86_64-unknown-linux-musl.tar.gz"
      sha256 "$(sha x86_64-unknown-linux-musl)"
    end
  end

  def install
    bin.install "termchat"
  end

  test do
    assert_match "termchat #{version}", shell_output("#{bin}/termchat --version")
  end
end
RUBY
