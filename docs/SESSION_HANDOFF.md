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

## Follow-up: embedded extraction-to-translation pipeline

- `prepare-subtitles --input movie.mkv --track N [--source en --target zh-Hans]`
  now composes child-process extraction, validation, optional Apple translation,
  and WebVTT output. Timing remains server-owned and unchanged through translation.
- Worker invokes the same current executable, with1 concurrent worker per parent,
  CPU180s, parent wall190s, output16MiB and no core dumps. Error/timeout kills and
  reaps; cancellation kill-on-drop. File revision before/after must match.
- Tested macOS `ulimit -v/-d` rejected the limits with Invalid argument; allocation
  still succeeded. Do NOT describe this as a hard memory sandbox. Automatic HTTP
  preparation/cache/native delivery remain pending.
- Rust build and synthetic MKV full pipeline passed: subprocess normalized output
  equaled direct extraction; real Apple Translation helper on Diamond translated
  synthetic embedded dialogue into Chinese while preserving exact timing lines.
  ASS drawing normalization/bitmap/missing-duration checks still passed.
  Fixture saved as local-state/checkpoints/2026-11-09/torrbaby-matroska-pipeline-smoke.py.
- This pipeline remains unreleased on codex/subtitles. Mac54/OTA54/productionRust
  are unchanged. The per-parent semaphore does not coordinate independent CLI
  invocations; daemon job scheduling must use the shared parent process.
- Full read-only inspection of62 completed MKVs on Diamond found52 with text
  subtitle tracks,8 with image/other-only tracks,2 with no subtitle tracks.
  Track totals:1068 S_TEXT/UTF8,7 S_TEXT/ASS,22 S_VOBSUB. This is track metadata,
  not full extraction/translation proof for all tracks. MP4s not inspected yet.

## 2026-11-09 build 55 and tvOS simulator checkpoint

- Build 55: full-width violet download gauges on a dedicated 22pt line in compact/comfortable torrent rows, narrow mobile cards, details, and local Downloads. Mac and signed iOS release builds passed; mobile OTA published to the existing Mac-only Miyu Install page. Mac installed at ~/Applications/Torrbaby/Torrbaby.app.
- tvOS debug simulator built and launched against Diamond using the existing DEBUG one-use bootstrap profile (no credentials in source/logs). UDID 3F6154D6-6A82-4461-B04A-6A1CAF6C7ED9.
- Xcode 27 uses /Applications/Xcode.app/Contents/Applications/DeviceHub.app; there is no standalone Simulator.app at the old Developer/Applications path. Device Hub visibly showed live library artwork. Genre/sort controls and search layout still need TV polish.
- Physical Apple TV has NOT been paired. Behind slate-seven: recommend temporarily connecting Mac to that router LAN for initial same-network discovery; select Settings > Remotes and Devices > Remote App and Devices on TV, then add/pair in Device Hub and enter displayed code. Cross-subnet deployment remains unverified; do not promise Tailnet routing alone handles discovery/deployment. No network changes made.
- Concurrent Android session owns Android/; leave its files untouched and do not switch branches in this shared worktree. Current branch codex/subtitles.
- Subtitle job manager in server/subtitle_jobs.rs with server/api.rs/main.rs integration is still local work pending review/docs/commit/deployment. cargo build --locked and local-state/checkpoints/2026-11-09/torrbaby-subtitle-jobs-smoke.py passed: authenticated prepare/status/VTT, job dedup, restart cache reuse, source revision invalidation, no playback side effects. Filesystem-backed max 4 queued jobs, one worker, 512MiB/256 entry cache bounds; translated/original WebVTT. Not deployed, not yet exposed in Apple subtitle UI.

## tvOS library-first visual pass (after build 55)

- Apple/Torrbaby/TV.swift now has native Home/Movies/TV Shows/Torrents/Search/Settings tabs, a large backdrop hero, blurred backdrop color wash, 240pt poster shelves, and native card focus behavior. Search keyboard is confined to Search. No local downloads on tvOS by user requirement. Torrents secondary, read-only status grouped Downloading/Seeding/Complete with wide violet gauges.
- Debug tvOS simulator build passed and live Diamond library verified visually in Device Hub. Physical TV and full remote navigation/playback acceptance remain pending. Detail screen still shared with mobile and needs a TV-specific visual pass. Hero still has native TV safe-area/card treatment; user prefers as much full bleed as possible. Metadata currently requests w1280 backdrops; higher-resolution cache support remains follow-up.
- Unsigned simulator build did not retain Keychain token across reinstall/relaunch; repeat DEBUG one-use bootstrap copy into app data tmp and SIMCTL_CHILD_TORRBABY_BOOTSTRAP_FILE for launch. Do not put token in command arguments/logs/source.

## tvOS full-bleed/detail follow-up

- TVLibraryView ignores horizontal safe-area for genuine edge-to-edge hero artwork; hero uses a restrained mint underline for remote focus rather than scaling/rounding the whole image card.
- TVMediaDetail replaces the shared mobile detail on tvOS: full-bleed backdrop, large title/rating/genre, Play/Resume, season controls, episode rows and extras, backdrop wash. Uses existing native PlayerScreen and server playback/progress APIs; no downloads.
- tvOS simulator build passed. Live UI verification with Device Hub remote Down/Select/Back confirmed hero focus, opening movie detail, and returning to Home. No playback started during this visual check; physical Apple TV acceptance and episode/season navigation still pending.

## tvOS episode browsing checkpoint

- Episode list defaults to the in-progress season, then an unwatched regular season, rather than always the first/specials season. Unnumbered episodes remain available separately when numbered seasons exist.
- Episode rows fetch stills via existing authenticated asset/episode artwork endpoint; incomplete items show Downloading. Playback title includes episode name.
- tvOS simulator build passed. Device Hub remote navigation verified TV Shows tab → Paradise detail → season control → Episode 1 focus; actual episode stills loaded for Episodes 1/2. Did not start playback or alter watch history. Multi-season switching and resume selection still require runtime acceptance.

## Build 56 torrent sorting

- Shared TorrentSort in Models.swift implements per-list defaults/options in docs/interface.md for macOS/iOS/iPadOS/tvOS. Per-list AppStorage persistence; deterministic title/ID ties; dates newest first, size largest first. No Android edits: concurrent port should follow the documented contract.
- Mac and signed iOS build56 passed; tvOS debug simulator build passed/reinstalled. Mac installed/running; mobile OTA56 published on Mac-only Miyu Install. IPA SHA256 800be1caf6c4a00b6c0b34c003d0fc02d5a9ce296dc215d90a48682e6a87edb9.
- Mac live AX verified Downloading progress order 99/66/33/11/4 percent and Seeding ratio order .63/.20/.05 etc. Other ordering choices compiled but not all manually exercised yet.

## Subtitle API checkpoint / Device Hub keyboard

- Queued subtitle jobs and embedded track inspection now implemented/documented in helpers/README.md. cargo build --locked passed; temporary subtitle-jobs smoke passed including embedded discovery, authorization, deduplication, VTT timing, restart reuse, revision invalidation. Not deployed to Diamond; player subtitle selection still pending.
- Device Hub troubleshooting: user was on simulated TV home screen, Capture Keyboard off. Opened Torrbaby using Right then Select on bottom remote; app loaded live library. Enabled toolbar Capture Keyboard; verified Down, Return (open detail), Escape (Back). Hold Escape releases keyboard capture per visible Device Hub hint. Left running on library with capture enabled. TV home icon remains placeholder and needs proper tvOS assets.

## Next: mobile hardware-keyboard audit (user priority)

User reports iPad left/right correctly skip 15 seconds but Space does not play/pause. Finish current live subtitle verification, then fix Space and audit essential mobile playback/navigation shortcuts with Magic Keyboard as the normal iPad setup. Verify actual key dispatch, avoid intercepting typing in text fields; keep native Apple player controls. Audit Escape/back/dismiss, arrows, Space, Return/select and standard search/settings/navigation where appropriate. No custom player chrome requested.

## Live subtitle deployment (2026-11-09 15:24)

- Deployed signed Rust source through 68b60da to Diamond; backup: ~/Library/Application Support/Torrbaby/backups/rust-20261109-152415 (bundle + SQLite). No schema change. Library remained 59 titles / 186 complete assets at pre-deployment check.
- Authenticated live inspection correctly reported unsupported VOBSUB and supported ASS/UTF8 tracks. English ASS original prepared successfully from 305265910-byte completed asset: 686 cues, 48111 WebVTT bytes. No dialogue logged.
- Chinese job still running at this checkpoint; helper PID54153 was confirmed active. Job IDs in ignored local-state/checkpoints/2026-11-09/live-subtitle-job.json, original timing-only baseline in live-subtitle-timings.json. Poll these exact job IDs; do not resubmit/restart merely because translation takes time.
- Player integration remains pending: native AVPlayer WebVTT selection requires appropriate delivery (e.g. HLS subtitle rendition); producing WebVTT alone is not playback integration.
- Keyboard audit finding: MobilePlayerPresentation.PlayerController and KeyboardPlayerController each override keyCommands with only left/right/Escape, dropping inherited AVKit commands and omitting Space. Consolidate shared controller, retain non-overridden native commands, add explicit Space play/pause using timeControlStatus (including waiting). Broader iOS app Command shortcuts currently largely macOS-only. Native Return/Tab/focus should remain system-owned.

- Live Chinese subtitle job completed successfully: 51156-byte WebVTT, 686 cue timings exactly equal to original, Chinese text present. This verifies preparation/translation on Diamond, not semantic translation quality or AVPlayer subtitle selection.

## Mobile build 57 / subtitle selector priority

- Build57 iOS release + iOS simulator compiled. Published to Mac Miyu Install; IPA SHA256 e0abe0e2f2197dacb8511616953b071983648a1fe27a6e203c4c2436faac21af. Space play/pause explicitly registered in shared KeyboardPlayerController, using timeControlStatus; mobile wrapper now subclasses it. Native non-overridden keyCommands preserved and discovery labels added. Physical Magic Keyboard verification pending; full audit in docs/hardware-keyboard.md.
- User cannot see subtitle selector on tvOS. Explained translated WebVTT output is NOT yet native-player delivery. Make native selectable original/translated subtitle renditions next playback priority. Do not claim completed translation jobs imply visible captions. Native player may show original tracks only when detected in delivered media.

## Native subtitle delivery experiment

- helpers/MediaSegmenter.swift development helper emits HLS fMP4 via AVAssetReader/Writer passthrough; no external codecs/packages. Not shipped/deployed/integrated with Rust yet. Apple WWDC20 10011 and AVAssetWriter outputFileTypeProfile docs support this approach.
- Generated own 24s H.264 fixture; output 4 segments. Temporary loopback HTTP18897 with HLS master and synthetic English/Chinese captions. Swift AVPlayer probe reported playable, readyToPlay, legible group en/zh-Hans and successful native option selection. Does not prove rendering/timing or actual movie/container/audio/HDR support.
- Scripts/logs ignored under local-state/checkpoints/2026-11-09/{make-hls-fixture.swift,inspect-hls-subtitles.swift,hls-*}; output /tmp/torrbaby-hls-packaged. Need external worker bounds, revision-keyed cache/quota, session-authenticated playlists/segments and real library/player integration. No production Python added.
- User feedback: tvOS library bare-bones; mobile Home requires too much horizontal scrolling. Proposed keep Home to Continue Watching/Recently Added/few highlights, See All to vertical poster grids, Movies/TV Shows grid-first with genre/sort. Needs implementation; preserve imagery-first/full-bleed TV direction.

## Build 59: fuller library and visible release number

- tvOS Home now includes Continue Watching, Recently Added, Recently Watched, Movies, TV Shows and genre shelves. Movies/TV tabs use vertical poster grids. See All opens a full collection grid; shelf previews stop at 10 TV / 8 mobile and Mac posters to reduce horizontal scrolling. Genre imagery verified in TV simulator before build59; complete See All navigation acceptance remains pending.
- Settings begins with installed bundle version/build on Mac, iPhone, iPad and TV; no hard-coded release label. TV connection screen also shows it. Mac live Cmd-comma verified Version 0.2.0 · Build 59 at top. Android port must follow same requirement (concurrent Android session owns Android/; untouched).
- Release59 Mac/iOS builds and tvOS simulator build passed. Signed Mac installed/relaunched; TV simulator updated/relaunched. Mac-only Miyu Install HTTPS current.json verified build59. IPA SHA256 f29911ab7247e1138a2c8675cba6f43066d4cd7dc262e83989942db2959039fb. Physical mobile update/visual acceptance pending.
- Native original/translated subtitle delivery remains the next playback priority; HLS experiment above is not deployed. Persistent full project goal remains incomplete.

## Supervised HLS packaging checkpoint

- Added Rust package-media CLI supervising AVFoundation helper: one worker per process, bounded stdout, CPU600/wall610, source revision check, new output only, failure cleanup. Helper configurable 1MiB–32GiB output budget and 1GiB free reserve per segment. Build script now bundles/signs helper on next server build; not deployed or connected to playback yet.
- cargo build --locked and Swift helper compile passed. Generated fixture packaged; existing output preserved on rejection; invalid media failed and cleaned output. No formal suite added. Cancellation kills worker but interrupted output recovery remains a prerequisite for persistent cache integration.
- Latest user priority: prominent Download beside Play/Resume; replace with progress while active, then Downloaded/size/delete (detail confirmation). Downloads page compact sizes and swipe-delete without extra confirmation. Work next in Library/Downloads UI; preserve Android session files.

## Build60 download presentation / next subtitle preferences

- Prominent Download beside Play/Resume on media detail; selected asset replaces it with active progress, byte count and pause/resume, then Downloaded/size/delete with confirmation. Existing toolbar season/multi-file menu remains available.
- Downloads now compact native List rows, 40x60 thumbnails, sizes, active progress/status; storage used/limit at top; mobile trailing full swipe deletes without extra prompt. Mac has direct trash. Existing keep-download toggle retained.
- Mac/iOS release60 compiled. Mac installed/relaunched; live AX verified Resume+Download on Matrix and storage total on empty Downloads page. Active/ready/swipe transitions not physically exercised yet. iOS OTA60 published to Mac-only Miyu Install.
- Latest explicit user requirement: shared server subtitle defaults (enabled, language, style), per-title overrides shared across devices, inherited default until overridden, Use defaults reset. Mobile Chinese still absent because prepared VTT isn't delivered via native player. Next priority is native subtitle delivery plus persistent preference API/client application. Do not claim translation generation equals playable captions.

## Shared subtitle preference backend

- Schema8 adds subtitle_preferences, shared household defaults plus whole-title overrides (series applies to episodes). API contract documented in docs/interface.md. Strict validation; reset resumes inheritance; reads return defaults/effective/overridden.
- Rust build passed. One focused SQLite check passed default inheritance, override persistence across reopen, invalid value rejection, unknown title rejection, reset. Not deployed; Diamond remains schema7. Apple UI and native player application remain pending. Prepare signed server deployment with database backup before schema8 rollout.
- Download build60 published and Mac installed; latest commit bf2d3e1. User still waiting on Chinese caption selection and persistent settings: prioritize next.

## Build61 / live subtitle preferences

- Signed Rust source70e6d92 deployed Diamond, schema8 verified; backup ~/Library/Application Support/Torrbaby/backups/rust-20261109-155503. Live authenticated reads: defaults off/original and unmodified title inheritance equal; library63titles. Packaging helper now bundled but not connected to stream routes.
- Shared SubtitlePreferencesView available Settings→Default subtitles and title→Subtitles on Mac/mobile/TV. On/off, Original/English/Chinese, font, size, text/background opacity, per-title Use defaults. Saves full preference to shared server. Mac live form loaded and unchanged default Save succeeded. No user language choice was made during verification.
- PlaybackModel creates SubtitlePlaybackController when native item ready; loads effective settings, applies AVTextStyleRule and legible selection, observes native selection changes and saves whole-title override. Cross-device refresh occurs at playback start. Missing requested language selects none; translations still NOT in playback stream. Offline preference caching not yet implemented; failed settings fetch preserves native defaults. Native menu persistence/rendered style acceptance still pending; builds only plus form/API validation.
- Apple61 Mac/iOS/tvOS simulator builds passed. Mac installed and running, TV simulator installed/launched. OTA61 on Mac Miyu Install, IPA dc7fe4dba49beb8cdd0d7878ff6ab716a8a93cbdb3731c8e04bf95212aaf58ab. Next: integrate authenticated HLS/subtitle delivery and real media compatibility, not merely more preferences UI.

## Real-media HLS packaging compatibility checkpoint

- Real Diamond MP4 H.264+AC3+tx3g initially failed AVAssetWriter start (-11875). Isolated video-only and audio-only both produced segments: issue was combined writer, not AC3 support. Helper now creates separate track-N renditions and audio-linked master, preserves compressed video/audio, caps total bytes across tracks. Rust validator updated for this layout. MKV correctly fails AVFoundation load -11828 (unsupported container).
- Swift helper and cargo build passed; Rust package-media validated generated fixture with new layout. Real MobLand episode (1.33GB) packaged on Diamond with diagnostic helper to /tmp. Loopback-only temporary HTTP18897 + SSH tunnel: native AVPlayer playable=true, ready, 1 audio option, duration3288.895, advanced playback. Muted probe, no rendered A/V sync assessment. No original media or watch history changed.
- Diagnostic remote HTTP PID56441 stopped explicitly after SSH termination left it running. Tunnel ended. Generated remote package (~1.2GB) and diagnostic segment directories removed. No production Python service added; production helper still previous layout until next server build/deploy.
- Ignored evidence scripts inspect-media-formats.swift, inspect-real-hls.swift, video-only-packager.swift/audio-only-packager.swift under local-state/checkpoints/2026-11-09. Next necessary work: authenticated HLS job/cache/session routes and subtitle renditions; MP4 tx3g extraction; MKV remux. Existing apps61 and server schema8 remain live unchanged this turn.

## User profiles backlog (explicit latest request)

- User wants individual users with separate history/watch positions after other planned work. Plan in docs/user-profiles.md. Migrate current shared state into default profile; device identity is not user identity. Offline events must capture profile at viewing time and retain it through deletion/reconnection. No implementation yet; do not silently treat current household history as per-user.

## Build63 / offline airplane handoff

- Removed duplicate media-detail toolbar download menu; prominent Download by Play/Resume remains. Mac live AX confirms one Download and no toolbar download control. Existing DownloadTitleMenu is now unused; restore season/multi-file batch selection alongside episode controls later (not as duplicate toolbar icon).
- New downloads seed local progress from catalog. Each successful server refresh imports newer positions into local downloads, including never-opened episodes; sync uploads happen before library/history reads so reconnection displays reconciled state promptly. Existing timestamp checks protect newer offline progress.
- Focused generated three-episode storage scenario passed: two completed episodes deleted (one auto, one explicit), third at47/100, app restart, independent pending events retained, acknowledgments persist, stale imports cannot rewind. Rust HTTP smoke passed three-episode watched/resume reconciliation, idempotent retry, stale rejection and active playback deferral. Physical airplane-mode acceptance not yet exercised.
- Mac/iOS63 built. Mac installed/relaunched, mobile published on Mac Miyu Install. IPA ab851490b97577660ab7a215bd8a26ea2b4b68c480f3effbd3219e259c5aaa13. Build62 was superseded before distribution. TV remains61 (no download feature).
