# Follow-up work

## Seinfeld library organization

Completed 2026-11-10: production catalog returns one Seinfeld title containing
171 video assets across nine seasons. Asset IDs and nine existing profile history
rows were retained. The full-pack metadata had an exact title/year match, but
the inheritance path required manual confirmation; individual episode searches
remained in review. High-confidence TV matches now propagate to same-title
episodes without requiring confirmation. Files did not need to be moved.
Signed server build deployed; backup `china-search-20261110-100820`.

Queued by the user after the search proxy work on 2026-11-10.

- Investigate why Seinfeld imported as scattered library entries.
- Correct show matching and episode/season grouping so it appears as one show.
- Inspect actual files and metadata before changing anything; preserve watch history and active Transmission seeding.
- Verify the corrected library on clients and address the import cause to prevent recurrence.

## English subtitles for Chinese content

Audit completed 2026-11-10; see `subtitle-language-audit.md`. Chinese-to-English
text translation selection exists independently of search market. A focused
regression check passed. No Chinese-only text-subtitle asset currently exists in
the scanned catalog, so a real reverse-translation job remains to be verified.

Remaining follow-up: recognize existing full English bitmap subtitles before
generating a new English translation; prefer extracting those with OCR where
needed for playback. Forced-only subtitles also need separate handling in the
language-availability check. Do not discard existing captions.

Queued after Seinfeld cleanup by the user on 2026-11-10.

- Check whether Chinese content added through China search gets queued for English subtitle translation when English subtitles are absent.
- Trace search-market provenance, detected media languages, and subtitle queue eligibility; compare China and World imports.
- Base translation eligibility on actual language availability, so Chinese content from either market is handled and existing English subtitles prevent unnecessary generation.
- Verify the Chinese-to-English generation path and generated-language reporting, and fix any gaps found.
