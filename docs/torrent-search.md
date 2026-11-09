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
