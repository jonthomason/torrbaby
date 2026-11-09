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
offsets, and rejects gaps or unsupported codecs/configurations. Track offsets and codec delays are preserved with edit lists.
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

The development converter now supports AC-3 with validated constant-cadence
timing. `server/dolby.rs` derives the three-byte MP4 dac3 configuration from
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

E-AC-3, DTS, HDR metadata and
production HLS integration are still outstanding. No production activation yet.

## Track origins, codec delay and playback policy

User explicitly questioned risking currently successful playback. Keep direct
playback for working files. The reason for this converter is native delivery of
generated captions (and compatibility when necessary), not mandatory conversion
of every file. The command remains isolated from playback APIs; callers must
never silently switch normal playback to this development path.

The converter now preserves each track's initial timestamp and subtracts its
Matroska CodecDelay. MP4 edit lists express delayed starts or priming trim. A
bounded pass over the finished output's moov updates fixed-size mvhd/tkhd duration
fields: oxideav-mp4 0.0.11 otherwise uses media duration without accounting for
edits. Compressed samples and their tables remain unchanged.

Owned 24-second fixture results in native Apple decoding:
- Video starting at 1s and audio at 200ms: movie duration25s; decoder inserts
  exactly9,600 silent audio samples before byte-identical original PCM.
- AC-3 CodecDelay5,333,333ns: exactly256 samples trimmed, remaining PCM equals
  the baseline suffix byte-for-byte. Native audio ends23.9886667s versus
  baseline23.994s (decoder itself trims288 initial samples in both paths).
- Independent reference remux has equivalent positive-offset timing. Reference
  delay rounds to milliseconds; our sample-accurate delay preserves the declared
 256sample trim rather than that reference's240sample trim.
- AVPlayer ready-to-play and exact seeks to12.5s,2s,23s pass for both variants.

These remain owned-fixture checks, not broad catalog/HDR/physical-device acceptance.
No production deployment or new Apple build for this development-only change.

## E-AC-3 and a real catalog conversion

The development converter now derives dec3 configuration for standalone E-AC-3
independent substream0 with six audio blocks per syncframe. It validates each
packet and rejects dependent/converted/multiple substreams, short-block assembly,
half-rate audio, extended mixing metadata and additional/JOC signalling until
those forms are implemented. Fixed configuration includes bitrate; variable-rate
changes currently reject safely. No audio bytes are changed. The HEVC codec ID
was corrected to the MP4 writer's `h265` identifier.

An owned 24-second 5.1 E-AC-3 tone plus reordered AVC video passed native decoder
comparison: all576video frames and1,151,712PCM sample frames match the reference.
Positive offsets insert exactly9,600silent sample frames; codec delay trims
exactly256sample frames with remaining PCM unchanged. Native AVPlayer forward,
backward and near-end seeks passed for both variants.

Read-only initial-header inspection on Diamond accepted41of42 E-AC-3 tracks
and the one AC-3 track. One E-AC-3 packet contains a different frame layout and
is rejected. This sample does not prove full-file compatibility for all41tracks.
A complete305MB/35-minute HEVC+E-AC-3 catalog asset converted successfully. Native
Apple decoding of its first2seconds and seeks at2,12.5,23seconds passed. The
existing AVFoundation HLS packager also completed successfully from that MP4.
No original files were modified and no playback API was switched to this path.

Outstanding: generated-caption HLS playback acceptance on that output, HDR/color/
track metadata, remaining packet layouts, and supervised conditional integration.
