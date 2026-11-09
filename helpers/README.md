# Subtitle translation bridge

`SubtitleTranslator.swift` bridges the Rust server to Apple's on-device Translation
framework on macOS 26+. The server build script includes and signs it; Rust's
`translate-subtitles` CLI invokes it. It never downloads language models; the requested pair must already
be installed on Diamond.

Compile and sign using the same Developer ID identity as Torrbaby Server:

```sh
swiftc -parse-as-library -target arm64-apple-macos26.0 helpers/SubtitleTranslator.swift -o /tmp/torrbaby-subtitle-translator
codesign --force --options runtime --sign "$MAC_SIGNING_IDENTITY" /tmp/torrbaby-subtitle-translator
```

One JSON document on stdin, terminated by EOF:

```json
{"source":"en","target":"zh-Hans","cues":[{"id":"17","text":"Where are we going?"}]}
```

Success (exit 0) returns version 1 and `cues` with identical IDs and order, with
translated `text`. Failure (exit 1) returns version 1 and `error`: `invalid_request`,
`models_not_installed`, `translation_failed`, or `invalid_response`. Cue text is
never logged. Only English/Simplified Chinese in either direction are accepted.
Limits: 1 MiB input, 128 cues, unique nonempty IDs up to 128 UTF-8 bytes, nonblank
text up to 16 KiB per cue. Invalid requests fail before invoking Translation.

Rust must own timings, parsing/normalization, markup handling, batching, caching,
and native-player subtitle delivery. The Rust process boundary enforces a 120-second
timeout, 4 MiB response limit, kill-on-cancellation, and exact ordered cue identities.
Feed plain cue
text only. A helper response alone does not make subtitles available in the app.

From a built server bundle, pipe the request into `torrbaby translate-subtitles`.
For development, specify `--helper /path/to/torrbaby-subtitle-translator`.
Authenticated preparation jobs are described below. Native-player subtitle integration is still pending.

## Timed SRT preparation

```sh
torrbaby prepare-subtitles --input subtitles.en.srt > subtitles.en.vtt
torrbaby prepare-subtitles --input subtitles.en.srt --source en --target zh-Hans > subtitles.zh-Hans.vtt
```

`--helper` can override the adjacent helper for development. This path accepts
UTF-8 SRT (optional BOM, CRLF or LF), preserves millisecond timings and overlapping
cues, and emits WebVTT only after every translation batch succeeds. It rejects
invalid timestamps, reversed intervals, decreasing start times, empty cues,
files over 8 MiB, more than 100,000 cues and cue text over 16 KiB. Common SRT
italic/bold/underline/font tags are flattened to dialogue before translation;
unknown markup is escaped as literal text. Legacy encodings, ASS/SSA styling,
embedded container tracks and OCR are not handled by this command.

WebVTT serialization follows the timestamp and cue-block structure in the
[W3C WebVTT specification](https://www.w3.org/TR/webvtt1/). This is a preparation
command, not yet a playable HLS subtitle rendition or a player configuration.

## Authenticated sidecar API

- `GET /v1/library/assets/{asset-id}/subtitles`: returns `tracks` with opaque ID,
  filename, inferred language (nullable), forced/SDH flags and relative VTT path.
  For MKV/WebM, also returns `embedded_tracks` with track numbers, language,
  codec and extraction support. `embedded_tracks_scanned` and `embedded_error`
  distinguish successful inspection from unsupported containers or failed inspection.
- `GET /v1/library/assets/{asset-id}/subtitles/{track-id}.vtt`: revalidates the
  completed media asset and discovers the sidecar again, then returns normalized
  UTF-8 `text/vtt`. Both routes require the normal Bearer credential.

Discovery is restricted to regular SRT files beside a completed media asset with
an exact matching video stem, optionally followed by dot-separated language and
forced/SDH labels. No arbitrary path can be supplied. Symlinks and unrelated
names are excluded. Language tags are filename hints, not content detection;
`zh` does not imply Simplified Chinese. These routes currently serve original
language only; embedded translation jobs are available below. Signed native-player
subtitle delivery remains pending. Directory scanning and parsing run off the async executor.

## Embedded Matroska text tracks (development CLI)

`matroska-subtitles --input movie.mkv` lists subtitle track numbers, codec,
language, forced/SDH flags, and whether this extractor supports them.
`matroska-subtitles --input movie.mkv --track NUMBER` emits normalized WebVTT.

Uses `matroska-demuxer` 0.8.1 (Zlib OR MIT OR Apache-2.0, no additional dependency
crates); Torrbaby can remain MIT licensed. Build-time notices include its license
files. The [Matroska codec mapping](https://www.matroska.org/technical/codec_specs.html)
defines subtitle block payloads and ASS/SSA event fields.

Supported now: S_TEXT/UTF8, S_TEXT/ASS, S_TEXT/SSA without content encodings.
ASS/SSA drawing commands and override styling are removed; timing and dialogue
remain. Bitmap tracks are listed but never OCR'd. WebVTT-in-Matroska is identified
as text but not yet extracted. MP4 embedded subtitles remain separate work.
Explicit cue duration is required; the extractor does not guess missing timing.
Timestamp scale and codec delay are applied before emitting millisecond timings.

The HTTP preparation API also invokes this read-only worker. Extraction
currently scans all frames (including audio/video), with a 180-second loop deadline,
5 million frames, 100,000 cues and 8 MiB normalized text limits. The third-party
parser can allocate frame/metadata buffers before those checks; these are not a
complete memory sandbox. HTTP preparation uses subprocess time/output limits,
revision-aware caching and bounded concurrency below. Player/HLS subtitle selection
is still pending.

## Extraction + translation pipeline

```sh
torrbaby prepare-subtitles --input movie.mkv --track 3 --source en --target zh-Hans > movie.zh-Hans.vtt
```

Omit the language options for original-language WebVTT. `--track` selects embedded
Matroska extraction; without it the input is a UTF-8 SRT sidecar.

Embedded preparation invokes the same Rust executable as a child worker. One
worker at a time is allowed per parent process. Worker CPU time is limited to180s,
parent elapsed time to190s, stdout to16MiB; core dumps are disabled. Timeout,
malformed output and extraction errors kill/reap the child. Cancellation uses
kill-on-drop. The parent validates cue counts/text/timing/order and rejects a file
whose inode/size/change timestamps changed during extraction. Only then are cue
texts sent in bounded batches to the Apple Translation helper. Timing stays in
Rust; output is emitted after complete success.

macOS rejected the tested address-space and data-size limits (`ulimit -v/-d`)
with Invalid argument. The worker is process isolation plus runtime/output
limits, **not a hard memory sandbox**. Automatic API preparation/caching and
native selectable rendition delivery still need implementation.


## Queued embedded subtitle preparation API

All routes require the existing Bearer credential and a completed, accessible
library asset. No request accepts a filesystem path.

1. `GET /v1/library/assets/{id}/subtitles` discovers sidecars and embedded MKV/WebM
   tracks. Embedded inspection uses at most two child processes, 10 CPU seconds,
   15 elapsed seconds and 1 MiB output each; busy/failing inspection is reported
   separately so sidecar results remain usable. Bitmap subtitles are listed but
   never extracted or OCRed.
2. `POST /v1/library/assets/{id}/subtitles/prepare` with `{"track":3}` prepares
   original text, or add `"source":"en","target":"zh-Hans"` for translation.
   The reverse pair is supported too; omit both language fields for original text.
3. Poll `GET /v1/subtitle-jobs/{job-id}` until `state` is `ready` or `failed`.
   Ready responses contain an authenticated relative WebVTT `path`.
4. `GET /v1/subtitle-jobs/{job-id}.vtt` returns the cached WebVTT artifact.

Jobs are deduplicated by asset identity, file revision, track and language pair.
Four pending jobs maximum, one running preparation, 20-minute overall timeout
including queue time. The adjacent signed Translation helper runs on Diamond;
installed language models are required. Cache files live in the private
`subtitles` state directory, bounded to approximately 512 MiB and 256 records,
with oldest inactive entries evicted first. This is not LRU. Prepared output is
capped at 16 MiB. Ready artifacts survive daemon restart. Interrupted jobs can be
resubmitted; changed media returns `stale`, missing artifacts `expired`.
Status/artifact requests revalidate the managed library asset. No database schema
change or Plex access is involved. This API produces subtitle files; it does not
yet make them selectable in AVPlayer.

## Native HLS packaging experiment

`MediaSegmenter.swift` is a supervised diagnostic helper, bundled by build_server.sh but not yet connected to client playback. It uses
AVFoundation passthrough samples (no re-encoding) to emit fragmented MP4 init/media
segments and a VOD playlist from compatible local media:

```sh
swiftc -parse-as-library helpers/MediaSegmenter.swift -o /tmp/torrbaby-media-segmenter
target/debug/torrbaby package-media --input INPUT --output NEW_OUTPUT_DIRECTORY \
  --helper /tmp/torrbaby-media-segmenter --max-bytes 536870912
```

Prototype limits: 8 audio/video tracks, 300 elapsed seconds during packaging,
512 MiB output. A production Rust worker must enforce external timeout/cancellation,
source revision checks, disk reservations/cache cleanup, authentication and session
URL scoping before integration. It intentionally does not replace direct playback.
Container/audio/HDR compatibility and timestamp alignment need broader validation.

A generated 24-second H.264 fixture was packaged and served from a temporary
loopback-only test server with English/Chinese WebVTT renditions. AVPlayer reported
readyToPlay and its native legible media-selection group contained `en` and
`zh-Hans`; selecting either succeeded. This verifies native track discovery/selection,
not rendered cue accuracy, production media compatibility, or deployed app support.
The temporary fixture server is diagnostic tooling, not a Python production service.

The Rust supervisor allows one packaging worker per process, imposes a 600-second
CPU / 610-second wall limit, bounds helper stdout, checks the source revision,
and removes its newly created output on failure. Existing output directories
are rejected. Cancellation kills the child; interrupted directories still need
cache recovery before HTTP integration. The helper reserves 1 GiB free space
before each segment and obeys a configurable output budget (1 MiB–32 GiB,
512 MiB default). This is passthrough only: unsupported containers/codecs fail.
No memory sandbox is claimed. Playback cache management, authenticated HLS
routes and subtitle renditions are still pending; the app player is unchanged.
