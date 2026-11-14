//! People own viewing state; device credentials only identify the requesting device.
use crate::storage::{Database, now};
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
pub const LEGACY_PROFILE: &str = "profile-jon";

impl Database {
    pub fn migrate_profiles(&mut self, previous_version: i64) -> Result<()> {
        let tx = self.connection.transaction()?;
        tx.execute_batch(include_str!("profiles_schema.sql"))?;
        if previous_version < 10 {
            tx.execute("INSERT OR IGNORE INTO profile_playback SELECT ?,media_id,position,duration,watched,last_played,client_id,active_session FROM playback", [LEGACY_PROFILE])?;
            tx.execute("INSERT OR IGNORE INTO session_profiles SELECT id,? FROM playback_sessions", [LEGACY_PROFILE])?;
            tx.execute("INSERT OR IGNORE INTO device_profiles SELECT id,? FROM clients", [LEGACY_PROFILE])?;
        }
        tx.pragma_update(None, "user_version", 10)?;
        tx.commit()?;
        Ok(())
    }
    pub fn require_profile(&self, id: &str) -> Result<()> {
        ensure!(self.connection.query_row("SELECT EXISTS(SELECT 1 FROM profiles WHERE id=?)", [id], |r|r.get::<_,bool>(0))?, "Profile not found");
        Ok(())
    }
    pub fn profiles(&self) -> Result<Vec<Value>> {
        self.rows("SELECT p.id,p.name,p.created,COALESCE(a.avatar_symbol,'') AS avatar_symbol,COALESCE(a.text_color,'#70E8C2') AS text_color,COALESCE(a.background_color,'#243A38') AS background_color FROM profiles p LEFT JOIN profile_appearance a ON a.profile_id=p.id ORDER BY p.created,p.id", [])
    }
    pub fn create_profile(&self, name: &str) -> Result<Value> {
        let name = profile_name(name)?;
        let id = format!("profile-{}", uuid::Uuid::new_v4());
        self.connection.execute("INSERT INTO profiles VALUES(?,?,?)", params![id,name,now()])?;
        Ok(json!({"id":id,"name":name}))
    }
    pub fn rename_profile(&self, id: &str, name: &str) -> Result<Value> {
        self.require_profile(id)?;
        let name = profile_name(name)?;
        self.connection.execute("UPDATE profiles SET name=? WHERE id=?", params![name,id])?;
        Ok(json!({"id":id,"name":name}))
    }
    /// Name and appearance are one atomic edit; old clients can still rename only.
    pub fn write_profile(&self, existing: Option<&str>, body: &Value) -> Result<Value> {
        let name = profile_name(body["name"].as_str().unwrap_or(""))?;
        let previous = if let Some(id) = existing {
            self.require_profile(id)?;
            self.profiles()?.into_iter().find(|p| p["id"] == id).unwrap_or(Value::Null)
        } else { Value::Null };
        let symbol = body.get("avatar_symbol").or_else(||previous.get("avatar_symbol")).and_then(Value::as_str).unwrap_or("");
        ensure!(symbol.chars().count() <= 32 && !symbol.chars().any(char::is_control), "Avatar must be an initial, emoji or printable symbol");
        let foreground = body.get("text_color").or_else(||previous.get("text_color")).and_then(Value::as_str).unwrap_or("#70E8C2");
        let background = body.get("background_color").or_else(||previous.get("background_color")).and_then(Value::as_str).unwrap_or("#243A38");
        for color in [foreground, background] {
            ensure!(color.len() == 7 && color.starts_with('#') && color[1..].bytes().all(|c|c.is_ascii_hexdigit()), "Profile colors must use #RRGGBB");
        }
        let id = existing.map(str::to_owned).unwrap_or_else(||format!("profile-{}",uuid::Uuid::new_v4()));
        let tx = self.connection.unchecked_transaction()?;
        if existing.is_some() { tx.execute("UPDATE profiles SET name=? WHERE id=?",params![name,id])?; }
        else { tx.execute("INSERT INTO profiles(id,name,created) VALUES(?,?,?)",params![id,name,now()])?; }
        tx.execute("INSERT INTO profile_appearance VALUES(?,?,?,?) ON CONFLICT(profile_id) DO UPDATE SET avatar_symbol=excluded.avatar_symbol,text_color=excluded.text_color,background_color=excluded.background_color",params![id,symbol,foreground,background])?;
        tx.commit()?;
        Ok(json!({"id":id,"name":name,"avatar_symbol":symbol,"text_color":foreground,"background_color":background}))
    }
    pub fn device_profile(&self, client: &str) -> Result<Value> {
        ensure!(self.connection.query_row("SELECT EXISTS(SELECT 1 FROM clients WHERE id=?)", [client], |r|r.get::<_,bool>(0))?, "Device not found");
        let id: Option<String> = self.connection.query_row("SELECT profile_id FROM device_profiles WHERE client_id=?", [client], |r|r.get(0)).optional()?;
        Ok(json!({"profile_id":id,"profiles":self.profiles()?}))
    }
    pub fn select_profile(&self, client: &str, profile: &str) -> Result<Value> {
        self.device_profile(client)?;
        self.require_profile(profile)?;
        self.connection.execute("INSERT INTO device_profiles VALUES(?,?) ON CONFLICT(client_id) DO UPDATE SET profile_id=excluded.profile_id", params![client,profile])?;
        self.device_profile(client)
    }
}
fn profile_name(name: &str) -> Result<&str> {
    let name = name.trim();
    ensure!(!name.is_empty() && name.chars().count() <= 80 && !name.chars().any(char::is_control), "Profile name must contain 1–80 printable characters");
    Ok(name)
}

/// Without detected credits, this close to the end counts as finished: at most two
/// minutes, and at most 5% of a short file. Deliberately far short of the old 90% /
/// ten-minute rule, which marked Bone Lake watched 8m49s before its end.
pub const FINISH_WITHOUT_CREDITS: f64 = 120.0;
const FINISH_WITHOUT_CREDITS_SHARE: f64 = 0.05;

/// Shared online/offline completion rule (the Apple and Android clients use the same):
/// the end of the file, or the start of the closing credits Diamond detected for it
/// (`credits`; only credits in the second half count), or, without detected credits,
/// the last moments of the file. Callers decide whether a report is evidence of watching.
pub fn finished(position: f64, duration: f64, credits: Option<f64>) -> bool {
    if !(duration.is_finite() && position.is_finite() && duration > 0.0) { return false; }
    if position >= duration { return true; }
    match credits.filter(|start| start.is_finite() && *start >= duration * 0.5 && *start < duration) {
        Some(start) => position >= start,
        None => duration - position <= FINISH_WITHOUT_CREDITS.min(duration * FINISH_WITHOUT_CREDITS_SHARE),
    }
}

/// The closing credits Diamond detected in the file behind one playback record
/// (`media` is "hash:index", or "asset-id:index" for a file without a torrent), if the
/// analysis is of a file this long: a replaced file's old analysis does not count.
pub fn closing_credits(connection: &rusqlite::Connection, media: &str, duration: f64) -> Result<Option<f64>> {
    let documents: Vec<String> = connection.prepare_cached(
        "SELECT c.document FROM media m JOIN library_assets a ON (json_extract(a.document,'$.torrent_hash')=m.torrent_hash AND COALESCE(json_extract(a.document,'$.file_index'),0)=m.file_index) OR a.id=m.torrent_hash JOIN credits_analysis c ON c.asset_id=a.id WHERE m.id=?")?
        .query_map([media], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(documents.iter().filter_map(|d| serde_json::from_str::<Value>(d).ok()).find_map(|d| crate::credits::closing_start(&d, duration)))
}

impl Database {
    /// Playback stopped in the closing credits before this rule existed stayed in Continue
    /// Watching ("4m left"). Marks those records watched once; positions and times are kept.
    /// Returns how many were repaired.
    pub fn repair_finished_playback(&mut self) -> Result<usize> {
        const REPAIR: &str = "closing-credits-completion-1";
        self.connection.execute("CREATE TABLE IF NOT EXISTS data_repairs (name TEXT PRIMARY KEY, applied REAL NOT NULL)", [])?;
        let done: bool = self.connection.query_row("SELECT EXISTS(SELECT 1 FROM data_repairs WHERE name=?)", [REPAIR], |r| r.get(0))?;
        if done { return Ok(0); }
        let rows: Vec<(String, String, f64, f64)> = self.connection
            .prepare("SELECT profile_id,media_id,position,duration FROM profile_playback WHERE watched=0 AND position>0 AND duration>0")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
        let mut finished_rows = Vec::new();
        for (profile, media, position, duration) in rows {
            if finished(position, duration, closing_credits(&self.connection, &media, duration)?) { finished_rows.push((profile, media)); }
        }
        let tx = self.connection.transaction()?;
        for (profile, media) in &finished_rows {
            tx.execute("UPDATE profile_playback SET watched=1 WHERE profile_id=? AND media_id=? AND watched=0", params![profile, media])?;
        }
        tx.execute("INSERT INTO data_repairs VALUES(?,?)", params![REPAIR, now()])?;
        tx.commit()?;
        Ok(finished_rows.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn appearance_edits_preserve_identity_and_reject_invalid_changes_atomically() -> Result<()> {
        let mut db = Database { connection: rusqlite::Connection::open_in_memory()? };
        db.connection.execute_batch(include_str!("schema.sql"))?;
        db.migrate_profiles(9)?;
        let p=db.write_profile(None,&json!({"name":"Alex","avatar_symbol":"👨‍👩‍👧","text_color":"#FFFFFF","background_color":"#345678"}))?;
        let id=p["id"].as_str().unwrap();
        let renamed=db.write_profile(Some(id),&json!({"name":"Alexis"}))?;
        assert_eq!(renamed["avatar_symbol"],p["avatar_symbol"]);
        assert_eq!(renamed["background_color"],"#345678");
        assert!(db.write_profile(Some(id),&json!({"name":"Broken","text_color":"red"})).is_err());
        let saved=db.profiles()?.into_iter().find(|p|p["id"]==id).unwrap();
        assert_eq!(saved["name"],"Alexis");
        assert_eq!(db.profiles()?.len(),2);
        Ok(())
    }
    #[test]
    fn migration_preserves_history_and_is_idempotent() -> Result<()> {
        let mut db = Database { connection: rusqlite::Connection::open_in_memory()? };
        db.connection.execute_batch(include_str!("schema.sql"))?;
        let client = uuid::Uuid::new_v4().to_string();
        db.register(&client, "iPad", "iPadOS")?;
        db.connection.execute("INSERT INTO media VALUES('m','hash',0,'Movie',100)", [])?;
        db.connection.execute("INSERT INTO playback(media_id,position,duration,watched,last_played,client_id) VALUES('m',7000,7500,1,123,?)", [&client])?;
        db.migrate_profiles(9)?;
        let guest = db.create_profile("Alex")?;
        db.select_profile(&client, guest["id"].as_str().unwrap())?;
        db.migrate_profiles(10)?;
        assert_eq!(db.device_profile(&client)?["profile_id"], guest["id"]);
        let row = db.rows("SELECT * FROM profile_playback", [])?;
        assert_eq!(row.len(),1);
        assert_eq!(row[0]["profile_id"],LEGACY_PROFILE);
        assert_eq!(row[0]["position"],7000.0);
        assert_eq!(row[0]["watched"],true);
        let fresh = uuid::Uuid::new_v4().to_string();
        db.register(&fresh,"TV","other")?;
        assert!(db.device_profile(&fresh)?["profile_id"].is_null());
        assert!(db.select_profile(&fresh,"missing").is_err());
        assert!(db.create_profile("  ").is_err());
        Ok(())
    }
    #[test]
    fn reopening_database_keeps_version_binding_and_backup() -> Result<()> {
        let root = std::env::temp_dir().join(format!("torrbaby-profiles-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("state.sqlite3");
        let client = uuid::Uuid::new_v4().to_string();
        {
            let old = rusqlite::Connection::open(&path)?;
            old.execute_batch(include_str!("schema.sql"))?;
            old.execute("INSERT INTO clients VALUES(?,'TV','other',1,1)", [&client])?;
            old.pragma_update(None,"user_version",9)?;
        }
        let profile;
        {
            let db = Database::open(&path,None)?;
            assert_eq!(db.device_profile(&client)?["profile_id"],LEGACY_PROFILE);
            profile = db.create_profile("Someone else")?["id"].as_str().unwrap().to_owned();
            db.select_profile(&client,&profile)?;
        }
        {
            let db = Database::open(&path,None)?;
            assert_eq!(db.device_profile(&client)?["profile_id"],profile);
            assert_eq!(db.connection.query_row("PRAGMA user_version",[],|r|r.get::<_,i64>(0))?,10);
        }
        assert_eq!(std::fs::read_dir(&root)?.filter_map(Result::ok)
            .filter(|e|e.file_name().to_string_lossy().starts_with("state-before-profiles-")).count(),1);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    fn fixture() -> Result<(Database,String,String)> {
        let mut db = Database { connection: rusqlite::Connection::open_in_memory()? };
        db.connection.execute_batch(include_str!("schema.sql"))?;
        db.migrate_profiles(9)?;
        db.save_state(&json!({}))?;
        let client = uuid::Uuid::new_v4().to_string();
        db.register(&client,"iPad","iPadOS")?;
        let second = db.create_profile("Alex")?["id"].as_str().unwrap().to_owned();
        db.connection.execute("INSERT INTO library_assets VALUES('asset-test',?)", [json!({
            "id":"asset-test","path":"/Movies/Test.mp4","torrent_hash":"hash",
            "file_index":0,"size":100,"complete":true,"available":true,"status":"matched","entity_id":"test-title"
        }).to_string()])?;
        db.connection.execute("INSERT INTO library_entities VALUES(?,?)",params!["test-title",json!({"id":"test-title","kind":"movie","title":"Test"}).to_string()])?;
        Ok((db,client,second))
    }
    #[test]
    fn manual_unwatched_is_profile_local_and_rejects_old_offline_updates() -> Result<()> {
        let (mut db, client, second) = fixture()?;
        db.mark(LEGACY_PROFILE,"hash",0,"Test",100,true)?;
        db.mark(&second,"hash",0,"Test",100,true)?;
        let recorded = now()-10.0;
        db.mark(&second,"hash",0,"Test",100,false)?;
        let result=crate::offline::sync(&mut db,json!({"event_id":uuid::Uuid::new_v4().to_string(),
            "client_id":client,"asset_id":"asset-test","profile_id":second,"position":1000.0,
            "duration":1000.0,"recorded_at":recorded,"reached_end":true}))?;
        assert_eq!(result["accepted"],false);
        assert_eq!(db.history(&second)?[0]["watched"],false);
        assert_eq!(db.history(LEGACY_PROFILE)?[0]["watched"],true);
        Ok(())
    }

    #[test]
    fn completion_is_the_closing_credits_or_the_last_moments() {
        // Weekend at Bernie's: credits from 5686.169 of 5904.169.
        let credits = Some(5686.169);
        assert!(!finished(5600.0, 5904.169, credits));
        assert!(finished(5686.169, 5904.169, credits));
        assert!(finished(5700.0, 5904.169, credits));
        assert!(finished(5904.169, 5904.169, credits));
        // Credits in the first half are not closing credits: the fallback applies.
        assert!(!finished(5700.0, 5904.169, Some(1000.0)));
        assert!(finished(5790.0, 5904.169, Some(1000.0)));
        // No credits: two minutes at most (Bone Lake, 8m49s before its end, is not finished).
        assert!(!finished(5138.563, 5667.412, None));
        assert!(!finished(7500.0, 8100.0, None));
        assert!(!finished(7979.0, 8100.0, None));
        assert!(finished(7980.0, 8100.0, None));
        // A short file: 5% of it.
        assert!(!finished(1300.0, 1369.0, None));
        assert!(finished(1302.0, 1369.0, None));
        assert!(finished(2700.0, 2700.0, None));
        assert!(!finished(f64::NAN, 2700.0, None));
        assert!(!finished(10.0, 0.0, None));
    }

    #[test]
    fn closing_credits_are_the_final_run_of_segments() {
        use crate::credits::closing_start;
        // Chamber of Secrets: cast cards, a minute without detection, then the crawl.
        let chamber = json!({"state":"ready","duration":10464.041,"segments":[
            {"start":9922.037,"end":10022.012},{"start":10082.03,"end":10430.041}]});
        assert_eq!(closing_start(&chamber, 10464.085), Some(9922.037));
        // A run of on-screen text well before the credits is not part of them.
        let apart = json!({"state":"ready","duration":5904.169,"segments":[
            {"start":4000.0,"end":4030.0},{"start":5686.169,"end":5861.126}]});
        assert_eq!(closing_start(&apart, 5904.169), Some(5686.169));
        // Nothing in the first half, nothing from a different file, nothing unfinished.
        let early = json!({"state":"ready","duration":5904.0,"segments":[{"start":2700.0,"end":2750.0},{"start":2800.0,"end":2900.0}]});
        assert_eq!(closing_start(&early, 5904.0), None);
        assert_eq!(closing_start(&apart, 5800.0), None);
        assert_eq!(closing_start(&json!({"state":"pending","segments":[]}), 5904.169), None);
        let half = json!({"state":"ready","segments":[{"start":2800.0,"end":2990.0},{"start":3000.0,"end":5800.0}]});
        assert_eq!(closing_start(&half, 5904.0), Some(3000.0));
    }

    fn credits_fixture(db: &Database, duration: f64, start: f64) -> Result<()> {
        db.connection.execute("INSERT INTO credits_analysis VALUES('asset-test','r',2,?,0)", [json!({
            "state":"ready","duration":duration,"segments":[{"start":start,"end":duration-5.0,"confidence":0.9}]
        }).to_string()])?;
        Ok(())
    }
    fn session(db: &mut Database, profile: &str, client: &str) -> Result<String> {
        Ok(db.begin(profile, "hash", 0, "Test", 100, client)?["session_id"].as_str().unwrap().to_owned())
    }

    #[test]
    fn stopping_in_the_closing_credits_finishes_a_movie() -> Result<()> {
        let (mut db, client, _) = fixture()?;
        credits_fixture(&db, 5904.169, 5686.169)?;
        let s = session(&mut db, LEGACY_PROFILE, &client)?;
        // Paused reports (scrubbing) are not evidence of watching, even in the credits.
        db.progress_with_evidence(&s, 1, 5000.0, 5904.169, false, true, false)?;
        assert_eq!(db.progress_with_evidence(&s, 2, 5700.0, 5904.169, false, false, false)?["watched"], false);
        assert_eq!(db.history(LEGACY_PROFILE)?[0]["watched"], false);
        // Playing in the credits is, and the movie leaves Continue Watching.
        assert_eq!(db.progress_with_evidence(&s, 3, 5705.0, 5904.169, false, true, false)?["watched"], true);
        db.progress_with_evidence(&s, 4, 5710.0, 5904.169, true, false, false)?;
        assert_eq!(db.history(LEGACY_PROFILE)?[0]["watched"], true);
        assert_eq!(db.begin(LEGACY_PROFILE, "hash", 0, "Test", 100, &client)?["resume_position"], 0.0);
        Ok(())
    }

    #[test]
    fn credits_of_another_file_are_ignored() -> Result<()> {
        let (mut db, client, _) = fixture()?;
        // The analysis was of a 3000 s file; this one is 5904 s long.
        credits_fixture(&db, 3000.0, 2800.0)?;
        let s = session(&mut db, LEGACY_PROFILE, &client)?;
        assert_eq!(db.progress_with_evidence(&s, 1, 5700.0, 5904.169, false, true, false)?["watched"], false);
        Ok(())
    }

    #[test]
    fn offline_reports_finish_in_the_credits() -> Result<()> {
        let (mut db, client, second) = fixture()?;
        credits_fixture(&db, 5904.169, 5686.169)?;
        let report = |position: f64, eligible: bool, ago: f64| json!({"event_id":uuid::Uuid::new_v4().to_string(),
            "client_id":client,"asset_id":"asset-test","profile_id":second,"position":position,
            "duration":5904.169,"recorded_at":now()-ago,"reached_end":false,"watched_eligible":eligible});
        crate::offline::sync(&mut db, report(5700.0, false, 20.0))?;
        assert_eq!(db.history(&second)?[0]["watched"], false);
        crate::offline::sync(&mut db, report(5700.0, true, 10.0))?;
        assert_eq!(db.history(&second)?[0]["watched"], true);
        Ok(())
    }

    #[test]
    fn start_repairs_playback_left_in_the_credits_once() -> Result<()> {
        let (mut db, client, second) = fixture()?;
        credits_fixture(&db, 5904.169, 5686.169)?;
        let s = session(&mut db, LEGACY_PROFILE, &client)?;
        db.progress_with_evidence(&s, 1, 5700.0, 5904.169, true, false, false)?;
        let other = session(&mut db, &second, &client)?;
        db.progress_with_evidence(&other, 1, 3000.0, 5904.169, true, false, false)?;
        assert_eq!(db.history(LEGACY_PROFILE)?[0]["watched"], false);
        assert_eq!(db.repair_finished_playback()?, 1);
        assert_eq!(db.history(LEGACY_PROFILE)?[0]["watched"], true);
        assert_eq!(db.history(LEGACY_PROFILE)?[0]["position"], 5700.0);
        assert_eq!(db.history(&second)?[0]["watched"], false);
        // Once only: what clients and the user do afterwards stands.
        db.mark(LEGACY_PROFILE, "hash", 0, "Test", 100, false)?;
        db.connection.execute("UPDATE profile_playback SET position=5700 WHERE profile_id=?", [LEGACY_PROFILE])?;
        assert_eq!(db.repair_finished_playback()?, 0);
        assert_eq!(db.history(LEGACY_PROFILE)?[0]["watched"], false);
        Ok(())
    }
}
