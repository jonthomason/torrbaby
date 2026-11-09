# Torrbaby

A small, private Transmission companion. It sorts completed torrents into Movies
or TV, preserves seeding by moving through Transmission, requests Plex scans,
and exposes an authenticated HTTP API for native apps and a CLI.

MIT licensed. Python 3.9+ standard library only. No third-party packages, bundled
Transmission source, metadata API keys, or hardcoded server addresses.

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
- `~/Library/Application Support/Torrbaby/state.json`: settings, jobs and recent events
- `~/Library/Application Support/Torrbaby/torrbaby.log`: rotating operation log
- `~/Library/LaunchAgents/org.torrbaby.daemon.plist`: per-user service

This is a LaunchAgent, so it starts in the user's login session. Transmission and
Plex are independent services. For Linux or a different supervisor, copy
`config.example.json`, create a private random token file, edit paths and run:

```sh
python3 torrbaby.py serve --config /path/to/config.json
```

Only one process may own a state file. Back up configuration and state when
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

Roadmap: a native SwiftUI management app for iOS, iPadOS and macOS consuming this
API, then a separate streaming feasibility prototype using Apple's native
`AVPlayerViewController` / `AVPlayerView`. No streaming, transcoding or UI
framework is bundled into the daemon.

Protocol references: [Transmission 4.1 RPC](https://github.com/transmission/transmission/blob/4.1.4/docs/rpc-spec.md),
[Plex scan commands](https://support.plex.tv/articles/201638786-plex-media-server-url-commands/).
