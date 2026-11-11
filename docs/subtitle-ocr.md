# Bitmap subtitle fallback

Torrbaby's Diamond daemon can recognize DVD-style VobSub and Blu-ray PGS captions inside MKV files using Apple Vision, then translate the recovered text with its existing Apple Translation helper. This runs in the background subtitle queue; no AI session is required.

## Selection

- Embedded text and separate text subtitle files always take precedence.
- OCR is admitted only when no text track or sidecar is available. The worker checks this again before decoding.
- Non-forced English or Chinese bitmap tracks are preferred; unlabeled bitmap tracks can be recognized and its language inferred. Unsupported or unverified languages fail rather than being labeled English.
- English produces Simplified Chinese; Chinese produces English. Existing original text captions remain the preferred input.
- Standalone SUP/DVD subtitle files, dynamic VobSub palettes, encrypted subtitles, and unsupported compression are not supported by this release.

## Preservation and resource use

Original media is never rewritten. OCR retains caption timestamps and publishes WebVTT using the existing subtitle job path. Both recovered captions and translations can be used by the native virtual MP4 player and offline packaging.

One subtitle worker runs at a time. Temporary grayscale images are limited to 256 MiB and removed after completion/failure; abandoned work is cleaned on startup. Validated recognized text is cached by source revision, track, and language hint, with a 128 MiB cap. Original-language and translated jobs reuse this cache. No full movie copy is created.

Decoding and recognition run as separate, time-limited processes. Malformed images/timestamps, source changes, decoding failures, unverified languages, or excessive unreadable captions fail the job without changing source media. A small number of unreadable images can be omitted (up to max(3, 1%)); the job reports `unreadable_cues`. OCR spelling mistakes remain possible.

## Queue and packaging

Jobs report `stage: ocr`, cue progress, and `extraction_method: ocr`. Prepared OCR captions and translations carry the tb marker already understood by clients. User reconfirmed on 2026-11-10 that OCR English should retain this badge; it means captions created by Torrbaby, not translation alone. Original embedded/sidecar text remains unbadged. Failed jobs retain the existing retry behavior.

The signed server bundle must include `torrbaby-subtitle-ocr` alongside the existing extractor, translator, and media segmenter. `scripts/verify_server_bundle.sh` checks this before deployment. Apple Vision requires no additional model service or API key.

## Acceptance

The focused text-first selection check covers embedded text, sidecars, unsupported text, bilingual text, and unlabeled VobSub fallback. The isolated full-episode acceptance uses Outlander S01E01: all 697 captions were recovered with no unreadable cues, followed by Chinese translation. The full OCR-plus-translation run completed in 369 seconds. English and Chinese WebVTT contained 697 identical timing intervals. Native AVPlayer loaded the live virtual MP4 in 0.33 seconds; English, Chinese, Off, forward, backward, and end-of-file seeks passed. This validates playback integration and timing, not a full human review of recognition or translation quality.

Deployed to Diamond on 2026-11-10, preserving the current schema-10 profiles release. Rollback bundle/database/config: `~/Library/Application Support/Torrbaby/backups/ocr-fallback-20261110-080411`. Existing queue work resumed. The validated Outlander S01E01 captions were retained and attached to its library inventory. No client release is required. Local evidence: `local-state/ocr-production/check.log`, `final-test.log`, and `live-native-check.log`.

Post-deployment check: the live episode inventory exposes generated `en` and `zh-Hans` tracks (697 cues each). Outlander was prioritized through the normal queue API; the next automatic OCR job advanced to 273/669 cues, confirming daemon-owned fallback scheduling and live progress. Server status had no error.

## VobSub colour mapping

DVD SET_COLOR/SET_CONTR store nibbles as `emphasis2 emphasis1 pattern background`. oxideav-sub-image 0.0.9 keeps them in wire order but labels slot 0 as background, so Torrbaby looks up 2-bit pixel `p` at slot `3 - p` (matching libavcodec). Before this fix, DVD tracks using the common white-fill/black-outline layout (e.g. the Spirited Away featurettes) rendered only scattered dots and failed as "OCR could not read these subtitles"; the white fill now becomes dark ink on white.

## PGS decoding

PGS display sets are assembled across Matroska blocks and passed to the existing MIT-licensed oxideav-sub-image decoder. It handles fragmented objects, composition/cropping, palettes, windows, and epoch resets. The adapter uses [Matroska block timestamps and durations](https://www.matroska.org/technical/subtitles.html), with subsequent replacement/clear displays ending captions. It never invents an end time for an unterminated final caption.

Transparent canvas margins are cropped before OCR, preserving the relative positions of multiple objects. Identical consecutive displays are merged only when their timing is contiguous. Canvas/object dimensions, declared fragment lengths, referenced objects and palettes, epoch storage (64 MiB), display-set size (16 MiB), and total output storage are checked before publishing captions.

Focused checks cover text precedence over PGS, segmented display assembly, transparent-margin cropping, clear events, duration precedence, and rejection of an unterminated final caption. Outlander S02E01 decoded into 762 nonoverlapping captions, using 39 MB of temporary grayscale images. OCR recovered 761 English captions; the omitted image was visually verified to contain only musical-note symbols. The final guarded decoder produced identical images and timings.

PGS deployed 2026-11-10; rollback `~/Library/Application Support/Torrbaby/backups/pgs-fallback-20261110-084806`. Live Outlander S02E01 exposes English and Simplified Chinese (761 captions each); native language selection and forward/back/end seeking passed, ready in 0.33 seconds. Retained this validated episode and requeued the other 28 PGS assets. Subsequent caption provenance deployment backup: `caption-provenance-20261110-085716`. Pending jobs resume using their original auto-language request identity to preserve checkpoints.

### Apple player translation menu

The user reproduced a third Chinese option on iPad after relaunch: the supplied Simplified Chinese track and Apple's regional Simplified Chinese option marked Translated. Cached S01E01 streams each contain one English and one Chinese caption track; Torrbaby does not author machine-generated/translation media characteristics. Apple's [WWDC26 session](https://developer.apple.com/videos/play/wwdc2026/256/) explains automatically available on-device translation of English subtitles and the Translated label/icon. The virtual MP4 now preserves `zh-Hans` through an `elng` atom rather than reducing it to generic `zh`; this improves the language label but did NOT suppress Apple's extra option in the physical iPad screenshot. No supported suppression mechanism was found in the inspected public APIs. Do not claim the duplicate system menu option is fixed, nor that the original source had Chinese subtitles. Native AVKit still chooses its localized display wording. Experimental track-name metadata had no effect and was removed.

### Client release 96 / Android 27

Mobile OTA 96 published to the Mac-only Miyu Install page, macOS 96 installed, tvOS simulator 96 built, Android 27 published and installed in emulator-5554. Detail actions are exposed directly; the primary Download control is not duplicated. Apple title-detail Chinese names use Chinese (Simplified). The media-info response retains verified OCR source languages and correct provenance for cached jobs.

These client edits depend on the other session's uncommitted profiles/file-actions baseline. The reviewed incremental client patches are saved in `local-state/pgs-production/*-ui.patch`; working-tree files and published builds contain them. Do not discard or overwrite that baseline. Source/build evidence: `local-state/pgs-production/`; public production decoder commit `6ed3325`.

## Compressed tracks, placeholders and failure reporting (2026-11-11)

Matroska ContentEncoding is now undone in one place (`matroska::decode_frame`)
for text extraction and bitmap OCR: zlib and header stripping, in descending
ContentEncodingOrder, bounded, for frames and (VobSub) CodecPrivate.
Encryption, bzlib, LZO and scope-4 encodings are reported as unsupported.
`matroska-subtitles` adds `ocr_supported`; `extraction_supported` covers text
only, so it is always false for PGS/VobSub. Compressed text tracks are no longer
skipped.

Friends S09 "Phoebe Battles the Pink Robots": all 14 PGS tracks are zlib
compressed (mkvmerge's default) and zlib was already handled. The two Japanese
tracks decode to 12 readable captions. The other 12 tracks, including the
unlabelled English and the Chinese track the queue picked, are placeholders: one
600x64 display from 0 to 10.01 s using a palette entry that the palette never
defines, so it is fully transparent. The decoder found no visible caption. The
OCR wrapper reported every decoder failure as "OCR could not read these
subtitles". Decoder failures are now reported as such. A track with no visible
captions fails with "This subtitle track has no visible captions". The queue
then treats that track as no source, and the title as not applicable when
nothing else remains.

The asset's alternating "No such file or directory (os error 2)" came from the
shared subtitle directory. Running jobs rename `*.json.tmp` files there, and
the OCR helper atomically replaces its progress file, while `prune` and the
translation storage check listed the directory and then called `metadata()` on
each entry. The same race existed in the OCR cache eviction. Entries that vanish
mid-scan are now skipped. No log line survives, so this cause is inferred from
the code and the failure pattern. Eleven other assets hit this error once each
between 23:13 and 01:12 UTC in the extraction, OCR and translation stages, while
their media still existed.

A real compressed English PGS track (Outlander S02 deleted scene "Faith")
decoded and was recognized by the production OCR helper: 83 captions, none
unreadable.
