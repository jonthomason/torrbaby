# Torrbaby handoff — 2026-11-09

Read this before continuing. The user requested a stopping point before clearing
context. The full persistent goal is still active and NOT complete. Do not infer
completion from the deployed Rust cutover or the current client builds.

## Checkpoint and release boundaries

- Repository: `~/src/torrbaby`, public MIT, `github.com/jonthomason/torrbaby`.
- Stable `main` / `origin/main`: **1527d03**, mobile build **42**.
- Current work branch: **codex/offline-downloads**. Its queue/player work is a
  checkpoint, NOT a released feature. Review its changes before merging to main.
- Latest mobile release was built from the clean detached worktree
  `/tmp/torrbaby-qr-landscape-release` at stable main, not from the dirty download
  checkout. It contains QR landscape and native PiP fixes plus responsive torrent
  rows, but does NOT include DownloadQueue or the Downloads page.
- Installed Mac app is **build 37**, verified in its Info.plist. The more recent
  stable changes are iOS-specific. Do not accidentally install the unfinished
  download build when the user asks for the current stable Mac app.
- Current branch's macOS and iOS Simulator builds both pass. Logs are under
  `local-state/checkpoints/2026-11-09/`. These builds have not been installed or
  published. Formal test-suite expansion is not requested.
- Stale local Rust preview PID 42922 and its SSH tunnels (55759 and 59363) were
  verified and stopped at checkpoint. Production Diamond was not stopped.
- tvOS 27.2 simulator runtime download completed. `xcrun simctl list runtimes`
  now lists it. No need to download it again. Apple TV physical pairing remains
  unresolved; continue simulator development until the user pairs it.

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
  `~/Library/Application Support/Torrbaby/`. SQLite schema version **6**.
- Signed bundle ID `org.torrbaby.server`, Developer ID team **23PTSAZN2G**.
  User granted file access; direct range reads were verified from media volumes.
- Python launch agent was replaced, not run alongside Rust. No Rust Plex calls.
  Legacy cached Plex-relative artwork can be read from disk, never fetched.
- Initial cutover backup: `backups/pre-rust-20261109-125138`; latest server update
  backup `backups/rust-20261109-133150` includes previous signed bundle and SQLite.
  Rolling back schema 6 to an older server requires the corresponding DB backup.
- Latest deployed Rust includes authenticated original-file download descriptors
  and ranged transfer (a4a7c89). Successful real-media descriptor + HEAD + 1 KiB
  conditional 206 range verified on Diamond after deployment.
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
