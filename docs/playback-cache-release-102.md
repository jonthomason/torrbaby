# Playback cleanup and library navigation — 2026-11-10

## Root causes and fixes

- Bone Lake is MP4. Preparation wrongly applied the 2 GiB MKV index quota (including 256 MiB reservation) even though MP4 requires no MKV disk index. MP4 now bypasses that quota; previously failed storage-limit jobs can retry.
- MKV index preparation now reclaims oldest unused indexes, protecting pending jobs, cached readers, and indexes accessed within the last hour. Source media is never deleted or modified. Background preindexing stops at the reserve threshold instead of evicting and rebuilding continuously.
- Apple player shutdown is idempotent, ends the playback session, and releases its current AVPlayerItem. Android destruction reports the final position before release. A new end-only endpoint handles sessions closed before valid playback progress exists without overwriting watched position.
- See All used a destination NavigationLink while title details used a value NavigationPath. Both now use typed routes. Home shelves avoid repeated titles; a final grid exposes remaining titles. See All retains the complete category.

## Search follow-up

Observed 1lou HTTP 429 (search too frequent), rather than assuming all intermittent errors were network failures. Requests are paced/coalesced, raw results cached across filters, and Retry-After cooldown honored. Provider limits still apply. China results show source-reported sizes immediately where present and lazily resolve attachment sizes otherwise; absent peer counts are omitted. Chinese forum tags/episode-pack notation are removed before TMDB matching. Live tagged 逐玉 matched Pursuit of Jade and its poster.

## Release and verification

- Signed Diamond server deployed; rollback backup `china-search-20261110-104922`.
- macOS 102 installed and launched; iOS 102 published on Miyu Install.
- iOS SHA256: `77122ad9a1a0f57d7f46242ec12495967544f6a7cefb3423e2865f297acff0e8`.
- Android 0.3.7 build 33 published; not installed to TCL as part of this release check.
- Android SHA256: `01dc0d4a3df520dd208ac0f223ac0f7382878e90e6c8356a7268463d593b41d6`.
- Apple/macOS/iOS and Android release builds passed. Focused cache reclamation test passed (recent index protected; old unused index removed). No live media was removed.
- macOS UI: See All → Bone Lake remains on details; Back returns to Movies. Played/resumed Bone Lake twice and closed both player windows. Database confirms both sessions ended (sequences 4 and 3). Playback test performed on 101; 102 adds complete See All category contents and was launched connected successfully.
- OTA current.json reports 102; android.json and install page link report 33.
- tvOS was not rebuilt in this release (previous build 99).

The worktree also contains independent profile/file-management changes. This commit deliberately includes only this task's changes; release builds used the shared worktree.
