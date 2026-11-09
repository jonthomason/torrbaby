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
No HTTP translation endpoint or player subtitle integration exists yet.

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
  `embedded_tracks_scanned: false` makes the current discovery scope explicit.
- `GET /v1/library/assets/{asset-id}/subtitles/{track-id}.vtt`: revalidates the
  completed media asset and discovers the sidecar again, then returns normalized
  UTF-8 `text/vtt`. Both routes require the normal Bearer credential.

Discovery is restricted to regular SRT files beside a completed media asset with
an exact matching video stem, optionally followed by dot-separated language and
forced/SDH labels. No arbitrary path can be supplied. Symlinks and unrelated
names are excluded. Language tags are filename hints, not content detection;
`zh` does not imply Simplified Chinese. These routes currently serve original
language only; translation jobs/cache and signed native-player delivery remain
separate pending work. Directory scanning and parsing run off the async executor.

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

This is a read-only development CLI, not yet called by the HTTP server. Extraction
currently scans all frames (including audio/video), with a 180-second loop deadline,
5 million frames, 100,000 cues and 8 MiB normalized text limits. The third-party
parser can allocate frame/metadata buffers before those checks; these are not a
complete memory/CPU sandbox. Automatic library jobs need subprocess resource
limits, cancellation, revision-aware caching and concurrency limits before wiring
this into the daemon. Player/HLS subtitle selection is still pending.
