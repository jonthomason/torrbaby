#!/bin/bash
# Required before installing a server bundle, including experimental builds.
set -euo pipefail
APP="${1:?Usage: verify_server_bundle.sh path/to/Torrbaby Server.app}"
for EXECUTABLE in torrbaby torrbaby-subtitle-translator torrbaby-subtitle-extractor torrbaby-media-segmenter torrbaby-subtitle-ocr torrbaby-credits-scanner torrbaby-speech-subtitles torrbaby-ffmpeg; do
    if [[ ! -x "$APP/Contents/MacOS/$EXECUTABLE" ]]; then
        echo "Incomplete server bundle: missing $EXECUTABLE" >&2
        exit 1
    fi
done
# Vision helpers share one signing identifier so they share compiled Neural
# Engine programs (see build_server.sh).
for EXECUTABLE in torrbaby-subtitle-ocr torrbaby-credits-scanner; do
    IDENTIFIER="$(codesign -dv "$APP/Contents/MacOS/$EXECUTABLE" 2>&1 | sed -n 's/^Identifier=//p')"
    if [[ "$IDENTIFIER" != "org.torrbaby.server.vision" ]]; then
        echo "$EXECUTABLE must be signed with identifier org.torrbaby.server.vision (found ${IDENTIFIER:-none})" >&2
        exit 1
    fi
done
codesign --verify --deep --strict "$APP"
