#!/bin/bash
# Build the small audio-only ffmpeg the server bundles as torrbaby-ffmpeg.
# It decodes audio formats Apple devices cannot play (DTS, TrueHD, FLAC, Opus…)
# and encodes Dolby Digital Plus. LGPL configuration: no GPL or non-free parts,
# no video codecs, no network. The source is the official release, pinned by hash.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION=9.0.2
SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e
OUT="$ROOT/local-state/ffmpeg"
STAMP="$OUT/.built-$VERSION-1"
if [[ -x "$OUT/ffmpeg" && -f "$STAMP" ]]; then exit 0; fi
mkdir -p "$OUT/source"
ARCHIVE="$OUT/source/ffmpeg-$VERSION.tar.xz"
if [[ ! -f "$ARCHIVE" ]]; then
    curl --fail --location --silent --show-error -o "$ARCHIVE.part" "https://ffmpeg.org/releases/ffmpeg-$VERSION.tar.xz"
    mv "$ARCHIVE.part" "$ARCHIVE"
fi
if [[ "$(shasum -a 256 "$ARCHIVE" | cut -d' ' -f1)" != "$SHA256" ]]; then
    echo "ffmpeg-$VERSION.tar.xz does not match the pinned SHA-256" >&2
    exit 1
fi
BUILD="$OUT/source/ffmpeg-$VERSION"
rm -rf "$BUILD"
tar -xf "$ARCHIVE" -C "$OUT/source"
cd "$BUILD"
./configure \
    --disable-everything --disable-autodetect --disable-doc --disable-debug \
    --disable-ffplay --disable-ffprobe --disable-network --disable-shared --enable-static \
    --extra-cflags="-mmacosx-version-min=26.0" --extra-ldflags="-mmacosx-version-min=26.0" \
    --enable-protocol=file \
    --enable-demuxer=matroska,wav \
    --enable-muxer=eac3,wav \
    --enable-encoder=eac3,pcm_s16le \
    --enable-decoder=dca,truehd,mlp,flac,opus,vorbis,mp3,mp2,alac,pcm_s16le,pcm_s24le,pcm_s32le,pcm_s16be,pcm_s24be,pcm_f32le,ac3,eac3,aac \
    --enable-parser=dca,mlp,flac,opus,vorbis,mpegaudio,ac3,aac \
    --enable-filter=aresample,aformat,anull >/dev/null
make -j"$(sysctl -n hw.ncpu)" ffmpeg >/dev/null
cp ffmpeg "$OUT/ffmpeg"
cp COPYING.LGPLv2.1 "$OUT/COPYING.LGPLv2.1"
rm -rf "$BUILD"
rm -f "$OUT"/.built-*
touch "$STAMP"
printf 'Built: %s\n' "$OUT/ffmpeg"
