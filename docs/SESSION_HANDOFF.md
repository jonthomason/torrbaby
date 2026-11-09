# Torrbaby handoff — 2026-11-09

Read this before continuing. The user requested a stopping point before clearing
context. The full persistent goal is still active and NOT complete. Do not infer
completion from the deployed Rust cutover or the current client builds.

## Current release boundaries (updated 2026-11-09, build 54)

- Repository: `~/src/torrbaby`, public MIT, `github.com/jonthomason/torrbaby`.
- `main` fast-forwarded through `a5ee609` for the early Downloads release.
  `codex/offline-downloads` remains as the development checkpoint branch.
- **Mac build 54 installed and running**, exactly one app process at
  `~/Applications/Torrbaby/Torrbaby.app`. CUA verified Connected and the Downloads
  page. Previous build37 archived at `local-state/releases/Torrbaby-mac-37.zip`.
- **Mobile build 54 published** to the Mac's Miyu Install host only:
  https://jons-m5-mbp.bengal-locrian.ts.net/ota/torrbaby/index.html
  IPA SHA256 `5f6321e6542395f7c495139279f79ee1ac7a70e4d2fcb8e686d263e50b9b6b86`.
  HTTPS IPA hash, current.json54, and hub entry verified. Profile covers six
  devices and expires 2027-11-09. No Bronze publishing.
- Source for build54 is `a5ee609`; later documentation commits do not change it.
  Future builds must use a number greater than54.
- Includes QR landscape, native full-screen/PiP, mobile torrent rows, and early
  Downloads: pause/resume, quota controls, cached posters, local-first playback,
  same-season episode continuation, delete after actual local completion, and
  bidirectional watch-position sync. Native device background transfer/PiP still
  needs real-device acceptance; no claim of universal container playback.
- Diamond is on the verified schema7 Rust server with backup; see production
  section. Subtitle preparation/discovery/translation CLI exist, but selectable
  translated player captions are NOT yet implemented.
- The old detached release worktree `/tmp/torrbaby-qr-landscape-release` remains at
  build42 source; do not use it for a current release without updating explicitly.
- Mac/iOS simulator/device builds passed, along with focused scratch fixtures.
  No permanent broad test suite was added. No test media was added to live library.
- tvOS27.2 runtime is installed; physical Apple TV pairing still unresolved.
- Follow-up entries below are chronological history and may describe earlier WIP
  status. This section and the latest deployment/release entries take precedence.

## User direction and priorities

- One shared headless Rust daemon on Diamond for torrent management, file catalog,
  metadata, playback state, streaming, subtitles/translation and future download
  preparation. No Python runtime work, Plex runtime dependency, or web UI.
- Native Apple player UI only. No custom player chrome. iPad opens full-screen;
  native close returns to the title page. Picture in Picture is critical.
- Read `docs/interface.md`: images and useful controls first, fewer words
  everywhere. No decorative slogans, implementation paragraphs, or prototype
  language. Keep errors, destructive consequences and required attribution clear.
- iPad/iPhone torrent rows must look as polished as Mac. Build 42 uses compact
  tablet columns and narrow-screen cards, with stable numeric slots, thumbnails,
  colored rates and percentage gauges. Mobile defaults to compact using the new
  `compactMobileTorrentView` preference. Per-torrent graphs remain in details,
  not in the mobile list. Physical-device visual validation still outstanding.
- User likes shared playback positions across devices; preserve them carefully.
- Automatic naming only for high-confidence/confirmed matches, through
  Transmission so seeding continues. Whole-torrent Movies/TV sorting.
- Separate torrent states: Downloading = incomplete (including paused/stalled),
  Seeding = completed and active seeding, Complete = other completed. Library Home
  is the landing page. Seeding target defaults to 1.2. No automatic media deletion.
- Downloading LOCAL COPIES to Mac/iPhone/iPad is a separate new feature: tight
  per-device storage cap and free-space reserve, pause/resume/cancel, offline
  metadata/art/subtitles/progress, native offline playback. Delete a local copy
  after actual playback completion; never merely on pause/dismissal or the 95%
  watched threshold. Persist progress before deletion. Keep downloaded pins it.
  After an episode finishes: delete its local copy, then next local episode or
  stream it if reachable. Never skip missing episodes. No server-file deletion.
- QR pairing had higher priority than downloads; it is now released. Any linked
  Mac/iPhone/iPad can show a code. Diamond itself stays headless. Mobile setup is
  QR-only, centered scan button with scan icon; no address/token form.
- Translated captions are NOT implemented. User expected Apple Translation;
  explicitly clarified this. Translation belongs on Diamond, in both directions
  English ↔ Simplified Chinese. Text-extractable subtitles only, no OCR/burned-in.
- Integrated indexer search is still requested, queued: server-backed direct JSON
  providers, optional Torznab, explicit user add. Do not copy GPL implementation.
- Fast iteration/restarts of Torrbaby authorized, minimal/no formal new tests.
  Commit and push regularly. Permissively licensed Rust crates are authorized.
  Don't alter network topology/firewall/Tailscale settings without approval.
- OTA only on the Mac through Miyu Install; NEVER publish Torrbaby to Bronze.

## Deployed state

Diamond production runs Rust 0.3.0 under `org.torrbaby.daemon`:

```
~/Applications/Torrbaby Server.app/Contents/MacOS/torrbaby serve \
  --config ~/Library/Application\ Support/Torrbaby/config.json
```

- Listener: `100.80.147.115:8765` (existing Tailnet address).
- Transmission local RPC: `http://127.0.0.1:9091/transmission/rpc`, v4.1.4 RPC2
  snake_case protocol. Media roots `/Volumes/Alpha/Media/{Downloads,Movies,TV}`.
- Config, state.sqlite3, api.token, tmdb.token and rust-server.log are in
  `~/Library/Application Support/Torrbaby/`. SQLite schema version **7** (updated 14:31 on 2026-11-09).
- Signed bundle ID `org.torrbaby.server`, Developer ID team **23PTSAZN2G**.
  User granted file access; direct range reads were verified from media volumes.
- Python launch agent was replaced, not run alongside Rust. No Rust Plex calls.
  Legacy cached Plex-relative artwork can be read from disk, never fetched.
- Initial cutover backup: `backups/pre-rust-20261109-125138`; latest server update
  backup `backups/rust-20261109-143105` includes the previous signed bundle and schema-6 SQLite.
  Rolling back schema 7 requires restoring that corresponding DB backup.
- Latest deployed Rust includes original-file transfers, sidecar subtitle APIs,
  shared progress read/offline sync, and the signed Apple translation helper
  (server source through 85dfd5c, unchanged in e4ecef0). Live library, history,
  progress/subtitle APIs and a 1 KiB media range verified after deployment.
- Standalone-server goal still needs a broad final acceptance audit, including
  cold startup/catalog/artwork/playback/progress without Plex available. Do not
  claim this broad goal complete based on a single range-read check.
- Catalog/naming refinements, subtitle pipeline, indexer search, complete offline
  downloads and polished tvOS behavior still remain. Some series/collection
  matching/grouping is imperfect; don't equate missing metadata with an extra.

## Stable feature evidence

- QR: `server/pairing.rs`, `Apple/Torrbaby/Pairing.swift`. Five-minute one-use
  random grants; SHA-256 digests in pairing_grants/device_credentials. Device gets
  its own token and UUID, saved to Keychain. Any linked device can issue grants.
  Cross-origin redemption is rejected. Paired credentials can be revoked.
  Existing shared-token devices cannot be revoked individually. Already-issued
  six-hour playback URLs still work until expiry after revocation.
- QR smoke verified unauthenticated denial, one-use redemption, expiry, origin
  rejection, issuance by another paired device, and revocation. Mac QR screen
  was opened and showed live expiry. Actual camera scan was exercised by user;
  landscape bug prompted build 41's fix, not yet physically reverified by us.
- QR rotation: AVCaptureDevice.RotationCoordinator + preview connection rotation,
  resize without layer animation, AnyLayout switches vertical/horizontal scan
  arrangement, inline navigation title, scrollable connection confirmation.
- Mobile playback: `MobilePlayerPresentation.swift` directly presents native
  AVPlayerViewController full-screen; retains coordinator/model through PiP;
  handles native dismissal and restoration. `UIBackgroundModes = [audio]` added
  by generate_xcode.py; existing audio session playback mode retained. Build 41
  and 42 signed successfully. Real-device PiP/full-screen exit needs feedback.
- Navigation: window-scoped history for sidebar + typed library/torrent paths,
  Cmd-[ / Cmd-], Back/Forward toolbar, local mouse buttons 3/4. Keyboard Back and
  Forward verified on Mac. Mouse hardware not verified. Cmd-comma works.
- Transfer rows: stable slots, right-aligned colored rates, singular peer, 18pt
  in-bar percentage, thumbnails via authenticated artwork. Rust prefetches posters
  during metadata collection. Collection uncertainty can still mean placeholder.
- Rust metadata, automation/naming, progress, rate/ETA persistence, toasts/local
  mobile notifications, magnet normalization, specials grouping already deployed.
  No APNs/push infrastructure.

## OTA and building

Miyu Install hub:
`https://jons-m5-mbp.bengal-locrian.ts.net/ota/`

Torrbaby direct page:
`https://jons-m5-mbp.bengal-locrian.ts.net/ota/torrbaby/index.html`

- Current OTA: **0.2.0 (42)**, verified hub and HTTPS IPA checksum:
  `d0c1cb3049a12cd0e54c70713e909160db6a1bd012dfb2aed674d434ecfadfcd`.
- Published directory `~/.cache/miyu/ota/torrbaby`.
- Development profile covers six devices, expires 2027-11-09.
- User installs in Safari with Tailscale; then Scan connection code. Linked app:
  Settings → Devices → Link a device. No ASC record needed for development OTA.
- Miyu hub entry was fixed in hosted PR **jonthomason/miyu#582**, merged, canonical
  main fast-forwarded to **16221a17**. `scripts/ota-host.py` reexecs on source change.
  Torrbaby has Mac install button only. Preserve other apps and Miyu TTS services.
  Miyu canonical checkout has unrelated untracked `backend/tts-qwen3/tools/timepitch/`;
  never remove it. Miyu changes require feature worktree + hosted PR, per AGENTS.md.
- Bundle IDs: `org.torrbaby.mac`, `org.torrbaby.ios`, `org.torrbaby.tv`.
- Mac installed path `~/Applications/Torrbaby/Torrbaby.app`.
- Build Mac:
  `MAC_SIGNING_IDENTITY='Developer ID Application: Jon THOMASON (23PTSAZN2G)' bash scripts/build_apple.sh macos`
- Build iOS: source `~/src/Miyu/scripts/asc.env` privately with set -a, then
  `DEVELOPMENT_TEAM=23PTSAZN2G bash scripts/build_apple.sh ios`.
- Xcode project generated by `python3 scripts/generate_xcode.py` (includes every
  Swift file and regenerates Info.plist). Keep generator changes with plist changes.
- `scripts/package_ota.py` packages/signature-checks, verifies registered device
  coverage and publishes atomically. Use current clean release source, not WIP.
- Root `local-state/apple/ios` is not necessarily the latest release: build 42
  was produced under the clean temporary worktree. Check Info.plist before publish.
- Keep future OTA build numbers >42. Branch commit counts can differ; set
  TORRBABY_BUILD_NUMBER explicitly when necessary.
- Server: `scripts/build_server.sh` requires Developer ID and emits signed bundle
  with MIT and permissive dependency notices. Zip/scp and use the preserved update
  script, which verifies signature, backs up DB/bundle and kickstarts only Torrbaby.

Never print credentials. Local connection profile, read programmatically:
`~/Downloads/Torrbaby/Connection/Diamond.json` (private). TMDB is already configured;
1Password entries TMDB_API_KEY / TMDB_READ_ACCESS_KEY are in Miyu Dev. Do not paste
those values into tools/transcripts or commit them.

## Apple Translation breakthrough — verified on Diamond

User confirmed Simplified Chinese installed. We verified more than availability:

```
en -> zh: installed
Translation: 我们要去哪里？
zh -> en: installed
Translation: Where are we going?
```

- Diamond reports macOS **27.2**.
- macOS 26+ supports a headless `TranslationSession(installedSource:target:)`.
  The signed Swift command-line helper successfully translated in both directions
  over SSH. No UI prompt or cloud translation service was needed for these checks.
- Probe source is preserved as `scripts/check_translation.swift`.
- Compile with `swiftc -parse-as-library -target arm64-apple-macos26.0 ...`, sign
  with Developer ID, scp to Diamond, run. Existing temporary binary is
  `/tmp/torrbaby-translation-probe` on Diamond. It only checks installed languages
  and translates synthetic sentences; it does not install models.
- Next: turn this into a narrow JSON/stdin Swift helper invoked by Rust, batch cue
  translation with IDs/timing preserved and deterministic cache/versioning.
- No ffmpeg/ffprobe binaries were found at /opt/homebrew/bin or /usr/local/bin on
  Diamond. Do not assume a demuxer is installed. Avoid dependence on Plex's bundled
  transcoder. Need text subtitle extraction (sidecars first and embedded tracks),
  normalization and native-player-compatible delivery, likely an HLS/WebVTT path.
- Apple native subtitle styling applies to WebVTT text rules, not arbitrary bitmap
  or every embedded format. No OCR. Translation and subtitle selection/styling
  remain unimplemented despite helper readiness.

## Offline download WIP — do not release without continuing

Stable foundation commits: a4a7c89 (Rust transfer), 8eccae8 (DownloadStorage).
Current branch adds DownloadQueue, DownloadsView and player integration.

Implemented in branch:
- `DownloadStorage`: owned UUID file paths; durable JSON manifest; 20 GB cap and
  5 GB free reserve defaults; pins; durable removal intent and recovery; playback
  state retained after local-file deletion. Root permissions 0700, excluded backup.
- `DownloadQueue`: URLSession background transfers, task restoration, serial
  admission, pause/resume/cancel, request auth and If-Match revision; resume
  context fingerprint checks server/token/Wi-Fi policy. Background app delegate.
- Downloads page and storage settings; title download menu for files/seasons;
  local playback in native player; downloaded episodes can auto-advance to the
  next same-season local episode or stream it. Never skip episode number gaps.
- `Playback` carries optional localDownloadID/libraryAssetID/libraryTitleID.
  `PlaybackModel` can load another item, records actual did-play-to-end separately
  from player close and 95% watched, releases local file before deletion.
- WIP branch uses the newly released MobilePlayerPresentation for iOS too.

Evidence and limitations:
- Current full branch builds for macOS and generic iOS Simulator, logs preserved.
- A real local HTTP fixture verified transfer, pause/resume, byte-exact payload,
  local playback URL, no delete at partial playback, delete at actual finish with
  progress retained. This was before subsequent next-episode/resume-context work;
  the harness's Playback stub needs the new optional identity fields before rerun.
- No WIP app was installed/published; no production movie was downloaded/deleted
  during testing. The fixture used synthetic bytes in a temporary directory.
- Offline progress is retained but NOT synced to Diamond yet. Need idempotent
  offline sessions/events and conflict handling that won't overwrite newer online
  viewing. Do not claim shared offline progress works.
- Cross-season autoplay is NOT implemented; need authoritative episode ordering
  so missing episodes aren't skipped. Current next-episode logic is same-season.
- Library Play still starts streaming rather than automatically preferring a local
  copy; Downloads page uses local copies. Needs unification.
- No offline metadata/artwork/subtitle cache yet; Downloads page uses film/TV icons.
  Add posters and saved title metadata, per the user's visual-first direction.
- Original-file downloads only; no compatible remux/transcode/preparation. Native
  player may reject original containers/codecs. Don't claim universal playback.
- Storage admission is deliberately conservative and can double-charge paused
  bytes; improve before claiming polished tight storage behavior. URLSession owns
  temporary files outside the app folder: UI accounting and crash reconciliation
  need attention. `.completed` staging files can be orphaned by a crash between
  delegate move and adoption; clean/reconcile only our own known UUID files.
- Verify cancel/finalization races, background completion callback ordering, stale
  resume data after credentials/network policy changes, persistence failures before
  delete, and rapid player/item transitions. A paused large file may be denied by
  conservative full-size re-reservation even when resuming should fit.
- Physical background transfer after suspension/termination and full-screen/PiP
  lifecycle have not been proven by simulator compilation. No permanent broad
  formal test suite required; use focused temporary checks and device feedback.
- Mac custom download folder/security-scoped bookmarks not implemented.

## Preserved local verification and operational files

`local-state/checkpoints/2026-11-09/` (ignored, private scratch area):
- macos-build.log / ios-simulator-build.log: final checkpoint compilation.
- torrbaby-storage-smoke.swift: quota, pin/delete/restart/progress retention.
- torrbaby-queue-smoke.swift + torrbaby-queue-fixture.py: HTTP queue fixture (stub
  fields need update for newest Playback identities).
- torrbaby-download-smoke.py: Rust auth/descriptor/HEAD/ranges/If-Match/If-Range,
  changed-file rejection, no playback DB effects.
- torrbaby-pair-smoke.py: grants, expiry, replay, origin and revoke checks.
- torrbaby-update-server.sh: atomic signed bundle + DB backup deployment routine.
- tvos-runtime.log: completed runtime download; contains carriage-return progress,
  so don't tail raw lines (can emit huge output). Runtime presence is authoritative.

The original /tmp smoke scripts/binaries also remain, but use these preserved copies
as references. Their absolute /tmp paths may need adjustment when rerunning.

## Resume sequence

1. Read this and docs/interface.md; inspect branch/status and actual device feedback.
2. Keep stable main/build 42 separate from the WIP download branch. Don't republish
   WIP in response to a narrow released-client bug.
3. User's latest priorities: verify mobile torrent polish, iPad full-screen/PiP,
   landscape QR; then subtitles/Chinese-English translation and offline downloads.
4. Apple Translation is now proven headlessly; next meaningful subtitle work is
   extracting/normalizing text and getting selectable native subtitles to clients.
5. Finish/review download persistence, storage accounting and shared progress before
   merging/releasing Downloads. Keep original full media-server scope intact.
6. Update this handoff as work progresses. Do not mark the persistent goal complete
   until the full requirements are actually verified.

## Follow-up: bounded translation helper implemented

- `helpers/SubtitleTranslator.swift` now implements the JSON stdin/stdout bridge;
  `helpers/README.md` documents limits, failure codes and integration contract.
- Compiled and Developer ID signed, copied only to
  `/tmp/torrbaby-subtitle-translator` on Diamond (no production restart/deploy).
- Live SSH checks passed: two English cues translated into Simplified Chinese in
  requested ID/order; a Chinese cue translated to English; duplicate IDs rejected
  with `invalid_request` and exit 1. No extra model installation was needed.
- Input is bounded at 1 MiB and 128 cues; only en/zh-Hans supported. No cue logging.
- Not bundled or wired into Rust yet. Next: server-owned text extraction,
  normalization, process timeout/cancellation, translation cache and native-player
  selectable subtitle delivery. This does not mean captions are already available.

## Follow-up: Rust translation process boundary

- `server/subtitles.rs` now validates and invokes the helper with bounded input
  (1 MiB), bounded output (4 MiB), a 120-second deadline, explicit kill/reap on
  errors, and kill-on-drop for cancellation. It checks protocol version and exact
  ordered cue IDs before returning translated text. No arbitrary helper error text
  is surfaced to logs.
- `torrbaby translate-subtitles [--helper PATH]` reads one JSON request from stdin.
  Default helper is adjacent to the Rust executable. `scripts/build_server.sh`
  now compiles and signs the Swift helper before signing the outer server app.
- `cargo check --locked`, `cargo build --locked`, shell syntax and diff checks passed.
  Actual Rust CLI -> SSH bridge -> Diamond helper translated a cue successfully;
  an intentionally wrong response cue ID was rejected. Timeout/cancellation logic
  is implemented but has not been exercised in a timing smoke check yet.
- No production deployment and no HTTP/client subtitle feature yet. Do not confuse
  working translation infrastructure with end-to-end translated captions.

## Follow-up: timed SRT translation/preparation

- `torrbaby prepare-subtitles --input FILE [--source en --target zh-Hans]
  [--helper PATH]` now emits WebVTT on stdout. Translation can run both directions;
  omitting languages only normalizes SRT. Full output waits for successful batches.
- Parser handles UTF-8/BOM and CRLF/LF, retains integer-millisecond timings and
  overlaps, flattens common SRT text formatting and escapes WebVTT payload markup.
  Strict limits/errors are documented in helpers/README.md. No legacy encoding,
  ASS/SSA or embedded-track extraction yet.
- This remains a CLI preparation stage: no automatic discovery/cache, HTTP
  rendition, HLS muxing, or selectable translated captions in clients yet.
- Current playback still streams original files directly. Native playback success
  for existing files does not prove arbitrary container/codec compatibility.
- Verification passed: cargo build --locked, format/diff checks; BOM+CRLF SRT with
  two overlapping cues translated through Rust -> SSH helper on Diamond and kept
  exact timing lines. Invalid minute values rejected with no stdout. The fixture
  used synthetic dialogue only; no library files changed. Production unchanged.

## Follow-up: authenticated subtitle discovery and WebVTT API

- Added GET `/v1/library/assets/{id}/subtitles` and GET the returned `.vtt` paths.
  Both require normal API authentication and completed/managed media validation.
- Exact-stem SRT sidecars only, with optional language/forced/SDH filename labels.
  Unrelated episode/movie names and symlinks excluded; list does not expose disk
  paths. Directory/track/file/cue limits apply. Blocking work uses spawn_blocking.
- Original-language WebVTT only. Response explicitly says embedded tracks have not
  been scanned. No translated rendition job/cache or native-player wiring yet.
- `cargo build --locked` and diff checks passed. Local HTTP fixture verified auth,
  exact stem matching, language/SDH flags, no disk-path leak, normalized timings and
  escaped text, unknown ID rejection, symlink replacement rejection, and no
  playback-session side effects. Scratch fixture saved to
  `local-state/checkpoints/2026-11-09/torrbaby-subtitle-smoke.py`.
- No production deployment or app release for this work.

## Follow-up: offline recovery and deletion safety

- `DownloadStorage` startup now removes only UUID-named `.completed` staging
  leftovers, validates ready files are regular/exact-size, marks missing/truncated
  files failed while preserving watch progress, and removes uncommitted media
  files for known non-ready records. Resume files and unknown filenames remain.
- `DownloadQueue.report`/`PlaybackModel.report` now return whether local progress
  was durably saved. Native end-of-playback only invokes delete-after-playback
  when that exact progress write succeeded, preventing stale completion flags
  from causing deletion after a persistence failure.
- Mac and iOS Simulator builds passed. Original storage smoke passed. New scratch
  `local-state/checkpoints/2026-11-09/torrbaby-storage-recovery-smoke.swift` verifies
  orphan cleanup, unknown/resume-file preservation, truncated-ready rejection,
  durable progress retention, and idempotent restart.
- Shared offline progress sync is still pending; no endpoint/outbox flush added in
  this change. Also still pending: normal Library Play preferring local media,
  offline artwork/metadata/subtitles, storage reservation improvements, cross-
  season ordering, and real-device background transfer/PiP checks.
- These app builds remain WIP and were not installed or published. Stable OTA42
  and installed Mac37 unchanged. Production Rust unchanged.

## Follow-up: durable offline progress upload

- Rust `/v1/playback/offline` + schema 7 receipt table now implemented (not deployed).
  Uses event UUID/client identity, resolves catalog media identity without file reads,
  accepts newer observation times, acknowledges stale reports without applying,
  defers newer reports during a recently active streaming session, deduplicates
  retries, and updates shared position/watched state without changing any media.
- App records separate progress UUID/timestamp, retains progress after local file
  deletion, retries on report/refresh when connected, and acknowledges only the
  matching event. Old in-flight replies cannot clear a newer local report.
- See docs/offline-progress.md for protocol, clock-skew policy and limitations.
  Bidirectional pull of newer shared progress before local playback still pending;
  this change is upload-only. Clock ordering assumes reasonably correct devices.
- Rust build and scratch HTTP smoke passed (auth, idempotency, newer/stale progress,
  active-session deferral/retry, completion, invalid future clock, schema 7).
  Swift storage smoke passed retention after deletion/restart and acknowledgment
  race handling. Scripts saved under local-state/checkpoints/2026-11-09 as
  torrbaby-offline-progress-smoke.py and torrbaby-storage-sync-smoke.swift.
- Production remains schema 6 and existing released server/apps. Schema 7 needs
  backup before deployment; rolling back to an older binary requires its DB backup.
- Mac and iOS Simulator builds passed for offline sync. Per-item sync failures are
  isolated so a removed catalog asset does not block other pending reports.

## Follow-up: shared resume pull and local-first Library Play

- Authenticated GET `/v1/library/assets/{id}/progress` returns shared position,
  duration, watched and last_played without requiring media filesystem access.
- DownloadQueue.preparePlayback fetches it only when connected to that download's
  server, with a 3-second request timeout, and falls back to the saved local
  position on errors. Storage imports strictly newer observations only; newer
  local reports remain pending. Imported watched never sets playbackFinished,
  so it cannot trigger delete-after-local-playback.
- Library Play now prefers a ready local copy. Downloads Play and same-season
  local next-episode playback use preparePlayback as well. Downloads Play prevents
  duplicate button actions while preparing. This does not add offline library
  artwork/metadata browsing or cross-season episode ordering.
- Rust, Mac and iOS Simulator builds passed. Extended HTTP smoke verified progress
  read authentication/content. Swift import smoke verified older shared progress
  cannot overwrite newer local progress, newer shared progress supersedes local
  pending state, and shared watched does not delete a local copy. Saved as
  local-state/checkpoints/2026-11-09/torrbaby-storage-import-smoke.swift.
- All still on WIP branch; no app installation/OTA or Diamond deployment. Physical
  disconnected/reconnected device acceptance remains pending.

## Follow-up: download quota accounting

- Storage now reports owned files plus tracked URLSession partial bytes outside its
  folder, subtracting any tracked partial already counted inside the folder.
  Admission reserves a resumed item's final size once instead of partial + full.
  Other paused downloads remain accounted. Disk-free admission still reserves a
  full restart plus metadata allowance conservatively.
- Transfer callbacks refresh displayed usage and check quota/free space at most
  once a second. Final adoption includes other partial downloads in quota checks.
  Fresh requests clear stale transfer counts; restored background tasks supply
  their current byte counts. Completion/error clears discarded byte counts.
- Focused accounting smoke passed external partial totals, resume quota admission,
  conservative disk reserve, owned-partial deduplication, and live byte overrides.
  Existing storage smoke still passed. Saved fixture:
  local-state/checkpoints/2026-11-09/torrbaby-storage-accounting-smoke.swift.
- URLSession's unreported temporary allocations and cleanup remain OS-owned; known
  byte totals are not a filesystem audit of its private cache. Runtime checks do
  not execute while iOS suspends the app; admission reserves the expected final
  size beforehand. Actual background download acceptance remains pending.
- Mac and iOS Simulator builds passed. No release/deploy. Further queue review:
  guard late completion from an older task when a replacement task exists for the
  same download ID, without dropping background completions delivered before task
  restoration. Continue the existing cancellation/finalization lifecycle review.

## Follow-up: complete queue HTTP verification and server rollout preparation

- Guarded stale successful completion callbacks when a replacement task for the
  same item already exists; early background callbacks before task restoration
  remain admissible and still pass durable state/revision/size checks.
- Updated the saved queue fixture for current Playback/API signatures and shared
  progress. Real HTTP 4 MiB transfer passed pause/resume, exact payload, local URL,
  shared resume import, no deletion on partial viewing, end-triggered deletion,
  retained pending progress after deletion, and subsequent acknowledgment sync.
- Rechecked Rust download/range and pairing fixtures: both passed. Signed Rust
  bundle build passed. Preparing an additive server rollout with DB backup;
  production before rollout is schema 6, 1 history record and 3 registered clients.

## Deployment: additive Rust APIs on Diamond, 14:31

- Signed server bundle deployed; launchctl restarted only org.torrbaby.daemon.
  Backup: ~/Library/Application Support/Torrbaby/backups/rust-20261109-143105
  (old signed server and schema-6 SQLite). Live schema is now 7.
- Live status/library passed: 180 complete assets, no library error, 3 clients;
  the single playback-history record was byte-for-byte JSON equal to predeploy.
- Authenticated new progress read, subtitle discovery, and 1 KiB 206 media read
  passed. The deployed Rust CLI invoked its adjacent signed Apple translator and
  translated a synthetic English cue to Chinese. This was not a movie caption job.
- Local scratch queue fixture stopped (PID 97351 confirmed terminal).
- Stable installed Mac37 and OTA42 unchanged. WIP app release still pending;
  deployment does not imply native subtitle selection or translation UI exists.

## Follow-up: offline posters and download row labels

- Enqueue now caches a downsampled JPEG poster (max 480 px, 512 KiB) locally;
  episodes share a server/title-keyed file. Root quota/free reserve includes art.
  Removing the final local item prunes its poster; startup removes orphan poster
  files while respecting other items' references. Invalid path keys are rejected.
- Downloads rows show cached artwork offline, concise season/episode labels,
  truncated titles and explicit received/total bytes, with a lazy list container.
- Enqueue captures/rechecks the server around download-info to avoid assigning
  metadata to the wrong server if the connection changes during the request.
- Storage accounting fixture still passed. Poster fixture verifies shared reuse,
  key confinement, reference survival/restart, final-removal cleanup and oversize
  rejection: local-state/checkpoints/2026-11-09/torrbaby-poster-storage-smoke.swift.
- Artwork failures are optional; automatic retries for titles that had no poster
  at enqueue are not implemented yet. Full offline library metadata/subtitle cache
  and visual device acceptance remain pending. No new OTA/install yet.
- Mac and iOS Simulator builds passed for poster/row changes. Production Rust
  remains the verified schema-7 deployment; installed Mac37 and OTA42 unchanged.

## Release: Mac and iOS build54

- Built and signed from a5ee609; installed Mac via staged replacement after normal
  app quit. CUA reopened it, verified Home Connected and Downloads empty state.
  Process check found exactly one Torrbaby app process at the installed path.
- Universal mobile build54 signed/profile verified and published to existing
  Mac-hosted OTA. Six registered devices, expiry2027-11-09. HTTPS IPA digest and
  hub visibility verified. No change to host routing or unrelated Miyu services.
- Main fast-forwarded to the reviewed feature branch. Earlier WIP warnings remain
  historical; this is now an early released Downloads feature with limitations
  listed in docs/release-notes.md. Full persistent goal is NOT complete.

## Follow-up: embedded Matroska text extraction on codex/subtitles

- Current branch `codex/subtitles`, based on main release54. Added pinned
  matroska-demuxer0.8.1 with Zlib OR MIT OR Apache2 license; no transitive crates.
  Existing notice generator includes its license texts successfully.
- New CLI `matroska-subtitles --input FILE` lists tracks; adding `--track NUMBER`
  extracts UTF8/ASS/SSA text as WebVTT. Applies timestamp scale and codec delay;
  retains overlaps; requires explicit durations. ASS/SSA styling/vector drawings
  discarded; bitmap tracks unsupported, no OCR. MP4 extraction pending.
- Synthetic EBML fixture passed non-default timestamp scaling, overlap, text
  normalization, ASS dialogue/comma/line-break handling and vector removal,
  bitmap rejection and missing-duration rejection. Saved scratch script:
  local-state/checkpoints/2026-11-09/torrbaby-matroska-smoke.py.
- Actual Diamond catalog counted62 completedMKV and118MP4. First six smallest MKVs
  had VobSub image subtitles; seventh had ASS. The signed inspection bundle in
  /tmp successfully extracted686 cues /48111UTF8 bytes from the305265910-byte MKV.
  No media files changed; actual dialogue was not printed to transcript or saved.
- Read-only inspection used a separately staged signed app under
  /tmp/torrbaby-subtitle-inspect.* on Diamond. Production server was NOT replaced.
  Script saved as local-state/checkpoints/2026-11-09/inspect-matroska-diamond.sh.
- Rust debug/release signed bundle builds passed. Automatic HTTP jobs are NOT
  wired: third-party frame allocations need subprocess resource limits, caching,
  cancellation and concurrency control. See helpers/README.md. Current local
  server bundle now includes unreleased Matroska code; don't deploy accidentally.
- Installed Mac54/OTA54 and production schema7 APIs unchanged. Persistent full
  objective remains active; native translated captions are not implemented yet.
