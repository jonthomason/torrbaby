PRAGMA journal_mode=WAL;
PRAGMA synchronous=FULL;
PRAGMA foreign_keys=ON;
        CREATE TABLE IF NOT EXISTS organizer_state (id INTEGER PRIMARY KEY CHECK(id=1), document TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS clients (id TEXT PRIMARY KEY, name TEXT NOT NULL, platform TEXT NOT NULL, created REAL NOT NULL, last_seen REAL NOT NULL);
        CREATE TABLE IF NOT EXISTS media (id TEXT PRIMARY KEY, torrent_hash TEXT NOT NULL, file_index INTEGER NOT NULL, name TEXT NOT NULL, length INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS playback (media_id TEXT PRIMARY KEY REFERENCES media(id), position REAL NOT NULL DEFAULT 0, duration REAL NOT NULL DEFAULT 0, watched INTEGER NOT NULL DEFAULT 0, last_played REAL NOT NULL DEFAULT 0, client_id TEXT REFERENCES clients(id), active_session TEXT);
        CREATE TABLE IF NOT EXISTS playback_sessions (id TEXT PRIMARY KEY, media_id TEXT NOT NULL REFERENCES media(id), client_id TEXT NOT NULL REFERENCES clients(id), started REAL NOT NULL, sequence INTEGER NOT NULL DEFAULT -1, ended INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS transfer_rates (time INTEGER PRIMARY KEY, download REAL NOT NULL, upload REAL NOT NULL);
        CREATE TABLE IF NOT EXISTS metadata (hash TEXT PRIMARY KEY, document TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS library_assets (id TEXT PRIMARY KEY, document TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS library_entities (id TEXT PRIMARY KEY, document TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS torrent_rates (hash TEXT NOT NULL, time INTEGER NOT NULL, download REAL NOT NULL, upload REAL NOT NULL, samples INTEGER NOT NULL, PRIMARY KEY(hash,time));
        CREATE INDEX IF NOT EXISTS torrent_rates_time ON torrent_rates(time);
        CREATE TABLE IF NOT EXISTS eta_state (hash TEXT PRIMARY KEY, started REAL NOT NULL, updated REAL NOT NULL, activity REAL NOT NULL, rate REAL NOT NULL);
        CREATE TABLE IF NOT EXISTS pairing_grants (id TEXT PRIMARY KEY, digest TEXT NOT NULL UNIQUE, expires REAL NOT NULL, client_id TEXT);
        CREATE TABLE IF NOT EXISTS device_credentials (client_id TEXT PRIMARY KEY REFERENCES clients(id), digest TEXT NOT NULL UNIQUE);
        CREATE TABLE IF NOT EXISTS offline_progress_events (client_id TEXT NOT NULL REFERENCES clients(id), event_id TEXT NOT NULL, response TEXT NOT NULL, received REAL NOT NULL, PRIMARY KEY(client_id,event_id));
        CREATE TABLE IF NOT EXISTS subtitle_preferences (scope TEXT PRIMARY KEY, document TEXT NOT NULL, updated REAL NOT NULL);
        CREATE TABLE IF NOT EXISTS media_inventory (asset_id TEXT PRIMARY KEY, revision TEXT NOT NULL, document TEXT NOT NULL, updated REAL NOT NULL);
        CREATE TABLE IF NOT EXISTS subtitle_priorities (asset_id TEXT PRIMARY KEY, priority INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS library_operations (
 id TEXT PRIMARY KEY,
 package_id TEXT NOT NULL UNIQUE,
 action TEXT NOT NULL CHECK(action IN ('delete','junk')),
 phase TEXT NOT NULL CHECK(phase IN ('accepted','moving','verifying','seeding','detaching','deleting','cleanup','done')),
 preview TEXT NOT NULL,
 destination TEXT,
 error TEXT,
 created REAL NOT NULL,
 updated REAL NOT NULL
);
CREATE TABLE IF NOT EXISTS library_operation_assets (
 asset_id TEXT PRIMARY KEY,
 operation_id TEXT NOT NULL REFERENCES library_operations(id)
);

CREATE TABLE IF NOT EXISTS library_operation_runtime (
 operation_id TEXT PRIMARY KEY REFERENCES library_operations(id),
 document TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS library_operation_archive (
 id TEXT PRIMARY KEY,
 package_id TEXT NOT NULL,
 document TEXT NOT NULL,
 archived REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS profile_hidden_titles (
 profile_id TEXT NOT NULL REFERENCES profiles(id), title_id TEXT NOT NULL,
 hidden_at REAL NOT NULL, PRIMARY KEY(profile_id,title_id)
);
