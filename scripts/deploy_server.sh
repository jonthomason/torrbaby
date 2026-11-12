#!/bin/bash
# Install a built server bundle on the server Mac without ever leaving it broken.
#
# The only way to deploy the server. Copying a bundle over the live one by hand once
# replaced it with a half-built bundle that had no server binary, and the server stayed
# down until a good build was made. Here the bundle is verified first, uploaded beside
# the live one, checked again on the server, swapped in with two renames, and put back
# automatically if the new server does not come up.
#
#   scripts/build_server.sh && scripts/deploy_server.sh
#   scripts/deploy_server.sh "path/to/Torrbaby Server.app"
#   scripts/deploy_server.sh --rollback        # put the previous bundle back
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HOST="${TORRBABY_SERVER_HOST:-diamond}"
SERVICE="org.torrbaby.daemon"
EXECUTABLES="torrbaby torrbaby-subtitle-translator torrbaby-subtitle-extractor torrbaby-media-segmenter torrbaby-subtitle-ocr torrbaby-credits-scanner torrbaby-speech-subtitles torrbaby-ffmpeg torrbaby-ffprobe"

# Runs on the server. Swaps bundles and proves the server answers, or undoes the swap.
remote() {
    ssh "$HOST" /bin/bash -s -- "$1" "$SERVICE" "$EXECUTABLES" <<'REMOTE'
set -euo pipefail
MODE="$1"; SERVICE="$2"; EXECUTABLES="$3"
cd "$HOME/Applications"
LIVE="Torrbaby Server.app"; NEW="$LIVE.new"; OLD="$LIVE.previous"; BAD="$LIVE.failed"
whole() { for name in $EXECUTABLES; do [[ -x "$1/Contents/MacOS/$name" ]] || { echo "Incomplete bundle on the server: $1 is missing $name" >&2; return 1; }; done; codesign --verify --deep --strict "$1"; }
restart() { launchctl kickstart -k "gui/$(id -u)/$SERVICE"; }
# Up means the new process is running and has opened its port.
up() {
    for _ in $(seq 1 45); do
        sleep 1
        if pgrep -f "$LIVE/Contents/MacOS/torrbaby serve" >/dev/null && lsof -nP -iTCP -sTCP:LISTEN 2>/dev/null | grep -q '^torrbaby .*:8765 '; then return 0; fi
    done
    return 1
}
if [[ "$MODE" == rollback ]]; then
    whole "$OLD" || { echo "No whole previous bundle to go back to" >&2; exit 1; }
    rm -rf "$BAD"; mv "$LIVE" "$BAD"; mv "$OLD" "$LIVE"; restart
    up && { echo "Rolled back; the server is up."; exit 0; }
    echo "The previous bundle did not start either" >&2; exit 1
fi
whole "$NEW" || { rm -rf "$NEW"; exit 1; }
rm -rf "$OLD"
if [[ -d "$LIVE" ]]; then mv "$LIVE" "$OLD"; fi
mv "$NEW" "$LIVE"
restart
if up; then echo "Deployed; the server is up."; exit 0; fi
echo "The new server did not come up; putting the previous one back." >&2
if whole "$OLD"; then
    rm -rf "$BAD"; mv "$LIVE" "$BAD"; mv "$OLD" "$LIVE"; restart
    up && { echo "Previous server restored. The failed bundle is kept as $BAD." >&2; exit 1; }
fi
echo "THE SERVER IS DOWN: neither bundle starts." >&2
exit 2
REMOTE
}

if [[ "${1:-}" == --rollback ]]; then remote rollback; exit; fi
APP="${1:-$ROOT/local-state/server/Torrbaby Server.app}"
[[ -d "$APP" ]] || { echo "No bundle at $APP; run scripts/build_server.sh first" >&2; exit 1; }
"$ROOT/scripts/verify_server_bundle.sh" "$APP"
# Beside the live bundle, never over it: the live one is untouched until this copy is whole.
rsync -a --delete "$APP/" "$HOST:Applications/Torrbaby\\ Server.app.new/"
remote deploy
