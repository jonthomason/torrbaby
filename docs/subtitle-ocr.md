# Bitmap subtitle fallback

Torrbaby's Diamond daemon can recognize DVD-style VobSub captions inside MKV files using Apple Vision, then translate the recovered text with its existing Apple Translation helper. This runs in the background subtitle queue; no AI session is required.

## Selection

- Embedded text and separate text subtitle files always take precedence.
- OCR is admitted only when no text track or sidecar is available. The worker checks this again before decoding.
- Non-forced English or Chinese VobSub tracks are preferred; unlabeled VobSub can be recognized and its language inferred. Unsupported or unverified languages fail rather than being labeled English.
- English produces Simplified Chinese; Chinese produces English. Existing original text captions remain the preferred input.
- PGS, standalone DVD subtitle files, dynamic VobSub palettes, encrypted subtitles, and unsupported compression are not supported by this release.

## Preservation and resource use

Original media is never rewritten. OCR retains caption timestamps and publishes WebVTT using the existing subtitle job path. Both recovered captions and translations can be used by the native virtual MP4 player and offline packaging.

One subtitle worker runs at a time. Temporary grayscale images are limited to 256 MiB and removed after completion/failure; abandoned work is cleaned on startup. Validated recognized text is cached by source revision, track, and language hint, with a 128 MiB cap. Original-language and translated jobs reuse this cache. No full movie copy is created.

Decoding and recognition run as separate, time-limited processes. Malformed images/timestamps, source changes, decoding failures, unverified languages, or excessive unreadable captions fail the job without changing source media. A small number of unreadable images can be omitted (up to max(3, 1%)); the job reports `unreadable_cues`. OCR spelling mistakes remain possible.

## Queue and packaging

Jobs report `stage: ocr`, cue progress, and `extraction_method: ocr`. Prepared OCR captions carry the generated marker already understood by clients. Failed jobs retain the existing retry behavior.

The signed server bundle must include `torrbaby-subtitle-ocr` alongside the existing extractor, translator, and media segmenter. `scripts/verify_server_bundle.sh` checks this before deployment. Apple Vision requires no additional model service or API key.

## Acceptance

The focused text-first selection check covers embedded text, sidecars, unsupported text, bilingual text, and unlabeled VobSub fallback. The isolated full-episode acceptance uses Outlander S01E01: all 697 captions were recovered with no unreadable cues, followed by Chinese translation. The full OCR-plus-translation run completed in 369 seconds. English and Chinese WebVTT contained 697 identical timing intervals. Native AVPlayer loaded the live virtual MP4 in 0.33 seconds; English, Chinese, Off, forward, backward, and end-of-file seeks passed. This validates playback integration and timing, not a full human review of recognition or translation quality.

Deployed to Diamond on 2026-11-10, preserving the current schema-10 profiles release. Rollback bundle/database/config: `~/Library/Application Support/Torrbaby/backups/ocr-fallback-20261110-080411`. Existing queue work resumed. The validated Outlander S01E01 captions were retained and attached to its library inventory. No client release is required. Local evidence: `local-state/ocr-production/check.log`, `final-test.log`, and `live-native-check.log`.
