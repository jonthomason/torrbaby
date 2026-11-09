# Torrbaby

A small, private Transmission companion. It sorts completed torrents into Movies
or TV, preserves seeding by moving through Transmission, requests Plex scans,
and exposes an authenticated HTTP API for native apps and a CLI.

MIT licensed. Python 3.9+ standard library only. No third-party packages, bundled
Transmission source, metadata API keys, or hardcoded server addresses.

## Active development goal

The server is now being ported to **Rust**, replacing the Python runtime before
further server feature development. The deployed Python daemon remains in place
only until the Rust replacement is ready. Preserve the HTTP API, configuration,
SQLite records, client identities, watch history, and Transmission seeding state.
The migration must not require clients to reconnect or reset their libraries.

The server remains headless: an authenticated API and CLI, with browsing and
administration in the native macOS, iOS/iPadOS, and tvOS apps. No web UI is part
of this goal. A Diamond-hosted web UI is deferred until there is a concrete need.

Port order: configuration and SQLite persistence; Transmission RPC and actions;
authenticated HTTP and original-file range streaming; shared playback state;
filesystem catalog, TMDB metadata and artwork; safe naming and automation;
signed macOS installation and launchd cutover. Validate on a database copy and
an alternate port before replacing the live service. Back up the database and
retain the old executable/configuration for rollback. Do not run two organizers
against the live library at once.

The Rust binary currently provides `check-access --media-root PATH` to test
directory enumeration and sample file reads. It is not yet the replacement
daemon. Dependencies and build requirements will be documented as the port lands.

Turn the shared Diamond daemon into a **standalone media server with no Plex
dependency**. Scan and persist metadata for every movie and TV episode, apply
correct names for high-confidence matches while preserving Transmission seeding,
and provide polished native Mac, iOS/iPadOS, and tvOS apps with artwork, search,
genre/date browsing, and TV season/episode navigation.

Plex may help migrate existing matches during development, but is not required
for the completed system. Completion requires cold startup, filesystem discovery,
metadata matching and local artwork storage, naming/organization, direct playback,
and shared watch progress to work with Plex stopped. Previously cached library
browsing must also work without contacting metadata providers. Native Apple player
UI remains the playback interface. Public remote-access infrastructure is outside
this goal.

This describes the target, not a claim that the current implementation is already
independent of Plex.

## Behavior

- Reconcile every 30 seconds, including after downtime. Only torrents directly
  assigned to the configured Downloads root are eligible; existing library
  torrents and other destinations are left alone.
- Wait for metadata and **all torrent files** to finish, with no torrent error or
  active verification. Selectively downloaded torrents with missing unwanted
  files are deliberately left for manual handling in this version.
- Detect TV from torrent names or file paths containing `S01`, `S01E02`,
  `S01.E02`, `Season 1`, or `1x02`. Other video torrents are Movies. Archives,
  music and torrents with no recognized video extension are left alone.
- Move the entire torrent with `torrent_set_location(move=true)`. Never perform
  an OS-level media move or copy. Subtitles, artwork and other payload files
  travel with the torrent. Preserve the existing paused/seeding state.
- Refuse existing destination roots, missing volumes, symlink paths and files
  shared with another torrent. Missing destinations are never created by the
  sorter. Torrent names are untrusted data, never shell commands.
- Persist the operation before requesting the move. Verify Transmission's new
  location, completion, target file sizes and source disappearance before
  marking it done. Ambiguous RPC timeouts are reconciled, not blindly repeated.
- Queue scans for source and destination Plex libraries. Failed scan requests
  survive restarts. A successful scan request means Plex accepted it; Plex
  indexing is asynchronous and can still reject unsupported media.
- Pausing automation suspends reconciliation and automatic scan retries. Manual
  torrent actions and explicit Plex scans remain available.
- No automatic deletion and no automatic metadata renaming.

## Install on macOS

Transmission 4.1+ must be running with local RPC enabled. Plex must already have
libraries whose roots match Downloads, Movies and TV. By default Plex's token is
read from the current user's Plex preferences without being printed. Alternatively
set `plex_token_file` to a private text file. Transmission Basic credentials may
be supplied as `user:password` in `transmission_auth_file`.

Copy this repository to the server and run:

```sh
python3 scripts/install.py --media-root /path/to/Media --listen-host 127.0.0.1
```

For remote native apps, use the server's **Tailscale IP** as `--listen-host`.
This binds to that address, not to all interfaces. HTTP is carried inside
Tailscale's encrypted connection. Don't expose the HTTP listener to the public
internet; use a TLS reverse proxy if deploying outside a private encrypted network.

The installer creates a random API token with mode 0600, preserves existing
configuration/state on upgrades, and restarts only Torrbaby's LaunchAgent:

- `~/Library/Application Support/Torrbaby/app/`: application code
- `~/Library/Application Support/Torrbaby/config.json`: server configuration
- `~/Library/Application Support/Torrbaby/api.token`: private bearer token
- `~/Library/Application Support/Torrbaby/state.sqlite3`: settings, jobs, recent events, devices and playback history. An existing `state.json` is imported once and retained as a migration backup
- `~/Library/Application Support/Torrbaby/torrbaby.log`: rotating operation log
- `~/Library/LaunchAgents/org.torrbaby.daemon.plist`: per-user service

This is a LaunchAgent, so it starts in the user's login session. Transmission and
Plex are independent services. For Linux or a different supervisor, copy
`config.example.json`, create a private random token file, edit paths and run:

```sh
python3 torrbaby.py serve --config /path/to/config.json
```

Only one process may own a state file. Stop Torrbaby before copying its SQLite database, or use SQLite’s online backup API so WAL transactions are included. Back up configuration and state when
upgrading. To stop/uninstall the macOS service without deleting media:

```sh
launchctl bootout "gui/$(id -u)/org.torrbaby.daemon"
```

Remove its LaunchAgent plist to prevent it returning at the next login. Keep or
remove Torrbaby's application directory separately. No uninstall operation
changes Transmission or Plex.

## CLI and API

All endpoints require `Authorization: Bearer <token>`, including status. Token
files belong outside the repository; import the token into Keychain in a future
native client. No CORS access is enabled. API errors use JSON `{"error":"..."}`
with HTTP 400 for invalid requests, 401/403 for access failures, 404 for unknown
routes and 502 for backend failures. Responses and request field names use
snake_case. Requests are limited to 64 KiB. Native clients should poll status and
torrents, with backoff when unreachable.

The CLI invokes the same API as an app; it does not bypass the service:

```sh
export TORRBABY_URL=http://127.0.0.1:8765
export TORRBABY_TOKEN_FILE="$HOME/Library/Application Support/Torrbaby/api.token"
python3 torrbaby.py request GET /v1/status
python3 torrbaby.py request GET /v1/torrents
python3 torrbaby.py request GET /v1/events
python3 torrbaby.py request PATCH /v1/settings --data '{"paused":true}'
python3 torrbaby.py request PATCH /v1/settings --data '{"paused":false}'
python3 torrbaby.py request POST /v1/reconcile
python3 torrbaby.py request POST /v1/plex/scan
```

Use `--url` and `--token-file` instead of environment variables if preferred.
For a loopback-only service, an SSH tunnel is sufficient for development.

| Endpoint | Purpose / request body |
| --- | --- |
| `GET /v1/status` | Version, uptime, last poll/error, settings, job phases, pending scans |
| `GET /v1/settings` | Runtime settings |
| `PATCH /v1/settings` | Persistent `paused`, `poll_seconds` (5–3600), `source`, `movies`, `tv` |
| `GET /v1/events` | Most recent 1,000 timestamped audit events |
| `GET /v1/torrents` | Torrent progress, speeds, errors, peers, activity timestamps, ratios, files, labels |
| `POST /v1/torrents` | `{"magnet":"magnet:?…","paused":false}` or base64 `metainfo` |
| `GET /v1/torrents/{hash}` | One torrent by full stable hash |
| `POST /v1/torrents/{hash}/start` | Start torrent |
| `POST /v1/torrents/{hash}/stop` | Pause torrent |
| `POST /v1/torrents/{hash}/verify` | Ask Transmission to verify local files |
| `POST /v1/torrents/{hash}/reannounce` | Ask Transmission to contact trackers |
| `POST /v1/torrents/{hash}/remove` | Remove torrent, **keep data by default** |
| `POST /v1/torrents/{hash}/retry` | Reconsider ignored/completed/blocked job; safely reconcile in-flight move |
| `POST /v1/torrents/{hash}/ignore` | Exclude torrent from automation; cannot cancel an in-flight move |
| `POST /v1/torrents/{hash}/lookup` | Opt-in online title suggestions; never renames |
| `POST /v1/torrents/{hash}/rename` | `{"path":"old/path.mkv","name":"New Name.mkv"}` via Transmission |
| `POST /v1/reconcile` | Wake the worker (respects automation pause) |
| `POST /v1/plex/scan` | Request scans of all three libraries immediately |

For destructive removal explicitly send both `delete_data: true` and
`confirm_hash: "<same full hash>"`. Data deletion is limited to managed roots and
refused for shared torrent files. This distinction should be visible in the app.
No zero-peer/slow torrent is automatically removed: the API exposes stalled
state, peer counts, rates, added/activity times and errors for user judgment.

A job's `moving` phase means the request may be in flight; `done` means file
relocation was verified, not that Plex has finished indexing. `blocked` includes
an explanatory error; `ignored` is a deliberate exclusion. Retry failures and
scan errors remain visible remotely. Runtime path changes apply to future jobs;
in-flight jobs retain their original source/destination.

Listener address, credentials, RPC/Plex URLs and token rotation are installation
settings requiring a service restart; every supported day-to-day operation is
available through the API. There is deliberately no remote shell, updater or
arbitrary Transmission RPC passthrough.

## Rename previews

An explicit `lookup` request sends the parsed title to
[TVmaze](https://www.tvmaze.com/api) for TV or the
[Apple iTunes Search API](https://performance-partners.apple.com/search-api) for
movies. Neither requires an API key. They return multiple candidates with
source links and proposed root names. TVmaze metadata is CC BY-SA; clients must
retain attribution and follow its data license. The application code is MIT.
The movie catalog is not comprehensive; no result is a valid outcome.

Lookups are opt-in and independent of the completion worker, so outages cannot
block sorting. Preview results are not persisted or automatically applied.
Check titles, years, season/episode ranges and edition distinctions. Renaming a
folder preserves its children; it doesn't reorganize episode files into seasons.
Use the `rename` endpoint to accept a reviewed name, preserving the extension.
It refuses collisions and shared paths, records the request, verifies the result
and queues a Plex scan. A future app can provide a review/accept screen.

## Development

```sh
python3 -m unittest discover -s tests -v
python3 -m py_compile torrbaby.py scripts/install.py
```

Tests use temporary files and fake RPC/Plex backends, plus a real local HTTP
server for API authentication checks. They do not touch your media library.

The shared SwiftUI client in `Apple/` runs on iOS, iPadOS and macOS. Playback uses
Apple's native `AVPlayerViewController` / `AVPlayerView`. The daemon serves original file bytes with HTTP range support; it never transcodes.

Protocol references: [Transmission 4.1 RPC](https://github.com/transmission/transmission/blob/4.1.4/docs/rpc-spec.md),
[Plex scan commands](https://support.plex.tv/articles/201638786-plex-media-server-url-commands/).


## Native apps

Open `Apple/Torrbaby.xcodeproj` and select the shared **Torrbaby** scheme. There
are no Swift package dependencies. The same target builds a universal iPhone/iPad
app and a native Mac app. Minimum versions: iOS/iPadOS 18 and macOS 15.

The app provides torrent search, live transfer status, active/download/seeding
filters, a persistent compact layout toggle, and a **Not progressing** page.
The latter shows Transmission-reported stalls, errors, and active downloads with
zero download rate and no reported activity for five minutes; newly added or
manually paused torrents are not marked unproductive merely for being idle.
Details include files, pause/resume, verification, reannounce, reviewed renaming,
organization retry/ignore and explicit removal with a separate delete-files choice.
Automation activity, pending Plex scans and folder/poll settings are remote.

In **Connection settings**, enter your private server URL and API token, or import
a JSON file containing `server` and `token`. Tokens go into the device's Keychain;
they are never bundled, published or stored in ordinary preferences. There are
no hardcoded credentials or server addresses in the app. HTTP is enabled for
private Tailscale endpoints; use HTTPS outside an encrypted private network.
The Debug build optionally reads a one-use `TORRBABY_BOOTSTRAP_FILE` for local
smoke tests; Release builds contain no such import path.

On macOS, **Settings → Magnet links → Use Torrbaby for magnet links** requests the
system default-handler change. Registering the app alone does not intentionally
replace an existing default. Magnet links open the Add Torrent review screen;
they are not automatically downloaded. The same URL handler is present on iOS.

### Native playback prototype

`POST /v1/torrents/{hash}/play` with `{"file_index": 0}` returns a relative `path`
and expiry. Resolve it against the server URL and give it to Apple's player.
The URL grants six hours of read-only access to that one torrent file; it does
not include the management token. `GET` and `HEAD` support single byte ranges,
including suffix ranges, with 206/416 responses. Expired or altered URLs fail.
URLs are not logged with their query strings. Treat playback URLs as private.

When macOS blocks direct volume reads, the daemon can fall back to Plex's
**original-file** `/library/parts/…` endpoint after matching the exact filesystem
path in Plex's library. It validates the returned byte range and size. This does
not use Plex's transcoding or playback UI, and Plex credentials stay server-side.
The fallback requires the file to be indexed in Plex. Set
`plex_stream_fallback: false` in server configuration to disable it.

On macOS the player opens in its own movable, resizable window with standard close controls and Command-W. The player has no separate title/Done frame. On iOS/iPadOS, titles are supplied as native player metadata. macOS AVPlayerView
does not expose the same title overlay, so no custom title overlay is added. Apple's controls handle playback, seeking, fullscreen, audio/subtitles
and Picture in Picture as supported by the platform/file. Left/Right arrows skip
15 seconds; Escape dismisses windowed playback on Mac (native fullscreen exits
first), and iOS sheets can be swiped down. Unsupported formats produce an error.
AVFoundation chooses the platform decoder; the app does not implement a decoder
or guarantee hardware decoding for every format. No incomplete-file streaming,
transcoding or separate subtitle loading is implemented.

### Signed builds and OTA

No App Store Connect app record is required for registered-device development or
ad hoc distribution. You need an Apple Developer team, signing certificate and
profile covering each device. The current workflow supports development-signed
OTA, matching an existing private OTA host. Developer Mode may be required.
App Store Connect records are needed later only if you choose TestFlight or the
App Store. iPhone/iPad share a universal app; a macOS platform can be added later.
See [Apple's registered-device distribution guide](https://help.apple.com/xcode/mac/current/en.lproj/dev7ccaf4d3c.html).

```sh
export DEVELOPMENT_TEAM=YOUR_TEAM_ID
# Optional ASC_KEY_PATH, ASC_KEY_ID, ASC_ISSUER_ID for noninteractive provisioning.
bash scripts/build_apple.sh ios
python3 scripts/package_ota.py \
  --app local-state/apple/ios/Build/Products/Release-iphoneos/Torrbaby.app \
  --publish-dir /path/to/existing/https-root/torrbaby \
  --base-url https://your-private-host/torrbaby \
  --device REGISTERED_DEVICE_UDID
```

The packager verifies signatures, identity, universal device families, profile
expiry and device coverage. IPA and manifest live in an immutable build folder;
the installer page switches atomically. It does not alter the host configuration
or neighboring apps. Keep the HTTPS host reachable from the installing device.
Provisioning profiles expire, so rebuild before the displayed expiration date.

For a Mac build, use `bash scripts/build_apple.sh macos` and set
`MAC_SIGNING_IDENTITY` to your Developer ID identity for distribution (defaults to
local ad hoc signing). Notarize externally before broad downloaded distribution.
Build numbers default to the commit count; override `TORRBABY_BUILD_NUMBER` when
needed. All generated output is ignored by Git.


### Shared playback and devices

Diamond owns the SQLite playback history. Each app installation stores a UUID in
its local Keychain and registers with a default host/device name. Settings lists
registered devices and allows renaming any of them. On iOS, the system may supply
a generic device name; rename it in the app if desired. A shared API token grants
control of the personal library; device IDs are identifiers, not credentials.

Playback automatically resumes an unfinished file. Apps report every five seconds,
on pause, on scene changes and on dismissal, retaining unsent reports locally for
retry. A newly started session supersedes older sessions for the same file;
sequence numbers reject out-of-order updates. Reaching 95% marks the file watched.
Watched files start from the beginning. The status menu beside each file can mark
it watched or unwatched; marking unwatched resets its position. Torrent hash plus
file index identifies a file across Transmission moves and renames. Removing and
re-adding a differently packaged torrent does not automatically merge its history.

All controls are available through the same bearer-authenticated API and CLI:

- `GET /v1/clients`; `POST /v1/clients` with `client_id` (UUID), `name`, `platform`.
- `PATCH /v1/clients/{id}` with `name`.
- `GET /v1/playback` returns shared positions, watched status and last device.
- `POST /v1/torrents/{hash}/play` accepts `file_index` and optional registered
  `client_id`; returns `session_id`, `media_id`, `resume_position` with the stream URL.
- `POST /v1/playback/progress` with `session_id`, increasing integer `sequence`,
  `position`, `duration` (seconds), and optional `ended` boolean.
- `POST /v1/torrents/{hash}/watched` with `file_index` and `watched` boolean.

### Subtitle roadmap

The next subtitle milestone is text-extractable embedded tracks and text sidecar
files. Diamond will extract, normalize, translate into Chinese or English and cache WebVTT,
preserving timing and originals. Original and translated tracks should remain
selectable through Apple’s native player. No OCR, image-based subtitle conversion
or burned-in subtitle processing is planned. Translation model/provider selection
and a compatible native subtitle delivery path are still to be implemented.

App-controlled font, size, text opacity and background opacity should use Apple’s
native renderer. Apple’s `AVPlayerItem.textStyleRules` applies only to WebVTT, so
these controls cannot restyle every existing embedded subtitle format directly.
No subtitle extraction, translation or appearance controls are implemented yet.

Bundle IDs are `org.torrbaby.ios` (iPhone and iPad) and `org.torrbaby.mac` (macOS). Both use the same shared Swift sources and display name.

Transfer graphs offer 1h, 6h and 24h ranges, using server-wide Transmission rates. Diamond samples every 30 seconds even when organization is paused and retains 25 hours in SQLite. `GET /v1/rates` returns one-minute averages for the last 24 hours; history begins at deployment and is not reconstructed retroactively.


### Metadata on Diamond

The daemon gathers metadata while downloads run and stores candidates and selected
records in SQLite. It checks existing Plex file matches, TVmaze's keyless show and
episode API, and Apple's keyless movie search. It does not use Apple's promotional
artwork as library artwork. Posters from Plex and TVmaze are cached on Diamond.

In Settings, save a TMDB API Read Access Token or API key to enable richer movie/TV
records, genres, US content ratings, runtime, posters, available logos and episode
lists. The credential is stored in `tmdb.token` beside the database with mode 0600;
clients never receive it. TMDB is free for noncommercial use with attribution.
Metadata, images and their provider terms remain separate from our MIT source code.
TVmaze attribution: <https://www.tvmaze.com/api> (CC BY-SA).
TMDB: <https://www.themoviedb.org/>. This product uses the TMDB API but is not
endorsed or certified by TMDB. Missing fields remain missing; ratings are not
inferred from a numeric audience score. Episode ordering follows provider numbering.

High-confidence exact title/year matches and explicitly confirmed choices are
renamed automatically only after completion and movement out of Downloads.
Transmission handles every root/file/sidecar rename and Plex scans follow. Movie
collections, shared paths, collisions and unclear episode numbering remain for
review. Pausing automation also pauses renaming. Metadata never overwrites media.
Collection decomposition and moving episodes into newly created season folders
are future work; this version preserves torrent structure and gives episode files
canonical names when their season/episode markers are unambiguous.

API: `GET /v1/metadata`, `GET /v1/providers`, `POST /v1/providers/tmdb` with `token`,
`POST /v1/torrents/{hash}/metadata-refresh`, `POST /v1/torrents/{hash}/metadata-select`
with candidate index, and authenticated `GET /v1/torrents/{hash}/artwork`.

### Apple TV prototype

The shared project also builds for tvOS 18+, with bundle ID `org.torrbaby.tv`.
Its first screen is a remote-friendly library and file chooser; playback uses
AVPlayerViewController and the same shared playback history. Enter the server and
API token in Settings using the Apple TV/iPhone keyboard. It currently uses simple
placeholder movie cards; polished artwork browsing and TV torrent controls are
future work.

Build with `DEVELOPMENT_TEAM=YOUR_TEAM scripts/build_apple.sh tvos`. Pair the Apple TV
in Xcode/Device Hub while it is on the same network: on the TV open Settings →
Remotes and Devices → Remote App and Devices, then pair using its displayed code.
Install/run the development build through Xcode or `xcrun devicectl device install
app --device DEVICE_ID PATH_TO_APP`. No App Store Connect record is needed for
this direct development route. Safari-style OTA installation links are not the
Apple TV delivery path. TestFlight is an optional later route and requires an App
Store Connect record and uploaded build.
