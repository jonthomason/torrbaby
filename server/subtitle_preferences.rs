//! Per-device subtitle choices, retaining legacy household/title APIs for older clients.
use crate::storage::{Database, now};
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    enabled: bool,
    language: String,
    font: String,
    size_percent: u16,
    text_opacity: u8,
    background_opacity: u8,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            enabled: false,
            language: "original".into(),
            font: "default".into(),
            size_percent: 100,
            text_opacity: 100,
            background_opacity: 0,
        }
    }
}
impl Database {
    fn subtitle_scope(&self, title: Option<&str>) -> Result<String> {
        if let Some(title) = title {
            ensure!(
                self.connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM library_entities WHERE id=?)",
                    [title],
                    |r| r.get::<_, bool>(0)
                )?,
                "Unknown library title"
            );
            Ok(format!("title:{title}"))
        } else {
            Ok("default".into())
        }
    }
    pub fn device_subtitle_preferences(&self, client: &str, body: Option<Value>) -> Result<Value> {
        ensure!(!client.is_empty() && client.len() <= 256, "Invalid device ID");
        let scope = format!("device:{client}");
        if let Some(body) = body { self.save_subtitle_scope(&scope, body)?; }
        let raw: Option<String> = self.connection.query_row("SELECT document FROM subtitle_preferences WHERE scope=?", [&scope], |r| r.get(0)).optional()?;
        // Device choices do not inherit household or show overrides.
        let p: Preferences = raw.map(|s| serde_json::from_str(&s)).transpose()?.unwrap_or(Preferences { enabled: true, language: "en".into(), ..Preferences::default() });
        Ok(json!({"defaults":p,"effective":p,"overridden":false}))
    }
    pub fn subtitle_preferences(&self, title: Option<&str>) -> Result<Value> {
        let scope = self.subtitle_scope(title)?;
        let read = |key: &str| -> Result<Option<Preferences>> {
            let raw: Option<String> = self
                .connection
                .query_row(
                    "SELECT document FROM subtitle_preferences WHERE scope=?",
                    [key],
                    |r| r.get(0),
                )
                .optional()?;
            raw.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
        };
        let defaults = read("default")?.unwrap_or_default();
        let override_value = if title.is_some() { read(&scope)? } else { None };
        Ok(
            json!({"defaults": defaults, "effective": override_value.as_ref().unwrap_or(&defaults), "overridden": override_value.is_some()}),
        )
    }
    pub fn save_subtitle_preferences(&self, title: Option<&str>, body: Value) -> Result<Value> {
        let scope = self.subtitle_scope(title)?;
        self.save_subtitle_scope(&scope, body)?;
        self.subtitle_preferences(title)
    }
    fn save_subtitle_scope(&self, scope: &str, body: Value) -> Result<()> {
        let p: Preferences = serde_json::from_value(body)?;
        ensure!(
            ["original", "en", "zh-Hans"].contains(&p.language.as_str()),
            "Unsupported subtitle language"
        );
        ensure!(
            ["default", "sans-serif", "serif", "monospace"].contains(&p.font.as_str()),
            "Unsupported subtitle font"
        );
        ensure!(
            (50..=200).contains(&p.size_percent)
                && p.text_opacity <= 100
                && p.background_opacity <= 100,
            "Invalid subtitle style"
        );
        self.connection.execute("INSERT INTO subtitle_preferences(scope,document,updated) VALUES(?,?,?) ON CONFLICT(scope) DO UPDATE SET document=excluded.document,updated=excluded.updated", params![scope,serde_json::to_string(&p)?,now()])?;
        Ok(())
    }
    pub fn reset_subtitle_preferences(&self, title: &str) -> Result<Value> {
        let scope = self.subtitle_scope(Some(title))?;
        self.connection
            .execute("DELETE FROM subtitle_preferences WHERE scope=?", [scope])?;
        self.subtitle_preferences(Some(title))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_title_override_reset_and_persistence() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("torrbaby-preferences-{}", uuid::Uuid::new_v4()));
        let path = root.join("library.sqlite3");
        let db = Database::open(&path, None)?;
        db.connection
            .execute("INSERT INTO library_entities VALUES('title-test','{}')", [])?;
        let title = Some("title-test");
        assert_eq!(
            db.subtitle_preferences(title)?["effective"]["enabled"],
            false
        );
        let mut p = serde_json::to_value(Preferences::default())?;
        p["enabled"] = json!(true);
        p["language"] = json!("zh-Hans");
        db.save_subtitle_preferences(None, p.clone())?;
        assert_eq!(
            db.subtitle_preferences(title)?["effective"]["language"],
            "zh-Hans"
        );
        p["language"] = json!("en");
        db.save_subtitle_preferences(title, p.clone())?;
        p["size_percent"] = json!(999);
        assert!(db.save_subtitle_preferences(title, p).is_err());
        drop(db);
        let db = Database::open(&path, None)?;
        assert_eq!(
            db.subtitle_preferences(title)?["effective"]["language"],
            "en"
        );
        assert_eq!(
            db.reset_subtitle_preferences("title-test")?["effective"]["language"],
            "zh-Hans"
        );
        assert!(db.subtitle_preferences(Some("missing")).is_err());
        assert_eq!(db.device_subtitle_preferences("tv", None)?["effective"]["language"], "en");
        let mut device = serde_json::to_value(Preferences::default())?;
        device["language"] = json!("zh-Hans");
        device["enabled"] = json!(true);
        db.device_subtitle_preferences("tv", Some(device.clone()))?;
        assert_eq!(db.device_subtitle_preferences("tablet", None)?["effective"]["language"], "en");
        device["size_percent"] = json!(999);
        assert!(db.device_subtitle_preferences("tv", Some(device)).is_err());
        drop(db);
        let db = Database::open(&path, None)?;
        assert_eq!(db.device_subtitle_preferences("tv", None)?["effective"]["language"], "zh-Hans");
        drop(db);
        fs_cleanup(&root)?;
        Ok(())
    }
    fn fs_cleanup(path: &std::path::Path) -> Result<()> {
        std::fs::remove_dir_all(path)?;
        Ok(())
    }
}
