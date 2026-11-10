# Chinese-to-English subtitle queue audit

2026-11-10, after search proxy deployment and Seinfeld repair.

`server/media_inventory.rs::subtitle_source` chooses Chinese → English when
Chinese text subtitles exist and English text subtitles do not. It chooses
English → Chinese in the opposite case and skips translation when both exist.
The queue is based on complete, available library assets and detected tracks;
it does not consult China/World search provenance. Supported scanned containers
are MKV, WebM, MP4, M4V, and MOV. Content needs a usable subtitle source; audio-only
speech transcription is not implemented.

The worker accepts `zh-Hans` → `en` and delegates translation to the Diamond Apple
Translation helper. Auto-detected source language also reverses the target when
the source is Chinese. Generated jobs are cached against the asset revision.

Validation: the focused test `chinese_text_gets_english_only_when_missing` passed,
covering Chinese text → English and existing English text → no duplicate job.
Production inventory had 21 assets with Chinese text subtitles, all also with
English text subtitles. No English-target jobs existed. Therefore code-path
eligibility is verified, but a real Chinese-only catalog job has not yet been
observed end to end. No content was downloaded solely for this audit.

## Completed follow-up

Deployed 2026-11-10, backup `china-search-20261110-101532`:

- Full English PGS/VobSub captions now count as an existing English subtitle.
  With Chinese text present, the queue extracts the English bitmap track instead
  of translating Chinese into English. Both languages in bitmap form produce two
  extraction-only jobs. Text is preferred within each language.
- Forced-only captions do not satisfy full-language availability and are not used
  as the translation source.
- Three focused queue regression checks passed, including the existing text/OCR
  precedence checks. Signed release build and bundle verification passed.
- Diamond's production Rust `prepare-subtitles` command translated an authored
  three-cue Chinese SRT sample through Apple Translation into fluent English,
  preserving all cue IDs and start/end timestamps. This used scratch files under
  `/tmp`, without adding a fake title to the family library.
- Pending production subtitle work was resumed after deployment. Seinfeld still
  grouped as one title with all 171 assets, and nine profile history rows remained.

No Chinese-only catalog title has yet exercised the entire automatic workflow;
queue selection and the real Diamond translation path were verified separately.
The source must contain extractable text or supported bitmap captions; this does
not implement audio transcription. Chinese variants currently use the helper's
`zh-Hans` language identifier; native subtitle tracks are not rewritten by this
queue change. Traditional-to-Simplified conversion is not a new feature here.
