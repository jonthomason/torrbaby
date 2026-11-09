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
