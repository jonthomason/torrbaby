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
