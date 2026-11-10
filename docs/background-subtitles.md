# Daemon-owned subtitle preparation

The Rust server scans completed, available library files in the background. This
continues without any open client or AI session. `media_inventory` in SQLite
(schema9) stores the source revision, audio/subtitle inventory, preparation IDs,
retry time and ready results. A changed file invalidates its inventory. Jobs and
WebVTT output persist in the server's subtitles directory.

Every20seconds the worker refreshes job states, inventories at most one new file,
and admits one title's original+translated subtitle pair when the subtitle queue
is idle. Preparation continues during playback (build74); one extraction/translation
runs at a time. Playback does not pause admission.
Failures wait an hour before retry. Interrupted jobs are admitted again after a
restart; completed work is reused by revision and track/language identity.

For usable English text without Chinese, generate Simplified Chinese. For Chinese
text without English, generate English. Preserve native tracks and label generated
ones separately. Unknown languages are not guessed; forced-only, bitmap-only and
unsupported text tracks are not translation sources. No OCR. English SRT sidecars
are inventoried but not yet automatically translated by this embedded-track worker.

Subtitle storage is bounded at512MiB (16MiB admission headroom) and4096records.
Ready translations are retained rather than evicted to make room for another
translation. Full storage stops admission; there is no automatic deletion of
original media/subtitles. The helper uses installed Apple Translation models.

## API

- `GET /v1/library/assets/{id}/media-info`: state, audio_tracks, subtitle_tracks,
  prepared, preparation_state. Prepared entries include language, generated,
  state, id, and the authenticated WebVTT path.
- `GET /v1/library`: each main asset includes `media_info` with the same data.
- `GET /v1/settings`: includes `background_subtitles`.
- `PATCH /v1/settings` with `{"background_subtitles":false}` stops future
  background admission; true resumes. In-flight work can finish. Configuration
  defaults to enabled; all these routes require ordinary client credentials.

Apple title pages show audio/subtitle language labels and a Translated tag only
for ready generated output. Pressing Play no longer requests translation: it
reuses a prepared target or continues without it with a notice. Extracting an
existing text track and preparing its HLS package may still happen on demand.
Native MP4 subtitle playback works; MKV caption generation is useful preparation,
but MKV remux is still required for native Apple delivery. Offline downloads do
not yet bundle the generated captions. These remain active work, not completed
format support.

## Progress and restart recovery (build 71)

Translations checkpoint after each batch (up to32cues and128KiB of source text)
using an atomic cache-file rename. Earlier versions used eight-cue batches.
On restart, the worker validates cue timing and resumes at the saved batch; ready
output removes the checkpoint. A focused 24-cue interruption check verified that
the first saved batch was not translated twice. Source files remain unchanged.

`GET /v1/status` includes `subtitle_preparation`: scanned/translated/failed counts,
waiting_for_idle, and current asset/language/completed_cues/total_cues. Apple
Settings now shows this progress and an automatic-preparation toggle on Mac, iOS
and tvOS. Disabling admission lets the current job finish.

Matroska UTF-8/ASS extraction tolerates trailing NUL padding from muxers. Interior
NUL characters remain invalid, and empty padding-only cues are skipped. A real
previously failing title extracted all 1,515 cues after this correction.

SRT sidecars are decoded by `server/text_encoding.rs`, not assumed to be UTF-8:
BOMs (UTF-8/UTF-16), BOM-less UTF-16, mostly-UTF-8 files with stray bytes, and
legacy code pages chosen from the filename language label (Windows-1252 for
untagged Western text). Text that looks double-byte is decoded only as GB18030 or
Big5 when common-character scoring is convincing, otherwise extraction fails
rather than producing mojibake. Release adverts (YTS/YIFY, "synced by", …) are
dropped as cues; an unlabelled sidecar containing only adverts is not inventoried.
Inventory version 4 rescans unfinished assets so earlier undecodable sidecars are
relabelled.

## Queue and validation (build 74)

Mac and mobile have a Subtitles page below Complete: Working, Waiting, Finished,
and Needs attention, with cue progress. Status includes a `queue` array with ID,
asset ID, title, state, source/target language, completed/total cues and error.
Manual preparation jobs are included alongside background jobs. Stale source
revisions and expired output are excluded from current counts.

Before a job becomes ready, the daemon checks source revision, cue count, exact
original start/end times, positive cue duration, nonempty bounded text and absence
of NUL characters. Text is escaped for WebVTT; source videos are never modified.
These structural checks do not certify translation quality or visual sync.

Live verification: 21 translated artifacts matched original timing lines exactly.
A real MP4 package with generated Chinese passed native AVPlayer readiness and
English/Chinese track selection. MKV delivery and visual sync acceptance remain
separate work. AI review of Homeland is deferred at the user's request.

Batch sizing was measured on Diamond using 32 owned sample sentences while the
catalog worker remained active: eight-cue batches took14.09/13.68seconds; one
32-cue batch took7.95/7.30seconds. This measures helper/session overhead for this
sample, not a guaranteed catalog-wide speedup. Request IDs/order and output count
passed in all runs. Worst-case JSON text escaping remains below the1MiB input
limit with the128KiB source-text cap; helper output and cue bounds are unchanged.
