#!/bin/bash
# Required before installing a server bundle, including experimental builds.
set -euo pipefail
APP="${1:?Usage: verify_server_bundle.sh path/to/Torrbaby Server.app}"
for EXECUTABLE in torrbaby torrbaby-subtitle-translator torrbaby-subtitle-extractor torrbaby-media-segmenter torrbaby-subtitle-ocr; do
    if [[ ! -x "$APP/Contents/MacOS/$EXECUTABLE" ]]; then
        echo "Incomplete server bundle: missing $EXECUTABLE" >&2
        exit 1
    fi
done
codesign --verify --deep --strict "$APP"
