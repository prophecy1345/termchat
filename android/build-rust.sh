#!/bin/sh
# Builds libtermchat_ffi.so for the phone and the emulator, and its Kotlin
# bindings, into app/build/rust (ADR-011).
set -eu
cd "$(dirname "$0")/.."
out=android/app/build/rust
profile=${PROFILE:-release}
dir=$profile
[ "$profile" = dev ] && dir=debug
cargo ndk -t arm64-v8a -t x86_64 -o "$out/jniLibs" build -p termchat-ffi --profile "$profile"
cargo run -q -p termchat-ffi --features bindgen --bin uniffi-bindgen -- generate \
    --library "target/aarch64-linux-android/$dir/libtermchat_ffi.so" \
    --language kotlin --no-format --out-dir "$out/kotlin"
