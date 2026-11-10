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

/// Shared online/offline completion rule. Never infer credits from a percentage.
pub fn completed(position: f64, duration: f64) -> bool {
    duration.is_finite() && position.is_finite() && duration > 0.0
        && position >= duration
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
    fn completion_requires_the_actual_end() {
        assert!(!completed(7200.0, 8100.0));
        assert!(!completed(7500.0, 8100.0));
        assert!(!completed(2300.0, 2700.0));
        assert!(!completed(2430.0, 2700.0));
        assert!(!completed(2699.0, 2700.0));
        assert!(completed(2700.0, 2700.0));
        assert!(!completed(f64::NAN,2700.0));
    }
}
