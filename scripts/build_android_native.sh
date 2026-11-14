#!/bin/bash
# Build the Android TV app's relay library (Android/native, a JNI shim over Apple/Direct) for
# armeabi-v7a (JMGO and other 32-bit TVs) and arm64-v8a, into the app's jniLibs. The Gradle
# build runs this before each build; cargo rebuilds only what changed.
#
# Needs the Android NDK (sdkmanager "ndk;29.0.14206865") and the Rust targets
# armv7-linux-androideabi and aarch64-linux-android from rustup (Homebrew's rustc has no Android
# standard library). CARGO_MIRROR=rsproxy fetches crates from
# rsproxy.cn when crates.io cannot be reached.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}}"
ndk="${ANDROID_NDK_HOME:-}"
if [ -z "$ndk" ]; then ndk="$(ls -d "$sdk"/ndk/* 2>/dev/null | sort -V | tail -1)"; fi
[ -d "$ndk" ] || { echo "No Android NDK under $sdk/ndk: install it with sdkmanager \"ndk;29.0.14206865\"" >&2; exit 1; }
host="$(ls "$ndk/toolchains/llvm/prebuilt" | head -1)"
bin="$ndk/toolchains/llvm/prebuilt/$host/bin"
api=26 # the app's minSdk
mirror=()
if [ "${CARGO_MIRROR:-}" = rsproxy ]; then
  mirror=(--config 'source.crates-io.replace-with="rsproxy"' --config 'source.rsproxy.registry="sparse+https://rsproxy.cn/index/"')
fi
out="$root/Android/app/src/main/jniLibs"
toolchain="$(dirname "$(rustup which rustc)")"
export RUSTC="$toolchain/rustc"
cd "$root/Android/native"
for pair in "armv7-linux-androideabi armeabi-v7a armv7a-linux-androideabi" "aarch64-linux-android arm64-v8a aarch64-linux-android"; do
  set -- $pair
  target=$1 abi=$2 clang="$bin/$3$api-clang"
  upper="$(echo "$target" | tr 'a-z-' 'A-Z_')"
  export "CARGO_TARGET_${upper}_LINKER=$clang"
  export "CC_${target//-/_}=$clang" "AR_${target//-/_}=$bin/llvm-ar"
  "$toolchain/cargo" ${mirror[@]+"${mirror[@]}"} build --quiet --release --locked --target "$target"
  mkdir -p "$out/$abi"
  cp -p "target/$target/release/libtbandroid.so" "$out/$abi/libtbandroid.so"
done
