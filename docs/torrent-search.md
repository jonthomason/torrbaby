# Torrent search

Authenticated daemon endpoints: `POST /v1/search/torrents` and
`POST /v1/search/torrents/detail`. Search accepts query, optional year/season/episode,
and optional sources. Default sources are `tpb` and `csv`; `nyaa` is optional.
Nyaa currently times out from Diamond, although reachable from the development Mac.

Results merge by info hash, rank matching title/year/episode before seeders,
and distinguish name-derived hints from source listings. These are not verified
media tracks. Detail lookup exposes source descriptions and file lists where
available; Torrent CSV does not supply these. Adding is explicit and supports
paused addition using the existing Transmission endpoint.

Apple clients expose Find torrents and title-context Find releases. Android TV
has the same entry points. Existing media browsing is independent of search.

## Verification, 2026-11-09

- Rust parser/ranking/magnet checks pass, including canonical unescaped xt URN.
- iOS, macOS, tvOS simulator build 84 and Android build 20 compile.
- Diamond deployment backup: `rust-20261109-204207`.
- Live Sintel search: CSV 3, TPB 37, 39 unique; 1.64 seconds cold, 4 ms cached.
- TPB detail: seven files, 2388 description characters.
- Search magnet accepted paused in Transmission, zero downloaded bytes, then
  test entry removed without deleting data.
- Existing subtitle job saved and resumed across daemon restart.

Pending: visual interaction checks on all clients, publishing search-enabled
client builds, expose additional source language metadata where meaningful,
and verify release/detail/add flow through client UI. Mobile OTA remains stable
build 83 until the search-enabled release is ready.

## Search release, 2026-11-09

Apple build 84 and Android build 20 published to Miyu Install; both downloaded
artifacts matched their SHA-256 checksums over HTTPS. Mac 84 installed in
`~/Applications/Torrbaby/Torrbaby.app`; tvOS 84 installed on the existing simulator.
Android 20 sent to TCL at 192.168.1.201; native installer confirmation still
needs verification (`local-state/tcl-build20-install.log`).

Source languages now appear when the listing supplies nonempty language strings;
missing/unknown fields are omitted. File-derived tags appear on Apple details.
The default source option is named Recommended (TPB + CSV); Nyaa is opt-in.

Visual acceptance remains open: CUA returned `cgWindowNotFound` for both Torrbaby
and Finder, even though the installed Mac process was running and its main thread
sample was idle in the AppKit event loop. Do not treat build success as UI proof.

## Matching refinement

Season and episode ranges (S01-S04, Season 1 to 8, S02E03-E05) now rank as packs
containing the requested episode/season, below exact matches but above conflicting
releases. Title searches stay broad so literal S02 filtering does not exclude
those releases. Targeted regression checks pass. Deployed to Diamond with backup
`rust-20261109-204903`; live search returned 39 unique results in 1.01 seconds,
cache in 3 ms, and paused magnet add/remove passed again.

TCL confirmed build 20 installed through PACKAGEINFO. Android 21 adds a small
search-state fix: old results cannot open while a new query is pending, preventing
a disabled Search button on return. Build 21 is published with verified HTTPS
checksum; Apple remains 84. Native window inspection is still unavailable.

## Completion audit — 2026-11-09

| Requirement | Evidence | Status |
| --- | --- | --- |
| Keyword searches across TPB and other sources | Live Diamond TPB/CSV results; Nyaa optional adapter | Verified |
| Smart contextual matching | Title/year/season/episode scoring, pack ranges, duplicate hashes; targeted tests | Verified |
| List results and load magnets | Client implementations compile; live API add paused, zero downloaded bytes, remove | API verified; UI acceptance pending |
| Further audio/details/file inspection | Source descriptions, language fields, release tags, TPB live seven-file listing | API verified; UI acceptance pending |
| All supported clients | Apple 84 macOS/iOS/tvOS builds; Android 21; HTTPS OTA hashes; TCL installed 20 | Delivery verified; UI acceptance pending |
| Commit/push | 75085bb and predecessors on codex/subtitles | Verified |

Desktop automation has returned `cgWindowNotFound` on three consecutive goal
turns, for both Torrbaby and Finder. The outstanding user question asks whether
the Mac is locked; this is not established as the cause. No pending build or
installer process needs restarting. The goal is blocked on client UI access,
not complete. Resume by checking an accessible desktop, then verify search,
open details, paused addition and return navigation on Mac/mobile/TV clients.
Android 21 is on OTA; TCL currently has confirmed build 20.

## Unlocked desktop acceptance — Apple 85

Desktop access recovered after the user unlocked the Mac. Verified through CUA:
- Mac 84: Sintel keyword search, results from both indexes, release description
  with English/audio information, seven files, Back preserves search results.
- Signed iPhone simulator 84: library Find releases passes title/year; independent
  Sintel keyword search, details, Add paused, and the Torrent added toast.
  Diamond confirmed status 0 and zero downloaded bytes; test torrent removed
  without deleting files.
- tvOS 84 simulator: remote navigation to Find torrents, native text keyboard,
  title-context Homeland search, live results and CSV release details.
- iPad simulator 85: title-context Matrix search, corrected ordering, full source
  names in menu (Recommended, The Pirate Bay, Torrent CSV, Nyaa).

The unsigned simulator test build initially could not persist Keychain credentials;
rebuilt with DEVELOPMENT_TEAM and normal simulator signing, bootstrapped using the
existing DEBUG-only one-use local file, then installed/verified the Release build.
No credentials were printed or embedded in released apps.

Apple 85 replaces the cramped iOS segmented sources with a menu. Matching now
penalizes extra title words before year/season markers; live Matrix results place
original-film releases ahead of the mislabeled Reloaded result. Targeted tests pass.
Daemon deployed with backup rust-20261109-213329; resumed subtitle job. Live search,
cache, seven-file detail and paused add/remove all passed again. Apple OTA85 HTTPS
checksum verified; Mac85 installed/launched; tvOS85 and signed mobile85 simulators
installed. Android remains OTA21, TCL confirmed20.

Outstanding: user TCL search/details check requested asynchronously. CUA rejects
its standalone emulator binary as an invalid app, so Android's actual search UI
has not been visually accepted. Goal is not complete yet.

## Completion

The user confirmed that searching for Sintel and opening a result works normally
on the physical TCL. This resolves the remaining Android UI acceptance item.
Together with the Apple UI checks, live multi-provider searches, source details
and file lists, matching regression checks, and verified Transmission addition,
the torrent-search goal is complete. Nyaa remains optional and may time out from
Diamond; the default TPB + Torrent CSV sources are operational. Index descriptions
and release tags are source claims, not independently probed media tracks.

Unrelated Android subtitle/playback edits present at closure were left untouched.

## Find redesign — Apple build 88

- Find is directly below Home in the main Library navigation. Title pages retain
  a more visible Find releases action.
- Results separate readable title, year/episode, quality/codec/source, size,
  language tags, and reported seeds/peers. Missing languages say Languages not
  listed; zero seeds says No reported seeds. These remain index/name claims.
- Mac and iPad show results beside details when at least 640 points are available;
  narrower iPad windows and iPhone use pushed details. Selection survives resizing
  and refresh when its hash remains in the results; a new search selects its first
  result. Searching disables stale result interactions.
- Details retain the complete release name. Description is collapsible on Mac/iOS,
  making files and the add action easier to reach. tvOS retains a plain description.
- macOS, iOS device/simulator, and tvOS simulator builds pass. A lightweight title
  cleanup check covered Sintel, Matrix, Homeland, 1917 and 2001: A Space Odyssey.
- Mobile OTA88 published and downloaded IPA SHA-256 verified over HTTPS. Mac88
  installed, iPhone/iPad release simulators updated. Native visual inspection is
  pending: CUA currently returns cgWindowNotFound for both Torrbaby and Device Hub;
  the user was asked to unlock the Mac.
- Existing concurrent subtitle/player/server edits were preserved and not included
  in this change's commit. Builds used the shared current workspace.


## Search polish — Apple build 90 (2026-11-10)

Release rows now include audio tags (including AAC, AC-3/E-AC-3, TrueHD and Atmos). Optional authenticated artwork requests are separate from search: exact normalized title/year matches, unique exact TV matches, collection rejection, smallest configured TMDB poster size, six-hour shared memory cache. Artwork absence/errors never block search. Live Matrix match returned w92; another encoding reused the cache in 3ms. Names without a movie year and collection names returned no artwork.

Successful Apple search additions move focus to the query and select its text; narrow layouts dismiss release details first. Typing the next query clears prior contextual year/season/episode filters. Failures remain in detail. In-app notices sit at the bottom; iOS foreground system notifications are suppressed to avoid a second banner over search. tvOS retains its remote-friendly detail behavior.

Validation: targeted Rust search tests (2), cargo check, signed server build and Mac/iOS/tvOS builds passed. Mac build 90 installed; mobile build 90 published and HTTPS IPA checksum verified. Builds include concurrent worktree subtitle changes, which are not included in this search commit. Post-add keyboard/selection behavior still needs interactive device verification.

Operational incident: deploying server at 04:32:59 restarted the daemon and interrupted active mobile playback. HLS Manager::new clears cached packages on restart, invalidating open playback URLs. At 04:37 both LAN and Tailscale library/torrent/events/playback endpoints returned HTTP 200 in under 0.1s. No further daemon restarts while user is watching. Follow-up: preserve ready HLS packages across restart or transparently reprepare/retry, and avoid active-session interruption during deployments.
