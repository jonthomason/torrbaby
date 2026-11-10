# Follow-up work

## Seinfeld library organization

Queued by the user after the search proxy work on 2026-11-10.

- Investigate why Seinfeld imported as scattered library entries.
- Correct show matching and episode/season grouping so it appears as one show.
- Inspect actual files and metadata before changing anything; preserve watch history and active Transmission seeding.
- Verify the corrected library on clients and address the import cause to prevent recurrence.

## English subtitles for Chinese content

Queued after Seinfeld cleanup by the user on 2026-11-10.

- Check whether Chinese content added through China search gets queued for English subtitle translation when English subtitles are absent.
- Trace search-market provenance, detected media languages, and subtitle queue eligibility; compare China and World imports.
- Base translation eligibility on actual language availability, so Chinese content from either market is handled and existing English subtitles prevent unnecessary generation.
- Verify the Chinese-to-English generation path and generated-language reporting, and fix any gaps found.
