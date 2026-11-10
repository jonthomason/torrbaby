#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
export JAVA_HOME="${JAVA_HOME:-/Applications/Android Studio.app/Contents/jbr/Contents/Home}"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
python3 "$ROOT/../scripts/check_localizations.py"
python3 "$ROOT/tools/check_tvos_parity.py"
if [ "$#" -eq 0 ]; then set -- assembleDebug; fi
exec "$ROOT/gradlew" -p "$ROOT" "$@"
