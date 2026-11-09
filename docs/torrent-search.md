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
