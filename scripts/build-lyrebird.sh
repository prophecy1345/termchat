#!/bin/sh
# Builds lyrebird, the Tor bridge transport (ADR-015), from the pinned source
# in third_party/lyrebird: build-lyrebird.sh <GOOS> <GOARCH> <output file>.
# Reproducible: an exact Go toolchain, module hashes from go.sum, no paths,
# VCS stamp or build id in the binary.
set -eu
out=$(cd "$(dirname "$3")" && pwd)/$(basename "$3")
cd "$(dirname "$0")/../third_party/lyrebird"
cgo=0
if [ "$1" = android ]; then
    # Bionic's resolver: a static Go binary finds no DNS on Android, and
    # webtunnel and snowflake need it.
    ndk=${ANDROID_NDK_HOME:?ANDROID_NDK_HOME is not set}
    host=$(ls "$ndk/toolchains/llvm/prebuilt")
    triple=aarch64-linux-android
    [ "$2" = amd64 ] && triple=x86_64-linux-android
    export CC="$ndk/toolchains/llvm/prebuilt/$host/bin/${triple}26-clang"
    export CGO_CFLAGS="-ffile-prefix-map=$ndk=/ndk"
    cgo=1
fi
# anet (snowflake) needs -checklinkname=0 with Go 1.23 and later.
GOTOOLCHAIN=go1.27.2 GOFLAGS=-mod=readonly CGO_ENABLED=$cgo GOOS=$1 GOARCH=$2 \
    go build -trimpath -buildvcs=false -ldflags="-s -w -buildid= -checklinkname=0" \
    -o "$out" ./cmd/lyrebird
