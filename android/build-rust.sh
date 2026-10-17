#!/bin/sh
# Builds libtermchat_ffi.so for the phone and the emulator, and its Kotlin
# bindings, into app/build/rust (ADR-011).
set -eu
cd "$(dirname "$0")/.."
out=android/app/build/rust
profile=${PROFILE:-release}
cargo ndk -t arm64-v8a -t x86_64 -o "$out/jniLibs" build -p termchat-ffi --profile "$profile"
# The release library is stripped of UniFFI's metadata; a host debug build has the same.
cargo build -q -p termchat-ffi
lib=target/debug/libtermchat_ffi.so
[ "$(uname)" = Darwin ] && lib=target/debug/libtermchat_ffi.dylib
cargo run -q -p termchat-ffi --features bindgen --bin uniffi-bindgen -- generate \
    --library "$lib" --language kotlin --no-format --out-dir "$out/kotlin"
