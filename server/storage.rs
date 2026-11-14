use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params, types::ValueRef};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::OpenOptionsExt,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

pub struct Database {
    pub connection: Connection,
}
impl Database {
    pub fn open(path: &Path, legacy: Option<&Path>) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)?;
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(10))?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > 10 {
            bail!("Database is newer than this server supports");
        }
        if version > 0 && version < 10 {
            // SQLite produces a consistent snapshot including committed WAL data.
            let backup = path.with_file_name(format!("state-before-profiles-{}.sqlite3", uuid::Uuid::new_v4()));
            connection.execute("VACUUM INTO ?", [backup.to_str().context("Invalid backup path")?])?;
            fs::set_permissions(&backup, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        }
        connection.execute_batch(include_str!("schema.sql"))?;
        add_column(&connection, "processing_failures", "resolution", "TEXT")?;
        let mut database = Self { connection };
        database.migrate_profiles(version)?;
        database.initialize_catalog_revision()?;
        // Fault tolerant: a failed repair is logged and the server starts regardless.
        match database.repair_finished_playback() {
            Ok(0) => {}
            Ok(n) => eprintln!("Marked {n} playback records stopped in the closing credits as watched"),
            Err(error) => eprintln!("Repairing playback stopped in the closing credits failed: {error:#}"),
        }
        let existing: Option<String> = database
            .connection
            .query_row("SELECT document FROM organizer_state WHERE id=1", [], |r| {
                r.get(0)
            })
            .optional()?;
        if existing.is_none() {
            let state = match legacy.filter(|p| p.exists()) {
                Some(path) => serde_json::from_slice(&fs::read(path)?)?,
                None => json!({"jobs":{},"events":[],"settings":{},"scans":[]}),
            };
            database.save_state(&state)?;
        }
        Ok(database)
    }
    pub fn save_state(&mut self, value: &Value) -> Result<()> {
        self.connection.execute(
            "INSERT OR REPLACE INTO organizer_state VALUES(1,?)",
            [value.to_string()],
        )?;
        Ok(())
    }
    pub fn state(&self) -> Result<Value> {
        let text: String = self.connection.query_row(
            "SELECT document FROM organizer_state WHERE id=1",
            [],
            |r| r.get(0),
        )?;
        Ok(serde_json::from_str(&text)?)
    }
    pub fn rows(&self, sql: &str, args: impl rusqlite::Params) -> Result<Vec<Value>> {
        let mut statement = self.connection.prepare(sql)?;
        let names: Vec<String> = statement.column_names().map(|x| x.to_string()).collect();
        let mut rows = statement.query(args)?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            let mut object = serde_json::Map::new();
            for (index, name) in names.iter().enumerate() {
                let value = match row.get_ref(index)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(n) => {
                        if name == "watched" {
                            json!(n != 0)
                        } else {
                            json!(n)
                        }
                    }
                    ValueRef::Real(n) => json!(n),
                    ValueRef::Text(s) => json!(std::str::from_utf8(s)?),
                    ValueRef::Blob(_) => bail!("Unexpected binary database column"),
                };
                object.insert(name.clone(), value);
            }
            result.push(Value::Object(object));
        }
        Ok(result)
    }
    pub fn register(&self, id: &str, name: &str, platform: &str) -> Result<Value> {
        let id = uuid::Uuid::parse_str(id)
            .context("client_id must be a UUID")?
            .to_string();
        validate_name(name)?;
        if !["macOS", "iOS", "iPadOS", "tvOS", "CLI", "other"].contains(&platform) {
            bail!("Unknown client platform");
        }
        let time = now();
        self.connection.execute("INSERT INTO clients VALUES(?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET last_seen=excluded.last_seen,platform=excluded.platform", params![id,name.trim(),platform,time,time])?;
        self.rows("SELECT * FROM clients WHERE id=?", [&id])?
            .into_iter()
            .next()
            .context("Client not found")
    }
    pub fn rename_client(&self, id: &str, name: &str) -> Result<Value> {
        validate_name(name)?;
        if self.connection.execute(
            "UPDATE clients SET name=? WHERE id=?",
            params![name.trim(), id],
        )? == 0
        {
            bail!("Client not found");
        }
        self.rows("SELECT * FROM clients WHERE id=?", [id])?
            .into_iter()
            .next()
            .context("Client not found")
    }
    pub fn begin(
        &mut self,
        profile: &str,
        hash: &str,
        index: u64,
        name: &str,
        length: u64,
        client: &str,
    ) -> Result<Value> {
        self.require_profile(profile)?;
        let tx = self.connection.transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM clients WHERE id=?)",
            [client],
            |r| r.get(0),
        )?;
        if !exists {
            bail!("Register the client before starting playback");
        }
        tx.execute("DELETE FROM profile_hidden_titles WHERE profile_id=? AND title_id IN (SELECT json_extract(document,'$.entity_id') FROM library_assets WHERE json_extract(document,'$.torrent_hash')=? AND COALESCE(json_extract(document,'$.file_index'),0)=?)",params![profile,hash,i64::try_from(index)?])?;
        let media = format!("{hash}:{index}");
        let session = uuid::Uuid::new_v4().to_string();
        let time = now();
        tx.execute("INSERT INTO media VALUES(?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,length=excluded.length", params![media,hash,i64::try_from(index)?,name,i64::try_from(length)?])?;
        tx.execute(
            "INSERT OR IGNORE INTO profile_playback(profile_id,media_id) VALUES(?,?)",
            params![profile,media],
        )?;
        let resume: f64 = tx.query_row(
            "SELECT CASE WHEN watched THEN 0 ELSE position END FROM profile_playback WHERE profile_id=? AND media_id=?",
            params![profile,media],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO playback_sessions(id,media_id,client_id,started) VALUES(?,?,?,?)",
            params![session, media, client, time],
        )?;
        tx.execute("INSERT INTO session_profiles VALUES(?,?)", params![session,profile])?;
        tx.execute(
            "UPDATE profile_playback SET active_session=?,client_id=?,last_played=? WHERE profile_id=? AND media_id=?",
            params![session, client, time, profile, media],
        )?;
        tx.execute(
            "UPDATE clients SET last_seen=? WHERE id=?",
            params![time, client],
        )?;
        tx.execute("DELETE FROM playback_sessions WHERE started<? AND id NOT IN (SELECT active_session FROM profile_playback WHERE active_session IS NOT NULL)", [time-90.0*86400.0])?;
        tx.commit()?;
        Ok(json!({"session_id":session,"media_id":media,"profile_id":profile,"resume_position":resume}))
    }
    pub fn progress(
        &mut self,
        session: &str,
        sequence: i64,
        position: f64,
        duration: f64,
        ended: bool,
    ) -> Result<Value> {
        self.progress_with_evidence(session, sequence, position, duration, ended, true, false)
    }
    pub fn progress_with_evidence(&mut self, session: &str, sequence: i64, position: f64,
        duration: f64, ended: bool, watched_eligible: bool, reached_end: bool) -> Result<Value> {
        if sequence < 0
            || !position.is_finite()
            || !duration.is_finite()
            || position < 0.0
            || duration <= 0.0
            || position > duration + 2.0
        {
            bail!("Invalid playback progress");
        }
        let tx = self.connection.transaction()?;
        let record: Option<(String,String,i64,bool,Option<String>,bool,String)> = tx.query_row("SELECT s.media_id,s.client_id,s.sequence,s.ended,p.active_session,p.watched,sp.profile_id FROM playback_sessions s JOIN session_profiles sp ON sp.session_id=s.id JOIN profile_playback p ON p.media_id=s.media_id AND p.profile_id=sp.profile_id WHERE s.id=?", [session], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional()?;
        let (media, client, previous, finished, active, watched, profile) =
            record.context("Playback session not found")?;
        if active.as_deref() != Some(session) || sequence <= previous || finished {
            return Ok(json!({"accepted":false,"reason":"superseded_or_stale"}));
        }
        // Played (not scrubbed while paused) into the closing credits, or to the end.
        let watched = watched || reached_end || (watched_eligible
            && crate::profiles::finished(position, duration, crate::profiles::closing_credits(&tx, &media, duration)?));
        let time = now();
        tx.execute("UPDATE profile_playback SET position=?,duration=?,watched=?,last_played=?,client_id=? WHERE profile_id=? AND media_id=?", params![position.min(duration),duration,watched,time,client,profile,media])?;
        tx.execute(
            "UPDATE playback_sessions SET sequence=?,ended=? WHERE id=?",
            params![sequence, ended, session],
        )?;
        tx.execute(
            "UPDATE clients SET last_seen=? WHERE id=?",
            params![time, client],
        )?;
        tx.commit()?;
        Ok(json!({"accepted":true,"watched":watched}))
    }
    pub fn history(&self, profile: &str) -> Result<Vec<Value>> {
        self.require_profile(profile)?;
        self.rows("SELECT m.*,p.position,p.duration,p.watched,p.last_played,p.client_id,c.name AS client_name FROM profile_playback p JOIN media m ON m.id=p.media_id LEFT JOIN clients c ON c.id=p.client_id WHERE p.profile_id=? ORDER BY p.last_played DESC", [profile])
    }
    pub fn mark(
        &mut self,
        profile: &str,
        hash: &str,
        index: u64,
        name: &str,
        length: u64,
        watched: bool,
    ) -> Result<Value> {
        self.require_profile(profile)?;
        let media = format!("{hash}:{index}");
        let tx = self.connection.transaction()?;
        tx.execute("INSERT INTO media VALUES(?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,length=excluded.length", params![media,hash,i64::try_from(index)?,name,i64::try_from(length)?])?;
        tx.execute(
            "INSERT OR IGNORE INTO profile_playback(profile_id,media_id) VALUES(?,?)",
            params![profile,media],
        )?;
        tx.execute(
            "UPDATE profile_playback SET watched=?,position=0,active_session=NULL,last_played=? WHERE profile_id=? AND media_id=?",
            params![watched, now(), profile, media],
        )?;
        tx.commit()?;
        Ok(json!({"media_id":media,"watched":watched}))
    }
}
fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.chars().count() > 80 {
        bail!("Client name must contain 1–80 characters");
    }
    Ok(())
}

/// Add a column to an existing table once; new databases get it from here too.
pub(crate) fn add_column(connection: &Connection, table: &str, column: &str, kind: &str) -> Result<()> {
    let exists: bool = connection.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM pragma_table_info('{table}') WHERE name=?)"), [column], |r| r.get(0))?;
    if !exists { connection.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {kind}"), [])?; }
    Ok(())
}
