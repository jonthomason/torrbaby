#!/bin/bash
# Build a consistently signed macOS identity for the standalone Rust server.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
: "MAC_SIGNING_IDENTITY:?Set MAC_SIGNING_IDENTITY to your Developer ID Application identity}"
cargo build --manifest-path "$ROOT/Cargo.toml" --release --locked
APP="$ROOT/local-state/server/Torrbaby Server.app"
mkdir -p "$APP/Contents/MacOS"
mkdir -p "$APP/Contents/Resources"
ICONSET="$ROOT/local-state/server/TorrbabyServer.iconset"
mkdir -p "$ICONSET"
for SIZE in 16 32 128 256 512; do
    cp "$ROOT/Apple/Torrbaby/Assets.xcassets/AppIcon.appiconset/icon-$SIZE.png" "$ICONSET/icon_${SIZE}x${SIZE}.png"
    DOUBLE=$((SIZE * 2))
    cp "$ROOT/Apple/Torrbaby/Assets.xcassets/AppIcon.appiconset/icon-$DOUBLE.png" "$ICONSET/icon_${SIZE}x${SIZE}@2x.png"
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/TorrbabyServer.icns"
cp "$ROOT/target/release/torrbaby" "$APP/Contents/MacOS/torrbaby"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>org.torrbaby.server</string>
<key>CFBundleName</key><string>Torrbaby Server</string>
<key>CFBundleDisplayName</key><string>Torrbaby Server</string>
<key>CFBundleExecutable</key><string>torrbaby</string>
<key>CFBundleIconFile</key><string>TorrbabyServer</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.3.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSUIElement</key><true/>
<key>NSRemovableVolumesUsageDescription</key><string>Torrbaby Server reads and organizes your movie and TV library on external disks.</string>
<key>NSNetworkVolumesUsageDescription</key><string>Torrbaby Server reads your movie and TV library on network disks.</string>
<key>NSHumanReadableCopyright</key><string>Copyright © 2026 Jon Thomason. MIT License.</string>
</dict></plist>
PLIST
codesign --force --options runtime --sign "$MAC_SIGNING_IDENTITY" "$APP"
codesign --verify --strict "$APP"
printf 'Built: %s\n' "$APP"
