# Pending search row layout — 2026-11-10

User-approved hierarchy: large title; prominent size and reported seeds immediately below; compact small supporting metadata/audio/language text. No placeholder availability line when the source does not report it. Apple uses two-point stack spacing and two compact metadata lines. Android increases title/size prominence and removes provider names from the prominent availability line.

Existing source-reported size and lazy attachment-size retrieval remain in use. Unknown size must not be fabricated. Missing peer counts do not establish availability or speed; source popularity is not a reliable substitute for live seeds.

No new builds or live provider requests for this change. User wants localization/profile changes integrated before fresh builds and combined visual testing. Check Mac/iPad/iPhone/tvOS and Android at narrow widths and larger accessibility text sizes, including multi-attachment size ranges and zero/missing seed counts. Screenshot supplied by user showed the older installed layout.

## Pending duplicate and Add fixes

Saved 1lou fixture contains 50 distinct thread IDs but repeated exact subject strings. Search already makes only one request per selected provider, deduplicates provider selections, and shares cached China queries across filters. Forum listings now group by identical full subject and size within provider, retaining alternative source posts. Known torrents group by normalized info hash. Different encodes remain separate; the short displayed title alone is not a safe duplicate key.

Row size, detail and Add now coalesce/cache the same page/attachment request. Repeated attachment anchors are deduplicated; optional detail fetching preserves healthy attachments when another fails. New /v1/search/torrents/resolve retrieves only the selected torrent file and returns metainfo. Both clients keep Add enabled while optional details load; an unresolved multi-release post asks for a release choice. The format choice is internal. Known magnets remain directly addable. No torrents were added during development verification.

Removed the arbitrary five-second search gap and the fabricated 15-second fallback cooldown. Honor numeric Retry-After from actual HTTP429 responses; describe a local wait separately from a fresh provider refusal. No live source requests were made. Rust check and three focused tests passed: exact-listing grouping, duplicate attachment anchors, and the saved public listing/torrent fixture. App build and visual/interaction verification remain deferred until localization/profile integration. Server/client endpoint changes must ship together.
