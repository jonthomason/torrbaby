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
        for asset in self.documents("assets")? {
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
                "id":id,"name":episode["title"].as_str().unwrap_or(if kind=="movie"{name}else{&fallback}),
                "season":asset["season"],"episode":asset["episode"],"summary":episode["summary"].as_str().unwrap_or(""),
                "airdate":episode["airdate"],"thumbnail":episode["thumbnail"],"duration_minutes":episode.get("duration_minutes").unwrap_or(&entity["duration_minutes"]),
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
