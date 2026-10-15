#!/bin/sh
# Reproducible release build (PLAN.md, M6). Same toolchain (rust-toolchain.toml),
# same Cargo.lock, and paths remapped so the checkout location and the user's
# home do not end up in the binary. Usage: scripts/build-release.sh [target]
set -eu
cd "$(dirname "$0")/.."
target=${1:-$(rustc -vV | sed -n 's/^host: //p')}
build=${BUILD:-build}
export RUSTFLAGS="--remap-path-prefix=$PWD=/termchat --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo ${RUSTFLAGS:-}"
export SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
cargo "$build" --release --locked --target "$target" -p termchat -p termchat-relay
