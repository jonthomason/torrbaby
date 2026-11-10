# Search polish and shared cache — Apple 112

- Apple result-row Add and info controls are vertically centered.
- The info sheet receives the size already displayed in its row, before optional details arrive. Removed the duplicate standalone size line.
- Android37 already centers row controls and uses cached size in its info page; no separate Android UI change was needed.
- tvOS112 simulator build passed and was installed. macOS112 installed/restarted; iPhone/iPad112 published with remote IPA hash verification.
- BT successful response caching now persists under the daemon's search-cache directory: search responses 10 minutes, posts/attachments 24 hours, bounded to 64 MB. Concurrent duplicate reads share a single fetch. Caches are shared by all clients and survive daemon restarts.
- Removed local Search is busy concurrency rejection and the proposed global BT request semaphore. No arbitrary user quotas/cooldowns were added. Actual provider Retry-After responses remain source-provided, not a Torrbaby search-frequency policy.

Validation used saved public fixtures and a local cache-restart test. The cache test forces any accidental network fallback through localhost port9 so it cannot contact a real indexer. Both passed. Rust/server and Apple builds passed; localization and whitespace checks passed. No tests queried Chinese torrent servers. New live-source UI behavior was not exercised by the agent.
