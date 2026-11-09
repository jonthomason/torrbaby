# Daemon-owned subtitle preparation

The Rust server scans completed, available library files in the background. This
continues without any open client or AI session. `media_inventory` in SQLite
(schema9) stores the source revision, audio/subtitle inventory, preparation IDs,
retry time and ready results. A changed file invalidates its inventory. Jobs and
WebVTT output persist in the server's subtitles directory.

Every20seconds the worker refreshes job states, inventories at most one new file,
and admits one title's original+translated subtitle pair when the subtitle queue
is idle. It starts no new scan/translation while an unended playback session has
reported progress within90seconds. An already running job may finish; readiness
continues to update while watching. One extraction/translation runs at a time.
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

Translations checkpoint after every eight cues using an atomic cache-file rename.
On restart, the worker validates cue timing and resumes at the saved batch; ready
output removes the checkpoint. A focused 24-cue interruption check verified that
the first saved batch was not translated twice. Source files remain unchanged.

`GET /v1/status` includes `subtitle_preparation`: scanned/translated/failed counts,
waiting_for_idle, and current asset/language/completed_cues/total_cues. Apple
Settings now shows this progress and an automatic-preparation toggle on Mac, iOS
and tvOS. Disabling admission lets the current job finish.
