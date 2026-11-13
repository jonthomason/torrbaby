#!/bin/bash
# The direct connection's in-app half (Apple/Direct, Rust): static libraries for iPhone/iPad
# and the simulator, linked by the iOS app. Needs rustup with the aarch64-apple-ios and
# aarch64-apple-ios-sim targets (Homebrew's rustc has no iOS standard library).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TOOLCHAIN="$(dirname "$(rustup which rustc)")"
cd "$ROOT/Apple/Direct"
for pair in aarch64-apple-ios:iphoneos aarch64-apple-ios-sim:iphonesimulator; do
    target="${pair%%:*}"; sdk="${pair##*:}"
    RUSTC="$TOOLCHAIN/rustc" "$TOOLCHAIN/cargo" build --release --locked --target "$target" -q 2>/dev/null \
        || RUSTC="$TOOLCHAIN/rustc" "$TOOLCHAIN/cargo" build --release --target "$target" -q
    mkdir -p "lib/$sdk"
    cp "target/$target/release/libtbdirect.a" "lib/$sdk/libtbdirect.a"
done
