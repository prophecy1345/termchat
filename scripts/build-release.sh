#!/bin/sh
# Reproducible release build. Same toolchain (rust-toolchain.toml),
# same Cargo.lock, and paths remapped so the checkout location and the user's
# home do not end up in the binary. Usage: scripts/build-release.sh [target]
# PACKAGES picks what to build: the terminal client and the relay by default,
# "-p termchat-desktop" for the GUI (ADR-018).
set -eu
cd "$(dirname "$0")/.."
target=${1:-$(rustc -vV | sed -n 's/^host: //p')}
build=${BUILD:-build}
export RUSTFLAGS="--remap-path-prefix=$PWD=/termchat --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo ${RUSTFLAGS:-}"
export SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
# Arti's directory client uses liblzma. Build it from its source: a system copy
# found through pkg-config ties the binary to the build machine's glibc (and
# to whatever that machine has installed), so the GUI's glibc 2.28 link fails.
export LZMA_API_STATIC=1
# shellcheck disable=SC2086 # a list of -p flags
cargo "$build" --release --locked --target "$target" ${PACKAGES:--p termchat -p termchat-relay}
