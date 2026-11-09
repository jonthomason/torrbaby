#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
export JAVA_HOME="${JAVA_HOME:-/Applications/Android Studio.app/Contents/jbr/Contents/Home}"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
export ANDROID_AVD_HOME="$ROOT/.local/avd"
mkdir -p "$ANDROID_AVD_HOME"
if [ ! -f "$ANDROID_AVD_HOME/Torrbaby-TV.ini" ]; then
  printf 'no\n' | "$ANDROID_HOME/cmdline-tools/latest/bin/avdmanager" create avd -n Torrbaby-TV -k 'system-images;android-36;android-tv;arm64-v8a' -d tv_1080p -p "$ANDROID_AVD_HOME/Torrbaby-TV.avd"
fi
exec "$ANDROID_HOME/emulator/emulator" -avd Torrbaby-TV -no-snapshot -no-boot-anim -gpu auto -memory 2048 "$@"
