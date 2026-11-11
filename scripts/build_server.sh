#!/bin/bash
# Build a consistently signed macOS identity for the standalone Rust server.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
: "${MAC_SIGNING_IDENTITY:?Set MAC_SIGNING_IDENTITY to your Developer ID Application identity}"
cargo build --manifest-path "$ROOT/Cargo.toml" --release --locked
APP="$ROOT/local-state/server/Torrbaby Server.app"
mkdir -p "$APP/Contents/MacOS"
mkdir -p "$APP/Contents/Resources"
python3 "$ROOT/scripts/generate_notices.py" "$APP/Contents/Resources/THIRD_PARTY_NOTICES.txt"
cp "$ROOT/LICENSE" "$APP/Contents/Resources/LICENSE"
ICONSET="$ROOT/local-state/server/TorrbabyServer.iconset"
mkdir -p "$ICONSET"
for SIZE in 16 32 128 256 512; do
    cp "$ROOT/Apple/Torrbaby/Assets.xcassets/AppIcon.appiconset/icon-$SIZE.png" "$ICONSET/icon_${SIZE}x${SIZE}.png"
    DOUBLE=$((SIZE * 2))
    cp "$ROOT/Apple/Torrbaby/Assets.xcassets/AppIcon.appiconset/icon-$DOUBLE.png" "$ICONSET/icon_${SIZE}x${SIZE}@2x.png"
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/TorrbabyServer.icns"
cp "$ROOT/target/release/torrbaby" "$APP/Contents/MacOS/torrbaby"
swiftc -parse-as-library -O -target "$(uname -m)-apple-macos26.0" \
    "$ROOT/helpers/SubtitleTranslator.swift" \
    -o "$APP/Contents/MacOS/torrbaby-subtitle-translator"
codesign --force --options runtime --sign "$MAC_SIGNING_IDENTITY" \
    "$APP/Contents/MacOS/torrbaby-subtitle-translator"
swiftc -parse-as-library -O -target "$(uname -m)-apple-macos26.0" \
    "$ROOT/helpers/MediaSegmenter.swift" \
    -o "$APP/Contents/MacOS/torrbaby-media-segmenter"
codesign --force --options runtime --sign "$MAC_SIGNING_IDENTITY" \
    "$APP/Contents/MacOS/torrbaby-media-segmenter"
swiftc -parse-as-library -O -target "$(uname -m)-apple-macos26.0" \
    "$ROOT/helpers/SubtitleExtractor.swift" \
    -o "$APP/Contents/MacOS/torrbaby-subtitle-extractor"
codesign --force --options runtime --sign "$MAC_SIGNING_IDENTITY" \
    "$APP/Contents/MacOS/torrbaby-subtitle-extractor"
# Both Vision helpers must share one signing identifier. Vision's compiled-model
# cache is per bundle, but aned keys the compiled Neural Engine programs by
# signing identifier. With two identifiers, each helper's compile replaced the
# other's, and they recompiled the text models in turn every minute.
VISION_IDENTIFIER=org.torrbaby.server.vision
swiftc -parse-as-library -O -target "$(uname -m)-apple-macos26.0" \
    "$ROOT/helpers/SubtitleOCR.swift" -o "$APP/Contents/MacOS/torrbaby-subtitle-ocr"
codesign --force --options runtime --identifier "$VISION_IDENTIFIER" --sign "$MAC_SIGNING_IDENTITY" "$APP/Contents/MacOS/torrbaby-subtitle-ocr"
swiftc -parse-as-library -O -target "$(uname -m)-apple-macos26.0" \
    "$ROOT/helpers/CreditsScanner.swift" "$ROOT/helpers/CreditCards.swift" \
    -o "$APP/Contents/MacOS/torrbaby-credits-scanner"
codesign --force --options runtime --identifier "$VISION_IDENTIFIER" --sign "$MAC_SIGNING_IDENTITY" "$APP/Contents/MacOS/torrbaby-credits-scanner"
# On-device English speech transcription for titles without subtitles.
swiftc -parse-as-library -O -target "$(uname -m)-apple-macos26.0" \
    "$ROOT/helpers/SpeechSubtitles.swift" -o "$APP/Contents/MacOS/torrbaby-speech-subtitles"
codesign --force --options runtime --sign "$MAC_SIGNING_IDENTITY" "$APP/Contents/MacOS/torrbaby-speech-subtitles"
# Audio-only ffmpeg (LGPL build) that converts DTS and other tracks Apple devices cannot decode.
"$ROOT/scripts/build_ffmpeg.sh"
cp "$ROOT/local-state/ffmpeg/ffmpeg" "$APP/Contents/MacOS/torrbaby-ffmpeg"
cp "$ROOT/local-state/ffmpeg/COPYING.LGPLv2.1" "$APP/Contents/Resources/FFMPEG_LICENSE.txt"
codesign --force --options runtime --sign "$MAC_SIGNING_IDENTITY" "$APP/Contents/MacOS/torrbaby-ffmpeg"
cp "$ROOT/local-state/ffmpeg/ffprobe" "$APP/Contents/MacOS/torrbaby-ffprobe"
codesign --force --options runtime --sign "$MAC_SIGNING_IDENTITY" "$APP/Contents/MacOS/torrbaby-ffprobe"
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
"$ROOT/scripts/verify_server_bundle.sh" "$APP"
printf 'Built: %s\n' "$APP"
