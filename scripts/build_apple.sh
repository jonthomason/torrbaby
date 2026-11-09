#!/bin/bash
# Apple tooling only. Signing configuration stays in environment/local files.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PLATFORM="${1:-ios}"
BUILD_NUMBER="${TORRBABY_BUILD_NUMBER:-$(git -C "$ROOT" rev-list --count HEAD)}"
OUTPUT="${TORRBABY_BUILD_ROOT:-$ROOT/local-state/apple}"
mkdir -p "$OUTPUT"
ARGS=(-project "$ROOT/Apple/Torrbaby.xcodeproj" -scheme Torrbaby -configuration Release -derivedDataPath "$OUTPUT/$PLATFORM" "CURRENT_PROJECT_VERSION=$BUILD_NUMBER")
if [[ "$PLATFORM" == ios || "$PLATFORM" == tvos ]]; then
    : "${DEVELOPMENT_TEAM:?Set DEVELOPMENT_TEAM to your Apple Developer team ID}"
    DESTINATION="generic/platform=iOS"
    if [[ "$PLATFORM" == tvos ]]; then DESTINATION="generic/platform=tvOS"; fi
    ARGS+=(-destination "$DESTINATION" "DEVELOPMENT_TEAM=$DEVELOPMENT_TEAM" -allowProvisioningUpdates)
    if [[ -n "${ASC_KEY_PATH:-}" ]]; then
        ARGS+=(-authenticationKeyPath "$ASC_KEY_PATH" -authenticationKeyID "${ASC_KEY_ID:?}" -authenticationKeyIssuerID "${ASC_ISSUER_ID:?}")
    fi
elif [[ "$PLATFORM" == tvos-simulator ]]; then
    ARGS+=(-destination 'generic/platform=tvOS Simulator' "DEVELOPMENT_TEAM=${DEVELOPMENT_TEAM:?Set DEVELOPMENT_TEAM to your Apple Developer team ID}")
elif [[ "$PLATFORM" == macos ]]; then
    ARGS+=(-destination 'platform=macOS' CODE_SIGNING_ALLOWED=NO)
else
    echo 'Usage: build_apple.sh ios|macos|tvos|tvos-simulator' >&2; exit 2
fi
xcodebuild "${ARGS[@]}" build > "$OUTPUT/$PLATFORM-build.log" 2>&1 || { tail -30 "$OUTPUT/$PLATFORM-build.log"; exit 1; }
if [[ "$PLATFORM" == macos ]]; then
    APP="$OUTPUT/macos/Build/Products/Release/Torrbaby.app"
    codesign --force --deep --options runtime --sign "${MAC_SIGNING_IDENTITY:--}" "$APP"
elif [[ "$PLATFORM" == tvos-simulator ]]; then
    APP="$OUTPUT/tvos-simulator/Build/Products/Release-appletvsimulator/Torrbaby.app"
elif [[ "$PLATFORM" == tvos ]]; then
    APP="$OUTPUT/tvos/Build/Products/Release-appletvos/Torrbaby.app"
else
    APP="$OUTPUT/ios/Build/Products/Release-iphoneos/Torrbaby.app"
fi
printf 'Built: %s\n' "$APP"
