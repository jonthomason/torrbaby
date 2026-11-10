# BT result actions — Apple 111 / Android 37

BT之家 results now occupy the list without automatic detail navigation. Each prepared row offers Add and an independent info button on the right. Add submits the cached public .torrent attachment URL directly to Transmission; multiple attachments offer a release choice. No new source lookup or magnet extraction is performed by Torrbaby on this path. Transmission itself downloads the torrent file, and success is reported only after its RPC response.

The row-summary request fetches the post page once using the existing in-flight/cache mechanism to extract size and attachment choices. Initial BT search responses omit attachment URLs, so a post-page request remains necessary. This is not one upstream request for an entire results list. Unknown size remains absent. No attachment downloads are performed during automatic row preparation.

Full details are requested only by the info button, using explicit:true. The daemon returns empty details without upstream access for older clients lacking that flag. Optional full details can still download attachments to inspect their file lists. World-source behavior is unchanged.

Validation: Apple macOS/iOS Release builds and Android Release compilation passed; localization checker (561 entries), git diff --check and server build passed. Mac signing used the existing identity with timestamp disabled after Apple's timestamp service failed twice; local signature verification passed. No BT search/detail probes or actual torrent additions were made. Production API checks confirmed automatic legacy details return immediately and invalid attachment URLs are rejected before Transmission. The new result UI has not been visually checked against a live BT query, to avoid more provider traffic.

Diamond deployed with app/database backup. Apple111 and Android37 OTA artifacts fetched and hashes verified; canonical Mac111 installed. Direct URL fetching uses Transmission's network path, unlike the search proxy; real provider download behavior remains to be verified through an intended user Add action.
