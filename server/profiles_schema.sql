CREATE TABLE IF NOT EXISTS profiles (
 id TEXT PRIMARY KEY, name TEXT NOT NULL, created REAL NOT NULL
);
INSERT OR IGNORE INTO profiles VALUES('profile-jon','Jon',0);
CREATE TABLE IF NOT EXISTS device_profiles (
 client_id TEXT PRIMARY KEY REFERENCES clients(id),
 profile_id TEXT NOT NULL REFERENCES profiles(id)
);
CREATE TABLE IF NOT EXISTS profile_playback (
 profile_id TEXT NOT NULL REFERENCES profiles(id), media_id TEXT NOT NULL REFERENCES media(id),
 position REAL NOT NULL DEFAULT 0, duration REAL NOT NULL DEFAULT 0,
 watched INTEGER NOT NULL DEFAULT 0, last_played REAL NOT NULL DEFAULT 0,
 client_id TEXT REFERENCES clients(id), active_session TEXT,
 PRIMARY KEY(profile_id,media_id)
);
CREATE TABLE IF NOT EXISTS session_profiles (
 session_id TEXT PRIMARY KEY REFERENCES playback_sessions(id) ON DELETE CASCADE,
 profile_id TEXT NOT NULL REFERENCES profiles(id)
);
CREATE TABLE IF NOT EXISTS profile_appearance (
 profile_id TEXT PRIMARY KEY REFERENCES profiles(id) ON DELETE CASCADE,
 avatar_symbol TEXT NOT NULL DEFAULT '',
 text_color TEXT NOT NULL DEFAULT '#70E8C2',
 background_color TEXT NOT NULL DEFAULT '#243A38'
);
