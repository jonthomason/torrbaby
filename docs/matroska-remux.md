# Native Apple delivery for Matroska

Status: development CLI implemented and native AVC decoding verified; no production MKV remux yet.

The existing HLS packager uses AVFoundation compressed passthrough. It cannot
read our MKV sources. A Rust container conversion must precede that path or
produce equivalent HLS track artifacts directly. Originals stay untouched.

## Evidence and implementation requirements

The catalog inspection found 62 completed MKVs: 53 HEVC and 9 AVC. Audio
includes AAC, E-AC-3, AC-3 and DTS. All five sampled codec combinations have
reordered video presentation timestamps. Matroska block timestamps cannot simply
be used as MP4 decode timestamps. Preserve packet decode order, reconstruct a
validated decode timeline, and retain composition offsets. Do not sort compressed
packets by presentation timestamp.

`inspect-matroska` now reports presentation-order delta histograms separately
from packet-order reorder counts, duplicate PTS and deltas outside the declared
default duration (allowing one timestamp tick for quantization). This is sample
evidence, not proof that an entire title has constant frame rate. Audio packet
durations must come from codec framing; video DefaultDuration is not sufficient
for arbitrary variable-frame-rate media. Respect CodecDelay and initial offsets.

Candidate oxideav-mp4 0.0.11 local source review:

- Ordinary muxer streams payload to disk; large_mdat must be enabled above 4 GiB.
- Faststart buffers the entire payload in memory: do not enable for movies.
- Fragmented muxer has bounded-by-fragment packet buffering, but caller must
  enforce limits and segment/keyframe cadence, including a pathological long GOP.
- Both require explicit duration and PTS/DTS for reordered content. Fragmented
  writer defaults missing composition offsets to zero; that would lose timing.
- AC-3/E-AC-3 sample entries accept raw dac3/dec3 extradata. They do not derive
  these configuration records from Matroska audio packets. Add a bounded parser
  and validate stream changes/dependent substreams before using this path.
- Preserve HEVC/AVC configuration, language, aspect ratio, color/HDR metadata.
  DTS needs a separately verified native support or conversion policy.

MIT oxideav-core 0.1.43 and oxideav-mp4 0.0.11 are now locked dependencies; notices generation passed. Further acceptance should use owned reordered
AVC/HEVC fixtures and actual AVPlayer decoding/seeking, audio sync, Dolby output,
and native generated-subtitle selection. Then test representative catalog files.
Keep remux supervised with time/storage limits, revision checks and cleanup of
owned output. Never publish a partial artifact as ready.

## First implementation

`remux-matroska --input SOURCE --output NEW_MP4 [--max-bytes N]` is a
development command, not connected to playback APIs. It streams compressed AVC/
HEVC and ordinary AAC-LC to MP4 with a 64-bit mdat. It validates all video PTS
against a zero-origin fixed cadence, reconstructs decode timing and composition
offsets, and rejects offsets/gaps/delays or unsupported codecs/configurations.
Audio uses 1024-sample AAC units with timestamp validation. Storage, packet count
and elapsed time are bounded; errors remove only the newly created output.
Existing destinations are never replaced. Source revision is checked at finish.

Owned 24-second AVC fixture with reordered frames: native AVAssetReader decoded
576 output frames, byte-identical to original decoded frames in display order.
Existing-output refusal passed. This is not proof of HEVC/HDR, audio sync or seek
behavior. Dolby/DTS, metadata preservation, variable frame rate, nonzero offsets,
full process supervision and HLS integration remain required. No production
server deployment or compatibility claim is made for this development command.

## AC-3 passthrough

The development converter now supports AC-3 with a zero-origin, no-CodecDelay
timeline. `server/dolby.rs` derives the three-byte MP4 dac3 configuration from
the syncframe header per ETSI TS 102 366 sections 4 and F.4. It validates sync,
version, sample rate, exact frame length, channel layout and conditional header
fields. Each packet is revalidated; midstream configuration changes fail rather
than silently producing a bad stream. A bounded initial scan finds configuration
before writing the MP4 header. Audio duration is 1,536 samples per syncframe.

Validation used our existing 24-second generated AVC clip plus an owned 440 Hz
stereo tone. The already-installed ffmpeg was used only to encode the test tone
and make a reference M4A; it is not linked, bundled or called by the daemon.
Native Apple decoding produced identical 576 video frames and 1,151,712 PCM
samples. Both reference and remux audio ended at 23.994 seconds (native decoder
trimming); their timelines match. An unsupported midstream header caused failure
and removal of the newly created partial output. A focused standard-header test
checks the dac3 bytes and rejects truncated/wrong-version packets.

E-AC-3, DTS, nonzero CodecDelay/track offsets, seek acceptance, HDR metadata and
production HLS integration are still outstanding. No production activation yet.
