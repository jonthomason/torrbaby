# Import readiness and numeric titles

Deployed 2026-11-10; backup `china-search-20261110-103154`.

Seinfeld's original issue was fixed in the daemon: high-confidence TV collection
matches propagate to matching episodes without manual confirmation. This was not
a one-off database edit. All 171 video assets remain grouped after rescanning.

The year parser previously took the first year-shaped token. For `2001 A Space
Odyssey (1968)`, that produced an empty query. It now keeps leading title numbers
and selects the last release-year token before season/encoding markers. Focused
checks cover 2001, 1917, and Blade Runner 2049. Punctuation-normalized comparison
already handles the missing colon. The live normal metadata pipeline matched
2001 to TMDB movie 62 and populated its poster, without manual metadata insertion.

Completed torrents wait for all media files to be identified (or classified as
extras) before moving to Movies/TV. Canonical naming runs through Transmission
before the move, and the move plan uses Transmission's refreshed paths. Existing
shared-path and destination-collision checks remain in place. The public library
requires completed, matched assets in library roots; unresolved media remains
available through the matches API and torrents.

User-initiated torrent file/folder renames invalidate the affected asset matches,
retain the new name as a lookup override, and trigger scan, metadata collection,
organization, and another scan. Automatic canonical renames do not invalidate their
own metadata. Explicit metadata refresh bypasses failed/unmatched lookup backoff.
No arbitrary filesystem rename UI was added by this change.

Validation: numeric-title regression passed; signed release and bundle verification
passed; live catalog matched 2001 with poster and contained no unmatched titles.
Both pending subtitle jobs resumed. A destructive live rename was not used as a test.
