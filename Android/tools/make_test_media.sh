#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
mkdir -p "$ROOT/.local"
cat > "$ROOT/.local/sample.vtt" <<'VTT'
WEBVTT

00:00.000 --> 00:30.000
Native subtitles · English

00:30.000 --> 01:00.000
Remote-only playback with shared progress.

01:00.000 --> 02:00.000
Streaming from the server. No offline video.
VTT
ffmpeg -hide_banner -loglevel error -y -f lavfi -i 'testsrc2=size=1280x720:rate=24' -f lavfi -i 'sine=frequency=220:sample_rate=48000' -f lavfi -i 'sine=frequency=440:sample_rate=48000' -t 120 -map 0:v -map 1:a -map 2:a -c:v h264_videotoolbox -b:v 1500k -c:a aac -b:a 96k -metadata:s:a:0 language=eng -metadata:s:a:1 language=zho -movflags +faststart "$ROOT/.local/sample.mp4"
