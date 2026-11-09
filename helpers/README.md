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
