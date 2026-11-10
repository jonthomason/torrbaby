# Search size and link retrieval — Apple 110 / Android 36

Unknown sizes no longer display Size… or Size unavailable. The entire size/availability line disappears when neither value is known. Actual size remains prominent below the title, with a smaller headline font on Apple and 16sp on Android.

The saved BT之家 search response contains thread ID, subject, author, date, view/post counts and attachment count, but no payload size. The post's full-title blockquote can contain the size omitted from the search subject. Previously every visible size lookup invoked full details, downloading all torrent attachments in batches of two before returning.

The new authenticated POST /v1/search/torrents/summary reads only the post page. It returns an unambiguous full-title size label plus attachment IDs, names and URLs. It does not mistake the small .torrent attachment's own size for the media payload, or guess among differing size labels. Post reads share the existing cache/in-flight coalescing with details and Add. Apple retains the release choices so Add can select a single attachment or ask the user to choose immediately. Full detail and Add remain separate: Add retrieves only the selected attachment and confirms success only after Transmission responds. The selected attachment still requires a network fetch if not already cached; this is not a promise of zero-latency Add.

Validation: cargo check; saved public fixture test extracts 21.40 GB and ignores attachment-link sizes; signed Apple Release builds; Android release compilation/signature; localization and whitespace checks. No live search-provider test traffic was generated, and no torrents were added. Consequently actual provider latency has not been measured for this release.

Diamond deployed with a database/app backup and pending subtitle-job preservation; authenticated status and summary-route validation passed. Apple110 and Android36 OTA artifacts were fetched and SHA-256 verified. Mac110 installed at ~/Applications/Torrbaby.app. Mobile108 already contains the compact sidebar/artwork work;110 includes it and the Mac109 footer correction.
