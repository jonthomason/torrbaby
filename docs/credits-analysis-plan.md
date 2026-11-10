# Credits analysis design (pending implementation)

Run on Diamond alongside subtitle preparation, independently of subtitle extraction or translation success. Every complete playable asset is eligible, including assets with no subtitle tracks. Reuse media identity, file revision and background scheduling infrastructure; do not make credits analysis a dependent subtitle job.

User limits: inspect at most the last 15 minutes of a movie or 5 minutes of an episode. These are analysis budgets, not assumed credit durations. If credits extend past the window or evidence is inconclusive, store no confident boundary rather than declaring the window start to be credits.

Start near EOF and inspect short chunks moving backward. Within each chunk, decode forward from a seek point; arbitrary compressed video generally cannot be decoded frame by frame backward. Sample sparse low-resolution frames first, then refine candidate boundaries. Benchmark chunked reverse search against a single forward decode of the bounded tail before selecting a default: repeated seeks can cost more than sequential decoding.

Detect rolling credits only. Seek backward through seek points and sample several forward-decoded frames at each candidate, requiring sustained coherent upward motion of text regions. A single text-heavy frame is insufficient. Static credit cards, logos, non-rolling credit sequences and black frames do not qualify. Continue searching backward past these within the scan budget. Handle intervening story scenes and post-credit sequences conservatively; do not bridge separate rolling segments across actual scenes. Save segment timestamps, confidence, algorithm version and file revision. Cache analysis; retain no large frame copies and never rewrite source media.

Playback stays responsive and analysis must yield to playback preparation. A detected credits marker is advisory: never terminate playback or destroy a resume position. Percentage-only watched completion has been removed in pending source changes.

Separate follow-up: audit titles without generated subtitles and report explicit reasons (no source, extraction/OCR failure, language already present, translation error, not queued) rather than inferring a missing source track merely from absent translations.

No scanner implemented, no builds or live scanning started by this design note.
