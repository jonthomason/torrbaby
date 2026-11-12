//! Read-only report of what a library scan would record and what the organizer would
//! move, for comparing two server builds on the same library (docs/disk-aggregation.md,
//! phase acceptance). Writes nothing to the library or to Transmission; open a copy of
//! the database, never the live one.
use crate::{config::Config, storage::Database, transmission::Transmission};
use anyhow::Result;
use serde_json::{Value, json};
use std::path::Path;

/// `torrents`: a saved Transmission snapshot to use instead of asking Transmission, so two
/// builds compare against the same torrents. `save_torrents`: where to write the snapshot.
pub async fn run(config: &Path, database: &Path, torrents: Option<&Path>, save_torrents: Option<&Path>) -> Result<Value> {
    let config = Config::load(config)?;
    let db = Database::open(database, None)?;
    // The same settings the running server applies over the file.
    let config = config.with_settings(&db.state()?["settings"])?;
    let snapshot = match torrents {
        Some(path) => serde_json::from_slice(&std::fs::read(path)?)?,
        None => {
            let rpc = Transmission::new(&config)?;
            let torrents = rpc.call("torrent_get", json!({"fields":crate::transmission::FIELDS})).await?;
            let session = rpc.call("session_get", json!({"fields":["incomplete_dir","incomplete_dir_enabled"]})).await?;
            json!({"torrents":torrents["torrents"],"session":session})
        }
    };
    if let Some(path) = save_torrents { std::fs::write(path, serde_json::to_vec(&snapshot)?)?; }
    let torrents = snapshot["torrents"].as_array().cloned().unwrap_or_default();
    let incomplete = Some(&snapshot["session"]).filter(|s| s["incomplete_dir_enabled"] == true)
        .and_then(|s| s["incomplete_dir"].as_str().map(std::path::PathBuf::from));
    let old = db.documents("assets")?;
    let known: std::collections::HashSet<String> = old.iter().filter_map(|a| a["id"].as_str().map(str::to_owned)).collect();
    let inventory = crate::scanner::inventory_with(&config, old, torrents.clone(), incomplete.as_deref());
    let mut assets: Vec<Value> = inventory.assets.into_iter().map(|mut asset| {
        // Times and the ids of newly found files differ on every run.
        for key in ["seen_at", "added_at"] { asset.as_object_mut().map(|a| a.remove(key)); }
        if !asset["id"].as_str().is_some_and(|id| known.contains(id)) {
            asset["id"] = json!("new");
            asset.as_object_mut().map(|a| a.remove("ready_at"));
        }
        asset
    }).collect();
    assets.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()).then(a["id"].as_str().cmp(&b["id"].as_str())));
    let mut plans: Vec<Value> = torrents.iter().map(|t| {
        let plan = match crate::organizer::plan(&config, t) {
            Ok(Some(mut plan)) => { plan.as_object_mut().map(|p| p.remove("created")); plan }
            Ok(None) => Value::Null,
            Err(error) => json!({"error": error.to_string()}),
        };
        json!({"hash":t["hash_string"],"download_dir":t["download_dir"],"plan":plan})
    }).collect();
    plans.sort_by(|a, b| a["hash"].as_str().cmp(&b["hash"].as_str()));
    let mut superseded = inventory.superseded;
    superseded.sort();
    Ok(json!({"assets":assets,"errors":inventory.errors,"superseded":superseded,"plans":plans}))
}
