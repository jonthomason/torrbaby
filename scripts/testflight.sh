#!/bin/bash
# Archive the iPhone/iPad app with distribution signing and send it to TestFlight.
#
#   scripts/testflight.sh            archive, sign and upload to App Store Connect
#   scripts/testflight.sh --rehearse archive and sign only; nothing is uploaded
#
# Needs DEVELOPMENT_TEAM and the App Store Connect API key variables (ASC_KEY_PATH,
# ASC_KEY_ID, ASC_ISSUER_ID). The app record for the bundle ID must already exist in
# App Store Connect; Apple offers no API to create it.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODE=upload; [[ "${1:-}" == --rehearse ]] && MODE=export
: "${DEVELOPMENT_TEAM:?Set DEVELOPMENT_TEAM to your Apple Developer team ID}"
: "${ASC_KEY_PATH:?}" "${ASC_KEY_ID:?}" "${ASC_ISSUER_ID:?}"
BUILD_NUMBER="${TORRBABY_BUILD_NUMBER:-$(git -C "$ROOT" rev-list --count HEAD)}"
OUTPUT="${TORRBABY_BUILD_ROOT:-$ROOT/local-state/apple}/testflight"
mkdir -p "$OUTPUT"
KEY=(-allowProvisioningUpdates -authenticationKeyPath "$ASC_KEY_PATH" -authenticationKeyID "$ASC_KEY_ID" -authenticationKeyIssuerID "$ASC_ISSUER_ID")
# The tunnel's and the direct connection's Rust half, built fresh: never upload a stale one.
"$ROOT/scripts/build_direct.sh"
python3 "$ROOT/scripts/check_localizations.py"
ARCHIVE="$OUTPUT/Torrbaby-$BUILD_NUMBER.xcarchive"
xcodebuild -project "$ROOT/Apple/Torrbaby.xcodeproj" -scheme Torrbaby -configuration Release \
    -destination 'generic/platform=iOS' -derivedDataPath "$OUTPUT/derived" -archivePath "$ARCHIVE" \
    "CURRENT_PROJECT_VERSION=$BUILD_NUMBER" "DEVELOPMENT_TEAM=$DEVELOPMENT_TEAM" "${KEY[@]}" archive \
    > "$OUTPUT/archive.log" 2>&1 || { tail -30 "$OUTPUT/archive.log"; exit 1; }
cat > "$OUTPUT/ExportOptions.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
    <key>method</key><string>app-store-connect</string>
    <key>destination</key><string>$MODE</string>
    <key>teamID</key><string>$DEVELOPMENT_TEAM</string>
    <key>signingStyle</key><string>automatic</string>
    <key>manageAppVersionAndBuildNumber</key><false/>
    <key>uploadSymbols</key><true/>
</dict></plist>
PLIST
xcodebuild -exportArchive -archivePath "$ARCHIVE" -exportPath "$OUTPUT/export" \
    -exportOptionsPlist "$OUTPUT/ExportOptions.plist" "${KEY[@]}" \
    > "$OUTPUT/export.log" 2>&1 || { tail -40 "$OUTPUT/export.log"; exit 1; }
if [[ $MODE == upload ]]; then echo "Uploaded build $BUILD_NUMBER to App Store Connect."
else echo "Rehearsal: build $BUILD_NUMBER archived and signed at $OUTPUT/export (not uploaded)."; fi
