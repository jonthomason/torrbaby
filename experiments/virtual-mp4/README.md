# Virtual MP4 feasibility pass — 2026-11-10

**Result: promising, isolated proof passed. Not production-ready.**
No Diamond restart, production code change, original modification, or full movie
copy. Python is disposable experiment scaffolding, not a proposed server runtime.
User wants Torrbaby-owned subtitle selection with native rendering/transport/PiP.

## Mechanism

A loopback HTTP server exposes one byte-addressable MP4 assembled from:
- Original fixture file, with only its old moov box type masked as free in transit.
- Tiny subtitle-only MP4 payload, with its ftyp and moov masked as free in transit.
- A newly generated moov containing original track descriptions and added subtitle
  tracks. Subtitle chunk offsets use co64 and point into the virtual payload.
  Track IDs, movie-timescale durations, edit durations, alternate group and display
  dimensions are adjusted. Original audiovisual sample offsets/data remain intact.

The originals are opened read-only. The assembled movie is never written to disk.
HTTP single-range GET/HEAD reads only requested portions of the backing files.
FFmpeg is an already-installed fixture authoring/inspection tool only.

Two issues resolved in the bounded experiment:
- A second live ftyp caused AVFoundation Cannot Open despite ffprobe acceptance.
- Subtitle-only tkhd dimensions were zero: native options existed but captions
  were invisible. Supplying the video's track dimensions enabled rendering.

## Measured evidence

Owned 120-second AVC + two AAC audio-track fixture, local HTTP:
- Original: 25,562,667 bytes.
- Subtitle-only file (English and Chinese): 1,369 bytes.
- Generated moov: 116,972 bytes, held in memory.
- Virtual map setup: about 0.2ms. No audiovisual payload scan needed for setup.
- iPadOS26.5 simulator AVPlayer ready: 0.34–0.36s.
- Native English caption visibly rendered inline.
- Chinese selected through our control and visibly rendered inside actual native
  PiP, verified through Device Hub. No UILabel overlay in this app.
- macOS AVPlayer ready: 0.265s; English and Chinese selection, Off, and exact
  forward/back/near-end seeks (90,5,115,2 seconds) passed.
- SHA-256 of all 25,446,711 original mdat bytes matched their virtual response.
- App and temporary HTTP server stopped after checks. Test playback muted.

## Limits and next decision

This proves one ordinary MP4 fixture, not the whole catalog. No real-device,
HDR/Dolby Vision, HEVC, Matroska, fragmented/encrypted MP4, or offline acceptance.
The prototype assumes conventional bounded nested boxes and specific subtitle
fixture layout. It lacks production parser hardening, authentication, source
revision pinning, cancellation/resource limits and adversarial range validation.
The movie header is at the end, so AVPlayer fetches the tail; source-data reads
and buffering still occur normally. Reported readiness is local-fixture timing,
not a promise of subsecond full-length LAN playback.

A production implementation should be Rust and initially admit only verified
ordinary MP4 inputs. Pin the exact source/subtitle revision for each URL; validate
all offsets/track references. Reject unsupported cases and preserve direct-play
or overlay fallback. Do not generalize this proof into a mandatory remux path.

Downloading the virtual response could create one self-contained captioned file
on the client without a full server copy, but that has not been tested here.

## Reproduce

From repository root:

```sh
mkdir -p local-state/virtual-mp4
ffmpeg -v error -n -i experiments/virtual-mp4/en.srt \
  -i experiments/virtual-mp4/zh.srt -map 0 -map 1 -c:s mov_text \
  -metadata:s:s:0 language=eng -metadata:s:s:1 language=zho \
  local-state/virtual-mp4/captions.mp4
python3 experiments/virtual-mp4/serve.py Android/.local/sample.mp4 \
  local-state/virtual-mp4/captions.mp4
```

In another terminal:

```sh
swiftc -parse-as-library experiments/virtual-mp4/Probe.swift \
  -o local-state/virtual-mp4/probe
local-state/virtual-mp4/probe
```

The isolated simulator source is iPadProbe.swift. Installed bundle identifier:
org.torrbaby.virtual-mp4-experiment. iPad simulator:
4E2EEF4D-440B-44AB-B4E3-58C9963DEE46. The app permits HTTP solely for its loopback
experiment and has background audio enabled for PiP. Generated app stays under
ignored local-state/virtual-mp4. Stop the app and server after use.

## Full-catalog follow-up, 2026-11-10

This playback work is the top priority. Apple MKV support was missing and should
have been disclosed clearly; do not claim it shipped based on these diagnostics.

Full Matrix MP4 over a signed source range URL (kept in ignored local state):
- 1,992,052,865 original bytes, 179,316 subtitle bytes, 4,102,953 new moov bytes.
- Setup 0.087s; desktop ready 0.353s; iPad simulator ready 0.52s.
- English/Chinese/Off and far forward/back/end seeks passed on desktop.
- Full-movie PiP has not yet been visually verified; fixture PiP passed above.

Rust MKV experiment now uses the existing passthrough remuxer as an index builder.
It records each demuxed packet's original source offset, intercepts payload writes,
and persists only packed MP4 metadata plus an extent map. A Rust loopback range
server reads these metadata bytes and original MKV packets. No media payload is
retained in a second movie. Construction currently uses a temporary sparse file:
filesystem allocation may exceed its useful bytes; remove this dependency before
production. Tiny fixtures demonstrated that allocation is filesystem-dependent.

Real Philosopher's Stone on Diamond:
- Original MKV: 2,845,962,568 bytes; HEVC 1920x800 + 6-channel AAC-LC.
- Index: 675,095 packets in 18.106s; JSON map 18,967,847 bytes;
  packed metadata allocation 9,322,496 bytes. Virtual MP4 2,850,292,380 bytes.
- Accepted its exact AAC-LC no-SBR sync extension; no arbitrary AAC broadening.
- Native macOS AVPlayer ready in 0.590s through an SSH tunnel to the isolated
  Diamond Rust loopback server. Duration 9,530.562s.
- Exact seeks to 0,60,4765.281,9520.562,15 succeeded and playback advanced.
- Native image decoding returned 640x266 frames near 60,4765,9500 seconds.
- This does not yet validate full audiovisual sync, every frame, physical iPad,
  captions on MKV, PiP on this movie, or all catalog codecs.
- Existing daemon was not restarted; original files untouched; probes muted.
- Owned AC3 fixture: all virtual bytes match reconstructed MP4 and 30 random
  HTTP ranges match. Packed metadata+map about 50KB for 615KB virtual movie.

Diagnostic commands (new destination directory required):
```
torrbaby index-matroska --input movie.mkv --output new-index-directory
torrbaby serve-indexed-matroska --input movie.mkv --index new-index-directory
```
The diagnostic listener binds loopback only. Do not expose it as production:
production needs authenticated signed URLs, source revision/inode pinning across
races and mutation, bounded scheduling, durable atomic index publication,
cleanup/invalidation, admission checks, and client integration. Existing MKV
subtitle tracks are not yet included by this index builder.

Next: eliminate sparse construction allocation; integrate background indexing
and authenticated virtual ranges; attach original/generated text subtitles;
validate full movies and native iPad/PiP before switching app playback. Unsupported
codecs need explicit fallback/error, never a claim that every MKV is supported.
