## Apple 103 / Android 32 — Cast pages and detail actions — 2026-11-10

Cast opens a separate scrolling page from the action row after Manage Files.
Both clients cap the list at 150 actor/role entries and show a truncation notice.
Android uses recycled ListView rows; Apple uses a lazy List. Season buttons replace
the Apple dropdown. Episode rows use right-aligned download/cancel/delete and
watched icons; TVs omit downloads. Server file management stays at title level.
Show playback buttons reserve space for the episode and minutes remaining and
use the accent color.

Mac tabs are explicit: Cmd-T or the plus after Back/Forward. Automatic window
tabbing is disabled; menu-bar Open Torrbaby reuses an existing library window.

Validation: Android 10,000-entry cast stress check passed (150-row cap, recycled
scrolling, selected season retained on Back). Signed iOS/macOS and tvOS simulator
builds passed. Mac UI check verified explicit tabs, navigation within a tab, and
Homeland cast scrolling to the cutoff. JMGO confirmed build 32; TCL received the
installation request but approval timed out. iOS build 103 is on the install page.

## iPhone/iPad build 101 — Next Episode overlay — 2026-11-10

During the last 10% of an episode, touching the player reveals a Next Episode
button for six seconds if the next episode is available. Selecting it records
the current episode as watched, including when paused, and uses the existing
end-of-episode advance path. Apple playback controls remain intact. The button
is hidden in Picture in Picture and while preparing the next episode.

iOS has no public native-controls visibility callback: the button has its own
interaction timer rather than inspecting AVKit's private view hierarchy.
Validation: signed iOS release build and diff whitespace check passed. Actual
overlay placement and interaction alongside native controls still need an iPad check.

## Profiles release — Apple 93 / Android 25 — 2026-11-10

The server now owns persistent user profiles. Existing history and linked devices
were migrated to Jon. New devices choose or create a user; TV users can switch,
and the selection survives restarts and updates. Watch history, watched markers,
Continue Watching, and Hero are personal; subtitle defaults remain device-specific.

Library detail menus now offer Manage files with a package-wide scope preview.
Permanent deletion removes owned files and associated data. Move to Junk hides
the whole package for everyone while preserving seeding, then removes it after
Transmission reports the seed policy complete. Pending cleanup and errors remain
visible in Seeding, even after the torrent disappears.

Metadata and artwork are prepared while packages download. The Library still
waits for the entire package to finish and its files to be verified. Episode-end
advancement now handles streamed and downloaded episodes; paused scrubbing alone
does not mark a title watched.

Deployment: Diamond schema 10 is live; Android 0.3.0 build 25 is installed on JMGO;
iPhone/iPad build 93 is available at the existing install page; Mac build 93 is in
`~/Applications/Torrbaby.app`, with a ZIP in `~/Downloads/Torrbaby/`.

Remaining limitations: TCL was unreachable during delivery. A tvOS device release
needs provisioning for `org.torrbaby.tv`; simulator builds pass. iPhone/iPad early
Next episode UI awaits a choice because native AVKit exposes no public controls
customization/visibility hook. Automatic advancement works. macOS uses the native
action menu; Android and tvOS expose the action with playback controls.

## Mobile build 83 — 2026-11-09

Republished the stable Apple client from commit `de1ebb3` as iPhone/iPad build 83.
Subtitles pins the running job above the queue, with title, target language,
cue progress, and two-second status refresh. In-progress torrent-search work is
not included in this release.

Published to Miyu Install on the Mac. Verified the HTTPS install page, manifest,
and downloaded IPA checksum. Signing covers six registered devices and expires
2027-11-09. Confirm build 83 at the top of Settings after installing.

# Build 54 — early offline downloads

Mac build installed; iPhone/iPad build available on the Mac-hosted Miyu Install
page. Existing QR linking, full-screen mobile playback, PiP and compact torrent
rows remain included.

Open a movie/show and use the download menu. Local copies appear in **Downloads**.
Its sliders button controls the storage limit, minimum free space, Wi-Fi-only
transfers and deletion after playback. **Keep downloaded** protects a copy from
automatic deletion. Removing a local download never removes Diamond's file or
stops its torrent.

Library Play prefers a downloaded copy. Watch position syncs with Diamond when
connected, and remains local when offline. Reaching an episode's actual end can
delete the local copy and advance to the next available episode in that season.

Still being developed: cross-season ordering, full offline library browsing,
translated subtitle delivery, compatible preparation for unsupported containers,
and physical-device background-transfer acceptance. Downloads currently copy the
original file; native player support for that file still matters. Server schema7
has a predeployment schema6/database+bundle backup on Diamond.

### 2026-11-10 — Android 31 / Apple 100: browsing, seasons, and cast

Android uses recycled responsive poster cards, incremental catalog rebinding, whole-row remote navigation, retained Back position, prioritized/prefetched artwork, and a 52-byte server revision check instead of repeatedly fetching an unchanged catalog. JMGO verification retained focus during progress updates, kept all 16 tested row movements fully visible, allocated one additional recycled card, and restored the exact Back selection in 77 ms. Cached startup remained about 0.6 seconds; no cold-start speedup is claimed.

All clients show one selected season at a time. Mobile/macOS now label the main show download action Download Season N and queue that season, skipping existing downloads/queued items. Android TV and tvOS retain their existing season-specific guides and do not download video.

Detail pages offer a default-collapsed Cast section only when credits exist. Movie rows display actor/character; TV rows display the supplied role and episode count. No custom count aggregation or per-episode credit crawl. TMDB credits are cached with catalog metadata. Live backfill completed for all 72 titles, including The Matrix and Homeland. Android expansion and remote row focus were visually verified; Apple iOS/macOS/tvOS-simulator builds pass. A focused server check verifies role/count preservation.

Delivery: Apple 100 published to Miyu Install; signed macOS 100 installed in ~/Applications/Torrbaby.app (reopen to use). Android 31/0.3.5 installed and package version verified on JMGO; APK on OTA and ~/Downloads/Torrbaby. TCL still requires native installer confirmation. Server backup: cast-details-20261110-095032. Server restart was explicitly approved during playback; queued subtitle work was preserved and re-admitted.
