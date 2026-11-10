# Search resolution filters and shared TMDB cache — build 113

World and China searches share the same metadata request cache. Successful complete TMDB JSON responses are cached in memory and on Diamond disk: search responses for six hours, other metadata for 24 hours, bounded to 128 MB. Keys normalize search whitespace/case and parameter order, retain language and other request options, and exclude credentials. Concurrent identical requests share one fetch. Errors are not cached; credential validation deliberately bypasses this cache.

Artwork is reused by canonical TMDB poster path and requested image size, with a bounded 64 MB persistent image cache. Apple search rows also reuse decoded-source bytes and in-flight requests by title/year/media type rather than torrent release filename. Different aliases can still require matching searches before resolving to the same entity.

Apple and Android provide remembered All/2160p/1080p/720p filters. Filtering uses the existing result set and does not submit another provider search. Hidden rows do not initiate enrichment; Android skips obsolete queued enrichment jobs. The server normalizes 4K to 2160p.

Validation: offline persisted-TMDB-response test passed across fresh Metadata instances with no token; all Apple targets and Android release built; 563 EN/ZH localization entries passed checks; git diff whitespace check passed. Mac controls visually checked in both languages without submitting a search. No Chinese torrent-server test queries were made. iOS/tvOS/Android were build-checked, not visually checked in this release.

Delivery: Apple build 113 published on Miyu Install and installed as the sole running canonical Mac app; tvOS simulator build 113 installed. Android 0.3.12 build 38 published to the same OTA page. Diamond daemon deployed and its live executable SHA-256 matched the signed local build. Public OTA manifests verified for Apple 113 and Android 38.
