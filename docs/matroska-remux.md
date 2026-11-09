# Native Apple delivery for Matroska

Status: investigation and timing diagnostics; no production MKV remux yet.

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

No new crate has been added yet. First acceptance should use owned reordered
AVC/HEVC fixtures and actual AVPlayer decoding/seeking, audio sync, Dolby output,
and native generated-subtitle selection. Then test representative catalog files.
Keep remux supervised with time/storage limits, revision checks and cleanup of
owned output. Never publish a partial artifact as ready.
