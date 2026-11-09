use crate::storage::Database;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;

impl Database {
    pub fn documents(&self, table: &str) -> Result<Vec<Value>> {
        let sql = match table {
            "assets" => "SELECT document FROM library_assets",
            "entities" => "SELECT document FROM library_entities",
            "metadata" => "SELECT document FROM metadata",
            _ => bail!("Unknown document collection"),
        };
        self.rows(sql, [])?
            .iter()
            .map(|r| {
                Ok(serde_json::from_str(
                    r["document"].as_str().context("Invalid stored document")?,
                )?)
            })
            .collect()
    }
    pub fn asset(&self, id: &str) -> Result<Value> {
        let rows = self.rows("SELECT document FROM library_assets WHERE id=?", [id])?;
        let row = rows.first().context("Media file not found")?;
        Ok(serde_json::from_str(
            row["document"].as_str().context("Invalid asset document")?,
        )?)
    }
    pub fn catalog(&self) -> Result<Value> {
        let entities: BTreeMap<String, Value> = self
            .documents("entities")?
            .into_iter()
            .filter_map(|v| v["id"].as_str().map(str::to_owned).map(|id| (id, v)))
            .collect();
        let history: BTreeMap<String, Value> = self
            .history()?
            .into_iter()
            .filter_map(|v| v["id"].as_str().map(str::to_owned).map(|id| (id, v)))
            .collect();
        let mut titles = BTreeMap::<String, Value>::new();
        let assets = self.documents("assets")?;
        for asset in &assets {
            if asset["available"] != true || asset["role"] == "extra" {
                continue;
            }
            let id = asset["id"].as_str().context("Invalid asset ID")?;
            let entity_id = asset["entity_id"].as_str().unwrap_or(id);
            let entity = entities.get(entity_id).cloned().unwrap_or(json!({}));
            let file = std::path::Path::new(asset["path"].as_str().unwrap_or(""));
            let fallback = file
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .replace(['.', '_'], " ");
            let kind = entity["kind"]
                .as_str()
                .unwrap_or(if asset["season"].is_number() {
                    "tv"
                } else {
                    "movie"
                });
            let name = entity["title"]
                .as_str()
                .or(asset["series_name"].as_str())
                .unwrap_or(&fallback);
            let title=titles.entry(entity_id.to_owned()).or_insert_with(||json!({
                "id":entity_id,"kind":kind,"title":name,"original_title":entity["original_title"],
                "year":entity["year"],"release_date":entity["release_date"],"genres":entity.get("genres").unwrap_or(&json!([])),
                "summary":entity["summary"].as_str().unwrap_or(""),"content_rating":entity["content_rating"],"duration_minutes":entity["duration_minutes"],
                "poster":entity["thumbnail"],"backdrop":entity["backdrop"],"logo":entity["logo"],"provider":entity["provider"],"source_url":entity["source_url"],
                "added_at":asset["added_at"],"status":asset["status"],"assets":[]
            }));
            if asset["added_at"].as_f64().unwrap_or(0.0) > title["added_at"].as_f64().unwrap_or(0.0)
            {
                title["added_at"] = asset["added_at"].clone();
            }
            let media = format!(
                "{}:{}",
                asset["torrent_hash"].as_str().unwrap_or(id),
                asset["file_index"].as_u64().unwrap_or(0)
            );
            let progress = history.get(&media).cloned().unwrap_or(json!({}));
            let episode = entity["episodes"]
                .as_array()
                .and_then(|eps| {
                    eps.iter().find(|e| {
                        !asset["season"].is_null()
                            && e["season"] == asset["season"]
                            && e["episode"] == asset["episode"]
                    })
                })
                .cloned()
                .unwrap_or(json!({}));
            title["assets"].as_array_mut().unwrap().push(json!({
                "id":id,"media_info":crate::media_inventory::cached(self,id)?,"role":"main","name":episode["title"].as_str().unwrap_or(if kind=="movie"{name}else{&fallback}),
                "season":asset["season"],"episode":asset["episode"],"summary":episode["summary"].as_str().unwrap_or(""),
                "airdate":episode["airdate"],"thumbnail":episode["thumbnail"],"duration_minutes":episode.get("duration_minutes").unwrap_or(&entity["duration_minutes"]),
                "complete":asset["complete"],"size":asset["size"],"position":progress["position"].as_f64().unwrap_or(0.0),
                "duration":progress["duration"].as_f64().unwrap_or(0.0),"watched":progress["watched"].as_bool().unwrap_or(false),
                "last_played":progress["last_played"].as_f64().unwrap_or(0.0),"torrent_hash":asset["torrent_hash"]
            }));
        }
        for asset in assets
            .iter()
            .filter(|a| a["available"] == true && a["role"] == "extra")
        {
            let Some(parent) = asset["parent_title_id"].as_str() else {
                continue;
            };
            let Some(title) = titles.get_mut(parent) else {
                continue;
            };
            let id = asset["id"].as_str().context("Invalid extra ID")?;
            let path = std::path::Path::new(asset["path"].as_str().unwrap_or(""));
            let name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .replace(['.', '_'], " ");
            let media = format!(
                "{}:{}",
                asset["torrent_hash"].as_str().unwrap_or(id),
                asset["file_index"].as_u64().unwrap_or(0)
            );
            let progress = history.get(&media).cloned().unwrap_or(json!({}));
            title["assets"].as_array_mut().unwrap().push(json!({
                "id":id,"role":"extra","name":name,"season":null,"episode":null,"summary":"","thumbnail":null,
                "complete":asset["complete"],"size":asset["size"],"position":progress["position"].as_f64().unwrap_or(0.0),
                "duration":progress["duration"].as_f64().unwrap_or(0.0),"watched":progress["watched"].as_bool().unwrap_or(false),
                "last_played":progress["last_played"].as_f64().unwrap_or(0.0),"torrent_hash":asset["torrent_hash"]
            }));
        }
        let mut titles: Vec<_> = titles.into_values().collect();
        titles.sort_by_key(|v| v["title"].as_str().unwrap_or("").to_lowercase());
        let state = self.state()?;
        let scan = &state["library_scan"];
        Ok(
            json!({"titles":titles,"last_scan":scan["last_scan"],"scanning":scan["scanning"].as_bool().unwrap_or(false),"error":scan["error"]}),
        )
    }
}

impl Database {
    /// Prefer shared torrent ownership, then a dedicated extras folder, when there
    /// is one parent title. Collections with multiple films stay unlinked.
    pub fn link_extras(&self) -> Result<()> {
        let assets = self.documents("assets")?;
        for extra in assets.iter().filter(|a| a["role"] == "extra") {
            let path = std::path::Path::new(extra["path"].as_str().unwrap_or(""));
            let anchor = path
                .ancestors()
                .skip(1)
                .find(|p| {
                    p.file_name().and_then(|s| s.to_str()).is_some_and(|s| {
                        [
                            "extras",
                            "featurettes",
                            "samples",
                            "sample",
                            "trailers",
                            "bonus",
                        ]
                        .contains(&s.to_ascii_lowercase().as_str())
                    })
                })
                .and_then(|p| p.parent());
            let mut parents = std::collections::BTreeSet::new();
            if let Some(hash) = extra["torrent_hash"].as_str() {
                for main in assets.iter().filter(|a| {
                    a["available"] == true && a["role"] != "extra" && a["torrent_hash"] == hash
                }) {
                    if let Some(id) = main["entity_id"].as_str().or(main["id"].as_str()) {
                        parents.insert(id.to_owned());
                    }
                }
            }
            if parents.is_empty() {
                if let Some(anchor) = anchor {
                    for main in assets
                        .iter()
                        .filter(|a| a["available"] == true && a["role"] != "extra")
                    {
                        if std::path::Path::new(main["path"].as_str().unwrap_or(""))
                            .starts_with(anchor)
                        {
                            if let Some(id) = main["entity_id"].as_str().or(main["id"].as_str()) {
                                parents.insert(id.to_owned());
                            }
                        }
                    }
                }
            }
            let parent = if parents.len() == 1 {
                json!(parents.into_iter().next())
            } else {
                Value::Null
            };
            if extra["parent_title_id"] != parent {
                let mut updated = extra.clone();
                updated["parent_title_id"] = parent;
                self.connection.execute(
                    "UPDATE library_assets SET document=? WHERE id=?",
                    rusqlite::params![
                        updated.to_string(),
                        extra["id"].as_str().context("Invalid extra ID")?
                    ],
                )?;
            }
        }
        Ok(())
    }
}
