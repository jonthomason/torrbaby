# tvOS / Android review

Reviewed against the current tvOS source on 2026-11-09, including shared subtitle preferences. The JSON baseline records exact source hashes; it detects subsequent changes, not visual equivalence.

| Area | Android implementation |
| --- | --- |
| Home | Full-width artwork hero; Continue watching, Recently added/watched, Movies, TV Shows, genres; ten titles then See All |
| Catalog | Alphabetical poster grids, search, extra genre/sort controls |
| Details | Full-width backdrop, bold 28sp title, 19sp metadata, mint Play/Resume, restrained synopsis, seasons, thumbnails, extras |
| Typography | Native Android sans with bold display titles, medium labels, tighter tracking; tvOS proportions translated from 1920 pixels to 960dp |
| Artwork/focus | Rounded posters without caption panels, subtle poster lift, translucent episode rows, light focused tabs, mint hero underline |
| Playback | Native Media3 controls, remote audio/subtitle menus, resume, seek, next episode |
| Speeds | 0.5, 0.75, 1, 1.25, 1.5, 1.75, 2×; remembered on each client |
| Subtitles | Shared defaults and title overrides: on/off, original/English/Chinese, font, size and opacity; native rendering |
| Torrents | Live status and per-category sort plus remote server controls, files, magnet submission and seeding target; no downloads on TV |
| Settings | Installed version/build, connection controls, shared subtitle defaults |

Android uses native fonts and native player controls, rather than copying Apple system assets. Older Android versions use a flat navy fallback instead of blurred ambient artwork. Library refresh runs every ten seconds while browsing, retaining focus and skipping text entry/dialogs; torrent status updates every five seconds. Android subtitle preferences fall back to native defaults on older servers. Server implementation remains owned by the other session; Android consumes its subtitle preparation/translation API.

When the build reports a changed reference, review the changed Apple files, update Android where applicable, inspect emulator screenshots, then regenerate the baseline. Do not overwrite Apple/server changes or treat a refreshed hash as a substitute for review.

## Upcoming profiles

Shared TVs will support multiple viewers with an obvious profile switcher; mobile normally remembers one viewer. Android integration is tracked in [PROFILES.md](PROFILES.md), pending the shared server contract and tvOS picker. Local Android profile integration now uses the server profile API; see PROFILES.md for release and validation status.

## Playback preparation review — f7f87d4

The latest tvOS update prepares translated captions and native HLS on the server, with progress and Cancel. Android now exposes Cancel while opening video; delayed completion cannot reopen a finished activity. Android plays direct media with native embedded/sidecar subtitles and now attaches prepared/translated WebVTT from the same server jobs. This provides the subtitle behavior without requiring Apple's HLS packaging step.

## Automatic connection

Android 0.1.1 follows the in-progress TVConnectionView: native DNS-SD `_torrbaby._tcp.` discovery, POST `/v1/lan/register` with name/platform, encrypted remembered device credentials, no pairing UI. Manual fallback asks only for a server address. Native discovery/port resolution passed an emulator instrumentation smoke test. Diamond LAN service was deployed by the Apple/server session and verified listening/advertising on 2026-11-09.

## Subtitle UI simplification (Android build8 / Apple75)

Title details contain language information and generated-translation indicators only.
Appearance and default language are configured in Settings. The player has one
subtitle selector: Off, English variants, Chinese variants, then other languages.
Forced/SDH variants retain distinguishing labels. Selection saves the shared title
override. Apple retains its native menu; identical ordering is not guaranteed.

### Android performance pass (2026-11-10, build 30)

Active scope: reusable poster views, incremental catalog updates, whole-row D-pad navigation with retained Back position, visible-first artwork and next-row prefetch, responsive consistent poster layout, and lightweight catalog revision checks.

Implemented in the working tree: PosterCard recycling; PosterGrid row selection; screen-local catalog bindings for grids, shelves, and Home hero; image priority queue and obsolete-screen cancellation; removal of full-screen GPU blur; durable server catalog revision endpoint and Android polling. Server revision check verified live: 52-byte response versus 942,697-byte catalog, same revision. Focused revision invalidation test passes. JMGO cached initial frame: build 26 585 ms; preliminary build 28 591 ms (no startup improvement claimed). Physical navigation verification exposed a firmware-specific row positioning issue and a confirm-key relayout issue; corrections are being checked before final release. Evidence is under ignored `local-state/android-performance/`.

Queued after the performance pass, per user:

- Remove All Seasons from every client's episode selector, including Android TV and tvOS. Show one season at a time, initially the season containing the next episode to watch.
- On download-capable clients only, the detail action should say Download Season N and enqueue that selected season, skipping already-downloaded/queued episodes. TVs remain streaming-only.
- Add a default-collapsed Cast action on all detail pages when cast metadata exists. Expanded content is a two-column Actor / Character list; blank character cells are acceptable. Cache TMDB movie/TV credits with other metadata. Omit the action if no cast data is available. Use the supplied aggregate roles and episode counts directly (Actor / Character / Episodes for TV); no custom aggregation or episode-by-episode credit fetching.

Performance completion evidence: Android release/debug builds pass. Emulator navigation check passed (16 row movements, 6 additional recycled views, Back 19 ms). The final JMGO check passed incremental progress rebinding without focus loss, all 16 row movements fully visible, one additional recycled view, exact Back title/position retained in 77 ms. Catalog revision invalidation test passes; live revision endpoint verified alongside matching catalog revision. Build 30/0.3.4 installed on JMGO and published to OTA/Downloads. TCL install requested, confirmation tracked separately. Cached startup remained approximately 0.6 seconds in the measured before/after runs; no claimed cold-start speedup. Shell-injected JMGO arrows were ignored by its firmware, so scrolling measurements use actual Activity key dispatch through the debug instrumentation runner.

The queued season and cast changes above are now implemented in Apple 100 and Android 31. Credits backfill completed for all 72 live titles. Android Cast disclosure and per-row remote focus were checked on Homeland, showing the supplied Claire Danes / Carrie Mathison / 96 entry. Movie credits omit the episodes column. iOS, macOS and tvOS-simulator builds pass; Android release passes and version 31 is installed on JMGO. TCL build 31 is requested and awaiting native consent. The cast role/count projection test passes. Details in docs/release-notes.md.

2026-11-10 TV shelf pass (Android43 / tvOS125): removed See All navigation and ten-title shelf caps. Native tvOS LazyHStack browses full categories; Android appends twelve posters ahead of D-pad/scroll movement, with no fixed catalog limit. Tighter horizontal gaps and full-width adaptive grids; sparse genre rows omitted. Continue Watching remains accurate to the active profile. Isolated EN/ZH Android fixture traversed all40 titles and back with real D-pad events.
tvOS additionally reserves menu space above top-aligned heroes and publishes a native still-image Top Shelf carousel (current/recently watched title plus four newest completed titles with backdrops). Shared Keychain snapshot has no credentials; extension uses public artwork URLs and opens app detail/play URLs. tvOS owns transitions/rotation timing, with no custom twenty-second timer. Snapshot refreshes with the app library/profile/language. Home Screen real-art rendering verified in simulator; Top Shelf requires the app in the top row.
