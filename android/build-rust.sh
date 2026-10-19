#!/bin/sh
# Builds libtermchat_ffi.so for the phone and the emulator, and its Kotlin
# bindings, into app/build/rust (ADR-011).
set -eu
cd "$(dirname "$0")/.."
out=android/app/build/rust
profile=${PROFILE:-release}
# Reproducible (F-Droid checks it): the checkout, cargo home and NDK paths are
# remapped in Rust and in the C code (SQLite, ring) alike.
cargo_home=${CARGO_HOME:-$HOME/.cargo}
ndk=${ANDROID_NDK_HOME:-/ndk}
RUSTFLAGS="--remap-path-prefix=$PWD=/termchat --remap-path-prefix=$cargo_home=/cargo ${RUSTFLAGS:-}" \
CFLAGS="-ffile-prefix-map=$PWD=/termchat -ffile-prefix-map=$cargo_home=/cargo -ffile-prefix-map=$ndk=/ndk ${CFLAGS:-}" \
SOURCE_DATE_EPOCH=$(git log -1 --format=%ct) \
    cargo ndk -t arm64-v8a -t x86_64 -o "$out/jniLibs" build -p termchat-ffi --features tor --profile "$profile" --locked
# lyrebird for Tor bridges (ADR-015), run by the app from its native library
# folder, the one place Android lets it start its own programs.
scripts/build-lyrebird.sh android arm64 "$out/jniLibs/arm64-v8a/liblyrebird.so"
scripts/build-lyrebird.sh android amd64 "$out/jniLibs/x86_64/liblyrebird.so"
# The release library is stripped of UniFFI's metadata; a host debug build has the same.
cargo build -q -p termchat-ffi
lib=target/debug/libtermchat_ffi.so
[ "$(uname)" = Darwin ] && lib=target/debug/libtermchat_ffi.dylib
cargo run -q -p termchat-ffi --features bindgen --bin uniffi-bindgen -- generate \
    --library "$lib" --language kotlin --no-format --out-dir "$out/kotlin"
