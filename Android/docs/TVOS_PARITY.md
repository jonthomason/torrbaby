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

Shared TVs will support multiple viewers with an obvious profile switcher; mobile normally remembers one viewer. Android integration is tracked in [PROFILES.md](PROFILES.md), pending the shared server contract and tvOS picker. Current Android viewing state is still household-wide.

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
