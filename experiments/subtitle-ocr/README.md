# VobSub → Apple Vision feasibility probe

Read-only experiment; not connected to the production daemon or translation queue.
The Rust executable extracts a selected Matroska VobSub track, reverses its zlib
content compression, decodes DVD subpictures, and writes cropped grayscale cue
images plus original display timestamps. Swift recognizes the images with Apple
Vision. No movie decoding, transcoding, rewriting, or OCR of burned-in text.

## Run on Diamond

```
cargo build --release --manifest-path experiments/subtitle-ocr/Cargo.toml
# The extractor needs the same removable-volume authorization as the server.
# Use a separately located, signed experimental bundle; never deploy it as the server.
torrbaby-subtitle-ocr-probe INPUT.mkv 3 NEW_CUE_DIRECTORY
swiftc -O -parse-as-library experiments/subtitle-ocr/OCR.swift -o ocr-probe
ocr-probe NEW_CUE_DIRECTORY NEW_OUTPUT_DIRECTORY 1
```

The optional fourth OCR argument limits cue count. Each output directory must be
new. English recognition is explicitly selected for this experiment. Twofold
nearest-neighbor enlargement, a white border, and inverted grayscale produce
black text on white. Empty recognition retries with more whitespace, then Vision's
fast recognition path. Outputs include text, timestamps, confidence, recognition
method, per-cue timing, summary metrics, and sampled images. Retry.swift isolates
the short-caption fallback experiment.

## Dependencies and references

- `oxideav-sub-image` 0.0.9: MIT; existing packet-level VobSub decoder API.
  https://github.com/OxideAV/oxideav-sub-image
- Vendored `matroska-demuxer`: MIT option, as used by Torrbaby.
- `flate2`: MIT/Apache-2.0; bounded zlib expansion.
- Apple Vision is a macOS system framework; no model service or API key.
  https://developer.apple.com/documentation/vision/vnrecognizetextrequest

Cargo.lock pins the isolated experiment. Transitive dependency metadata was
checked: MIT-compatible permissive licenses only. No GPL source was incorporated.

## Scope limits before production integration

This probe handles the observed static-palette VobSub track and rejects dynamic
palette changes, encryption, and unsupported compression. It is not yet a general
PGS/OCR implementation. Production needs worker time/resource limits, cancellation,
progress/resume, provenance, robust caption ordering and language handling, and
preservation of unsuccessful cues for retry. Never interpret confidence as a
measured accuracy score. Keep image/OCR artifacts out of git; the review corpus
and full extracted text remain private in Downloads/Torrbaby/OCR-Experiment.

## Measured result: Outlander S01E01 on Diamond, 2026-11-10

The tested episode spans about 62 minutes and contains 697 VobSub cues.

| Stage / run | Wall time | Empty captions |
| --- | ---: | ---: |
| Rust extraction and bitmap decoding | 4.54 s | 0 decode failures |
| Initial accurate OCR, cold setup | 75.63 s | 2 |
| Accurate OCR, warm, one worker | 24.80 s | 2 |
| Accurate OCR, warm, four workers | 25.69 s | 2 |
| Final OCR with retry, cold helper run | 69.58 s | 0 |
| Final OCR with retry, warm, one worker | 25.36 s | 0 |

The final warm pipeline totals about 29.9 seconds including extraction. The first
OCR call accounted for 42.8–48.4 seconds in cold runs; a process sample showed
Apple Neural Engine model compilation. Four workers did not improve throughput.
Initial warm single-worker median cue time was 38.8 ms, p95 53.9 ms.

Visual review: 28 regularly spaced captions plus both initially empty captions
(30 total) matched the source images after retry, allowing normal whitespace
normalization. Names, italics, and Scots dialogue appeared intact in those samples.
The misses were very short captions: extra whitespace recovered one through the
accurate path; the other needed the fast path on a padded image. Its confidence
was only 0.5 despite a correct result. Do not use confidence alone as an accuracy
measurement. We have not computed a whole-episode word error rate.

All 697 start/end timestamps were present, nonempty final text was produced for
every cue, and cold/warm final runs returned identical text. The original movie
was opened read-only; source size/mtime remained unchanged. English SRT/VTT and
a 30-caption image comparison HTML page were saved privately to
~/Downloads/Torrbaby/OCR-Experiment/Outlander-S01E01/. No OCR subtitles were
installed into the library or sent to translation; no production service was
restarted. Recommendation: feasible as a bounded, cached background fallback,
starting with VobSub and a single OCR worker.
