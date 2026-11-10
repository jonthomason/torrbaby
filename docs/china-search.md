# World and China search — implementation in progress

The goal is separate World/China selections with parallel result/detail flows on Apple and Android. No cross-market fallback. World remains the default for old clients.

2026-11-10: server Query now has a serde-defaulted market, partitioned providers and cache keys. China adapters for public DMHY/Mikan RSS compile; a focused test verifies base32 magnet conversion, Mikan hash/size extraction, foreign-host rejection and provider partition. CLI accepts --market world|china. Not deployed yet; selectors not yet implemented.

Live evidence: both RSS search endpoints return real Chinese results on M5 without authentication. Diamond urllib and curl fail (timeout/no route); investigate routing/DNS before release, do not assume an account issue. These providers primarily cover animation; broader Chinese movie/TV sources are still required. BT4G also unreachable from Diamond. mikanani.tv redirects to an unrelated survey domain; do not use it. Fixtures saved under ignored local-state/china-search.

Interleaved UI request: Apple build 97 published to Miyu Install, macOS 97 installed. Detail buttons use consistent 12pt spacing, horizontal when possible, stacked on narrow layouts. Find releases removed from Apple detail pages and Android hero source. Mark as Watched labels corrected. Manage Files gets file cards and full-width actions on Apple. macOS rendered layout verified; mobile portrait/landscape visual verification and Android build still outstanding. Stale profiles-check macOS process was terminated; installed build explicitly relaunched. Client edits depend on other session's uncommitted baseline; retain all shared changes.
