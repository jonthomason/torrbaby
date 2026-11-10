# One processing queue per media file

Diamond tracks each complete, visible movie or episode in SQLite's
`media_processing` table. The file revision and processing-policy version identify
what was signed off. A changed file reopens the record; an unchanged completed file
is not processed again.

The ordered stages are language inventory, English/Chinese subtitle extraction and
translation, then rolling-credit detection. Extraction precedes translation, which
reuses the extracted cues. Each file remains claimed through its stages. Explicit
failures appear under Needs attention, with a retry time; they never count as finished.
Resource deferrals remain unfinished. Existing title/episode priorities order waiting
work, with claimed files finishing first.

SQLite stores the overall state, current stage, each stage's outcome, cue progress,
error, retry time, attempts, and final completion time. A guarded SQL update only
signs off the current revision once inventory is ready and both subtitle and credit
stages are ready or explicitly not applicable. A missing supported subtitle source
is recorded as not applicable and shown in the client; it is not reported as translated.
Credit detection may successfully find no credits. An incomplete scan is a failure.

The subtitle/VTT files and translation checkpoint files remain reusable artifacts;
they no longer define membership or completion of the user-facing queue. The API
returns exactly one row per file from the database at `/v1/processing`. The existing
`subtitle_preparation` status field exposes the same combined queue for compatibility.
The old independent subtitle, credit, and pre-index schedulers have been replaced by
one coordinator; credit analysis prepares the needed video index as part of that file.

Migration preserves artifacts and adopts unfinished subtitle work. It also recognizes
older OCR artifacts keyed under `auto` after their language was detected. Existing
valid translations and credit results are reused. New sign-off counts therefore start
low while existing files are checked; this does not mean their translations were lost.

Validation: focused stage/sign-off/revision/recovery tests, extracted-cue roundtrip,
legacy OCR-cache reuse, and the existing subtitle selection tests. Live verification
confirmed 445 unique rows for 445 files and a file advancing through translation,
credit detection, and sign-off before the next file was claimed. Apple queue labels
are maintained in the native EN/ZH translation tables.

## Playback-aware concurrency

Two files advance concurrently when idle. A recent authenticated playback heartbeat
(within 90 seconds) reduces admission to one; paused players conservatively retain
that reservation until their heartbeat expires or the session closes. Virtual-media
preparation pauses new subtitle jobs. Capacity is refreshed every two seconds even
while a stage is running. Credits scans yield to active playback. During urgent virtual-media preparation, running subtitle futures are cancelled;
helper processes use kill-on-drop. Jobs persist as interrupted and are admitted
again once preparation clears. Translation checkpoints are reused; unfinished OCR
may restart. Ordinary active playback lowers admission without killing jobs.

Failures are retained in processing_failures with file revision, stage, message,
occurrence count and first/last timestamps. Successful completion marks the record
resolved but retains its history. GET /v1/processing/failures exposes records and
error groups so repeated systemic failures are visible. One failed file does not
prevent other files from progressing.
