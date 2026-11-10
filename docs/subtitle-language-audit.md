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

## Remaining edge cases

- Availability currently counts text-extractable tracks only. Chinese text plus
  existing English PGS/VobSub can trigger an unnecessary English translation.
  Prefer OCR of the existing English track where a text rendition is needed.
- The availability predicate counts forced-only text tracks, although source
  selection excludes forced tracks. Full and forced subtitles must be separated
  before deciding that a complete target-language subtitle already exists.
- Language normalization folds Chinese variants to `zh-Hans`; traditional-only
  subtitles need a deliberate simplification policy rather than silent relabeling.

These cases are recorded for follow-up, not claimed fixed by this audit.
