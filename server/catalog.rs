use crate::storage::Database;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A title's (start) year, never blank when any source knows it: the provider's
/// year, its release/first-air date, the earliest episode air date, then a year
/// in the file or folder name.
fn title_year(entity: &Value, file: &std::path::Path) -> Value {
    let four = |text: &str| text.get(..4).filter(|y| y.len() == 4 && y.chars().all(|c| c.is_ascii_digit())).map(str::to_owned);
    let known = entity["year"].as_str().and_then(four)
        .or_else(|| entity["release_date"].as_str().and_then(four))
        .or_else(|| entity["first_air_date"].as_str().and_then(four))
        .or_else(|| entity["episodes"].as_array().and_then(|episodes| episodes.iter()
            .filter_map(|e| e["airdate"].as_str().and_then(four)).min()))
        .or_else(|| file.ancestors().filter_map(|p| p.file_name()?.to_str()).take(3).find_map(|name| {
            static YEAR: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?:^|[^0-9])((?:19|20)[0-9]{2})(?:[^0-9]|$)").expect("valid regex"));
            YEAR.captures(name).map(|c| c[1].to_owned())
        }));
    known.map_or(Value::Null, |year| json!(year))
}
/// Whether a series is still running; null when the provider's status is not known.
fn series_ongoing(entity: &Value) -> Value {
    match entity["series_status"].as_str() {
        None | Some("") => Value::Null,
        Some("Ended") | Some("Canceled") => json!(false),
        Some(_) => json!(true),
    }
}
/// The year a finished series last aired; null while it continues or when unknown.
fn series_end_year(entity: &Value) -> Value {
    if series_ongoing(entity) != json!(false) { return Value::Null; }
    entity["last_air_date"].as_str().and_then(|date| date.get(..4)).filter(|year| year.len() == 4 && year.chars().all(|c| c.is_ascii_digit())).map_or(Value::Null, |year| json!(year))
}

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
    /// Durable invalidation token: unrelated transfer polling and device heartbeats do not change it.
    pub fn initialize_catalog_revision(&self) -> Result<()> {
        self.connection.execute_batch("CREATE TABLE IF NOT EXISTS catalog_revision (id INTEGER PRIMARY KEY CHECK(id=1), epoch TEXT NOT NULL, revision INTEGER NOT NULL); INSERT OR IGNORE INTO catalog_revision VALUES(1, lower(hex(randomblob(16))), 0);")?;
        for (table, condition) in [
            ("library_assets", "OLD.document IS NOT NEW.document"),
            ("library_entities", "OLD.document IS NOT NEW.document"),
            ("media_inventory", "OLD.document IS NOT NEW.document"),
            ("profile_hidden_titles", "1"), ("profile_playback", "1"), ("playback", "1"), ("media", "1"),
            ("library_operations", "OLD.package_id IS NOT NEW.package_id"),
            ("library_operation_assets", "1"),
        ] {
            for event in ["INSERT", "UPDATE", "DELETE"] {
                let when = if event == "UPDATE" { format!(" WHEN {condition}") } else { String::new() };
                self.connection.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS catalog_revision_{table}_{event} AFTER {event} ON {table}{when} BEGIN UPDATE catalog_revision SET revision=revision+1 WHERE id=1; END;"))?;
            }
        }
        self.connection.execute_batch("CREATE TRIGGER IF NOT EXISTS catalog_revision_scan AFTER UPDATE ON organizer_state WHEN json_extract(OLD.document,'$.library_scan') IS NOT json_extract(NEW.document,'$.library_scan') BEGIN UPDATE catalog_revision SET revision=revision+1 WHERE id=1; END;")?;
        Ok(())
    }

    pub fn catalog_revision(&self) -> Result<String> {
        Ok(self.connection.query_row("SELECT epoch || ':' || revision FROM catalog_revision WHERE id=1", [], |r|r.get(0))?)
    }

    pub fn catalog(&self, profile: &str) -> Result<Value> {
        let entities: BTreeMap<String, Value> = self
            .documents("entities")?
            .into_iter()
            .filter_map(|v| v["id"].as_str().map(str::to_owned).map(|id| (id, v)))
            .collect();
        let history: BTreeMap<String, Value> = self
            .history(profile)?
            .into_iter()
            .filter_map(|v| v["id"].as_str().map(str::to_owned).map(|id| (id, v)))
            .collect();
        let hidden: std::collections::HashSet<String> = self.rows("SELECT title_id FROM profile_hidden_titles WHERE profile_id=?",[profile])?.iter().filter_map(|v|v["title_id"].as_str().map(str::to_owned)).collect();
        let mut titles = BTreeMap::<String, Value>::new();
        let assets = self.visible_assets()?;
        let listed = |asset: &Value| !(asset["available"] != true || asset["complete"] != true || asset["role"] == "extra"
            || asset["library_ready"] == false || asset["status"] != "matched" || !asset["entity_id"].as_str().is_some_and(|id|entities.contains_key(id)));
        let package = |asset: &Value| asset["torrent_hash"].as_str().or(asset["id"].as_str()).unwrap_or("").to_owned();
        // A movie downloaded more than once is listed once per download, so each copy
        // can be played and removed on its own. The earliest download keeps the movie's
        // own title ID, and with it existing progress and hide choices.
        let mut first_package = BTreeMap::<String, (f64, String)>::new();
        for asset in assets.iter().filter(|a| listed(a)) {
            let candidate = (asset["added_at"].as_f64().unwrap_or(0.0), package(asset));
            let entry = first_package.entry(asset["entity_id"].as_str().unwrap_or("").to_owned()).or_insert(candidate.clone());
            if candidate.0 < entry.0 || (candidate.0 == entry.0 && candidate.1 < entry.1) { *entry = candidate; }
        }
        for asset in &assets {
            if !listed(asset) {
                continue;
            }
            let id = asset["id"].as_str().context("Invalid asset ID")?;
            let movie_id = asset["entity_id"].as_str().unwrap_or(id);
            let entity = entities.get(movie_id).cloned().unwrap_or(json!({}));
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
            let this_package = package(asset);
            let entity_id = if kind == "movie" && first_package.get(movie_id).is_some_and(|(_, first)| *first != this_package) {
                format!("{movie_id}:copy:{this_package}")
            } else {
                movie_id.to_owned()
            };
            let entity_id = entity_id.as_str();
            let title=titles.entry(entity_id.to_owned()).or_insert_with(||json!({
                "id":entity_id,"copy_of":if entity_id != movie_id { json!(movie_id) } else { Value::Null },"continue_watching_hidden":hidden.contains(entity_id),"kind":kind,"title":name,"original_title":entity["original_title"],
                "year":title_year(&entity,file),"end_year":series_end_year(&entity),"ongoing":series_ongoing(&entity),"release_date":entity["release_date"],"genres":entity.get("genres").unwrap_or(&json!([])),
                "summary":entity["summary"].as_str().unwrap_or(""),"content_rating":entity["content_rating"],"duration_minutes":entity["duration_minutes"],
                "poster":entity["thumbnail"],"backdrop":entity["backdrop"],"logo":entity["logo"],"cast":entity["cast"],"provider":entity["provider"],"source_url":entity["source_url"],
                "added_at":0.0,"status":asset["status"],"assets":[]
            }));
            // A title is as recent as the latest of its files to become watchable.
            let arrived = asset["ready_at"].as_f64().filter(|at| *at > 0.0).or(asset["added_at"].as_f64()).unwrap_or(0.0);
            if arrived > title["added_at"].as_f64().unwrap_or(0.0) {
                title["added_at"] = json!(arrived);
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
            // Specials (season 0) are extras: listed with the bonus material, never in the
            // seasons or the next-episode flow. Release packs number specials differently
            // from TMDB, so they keep the name from their file. A special marked by hand as
            // `watch_first` (a pilot miniseries) stays an episode of season 0, before season 1.
            if kind == "tv" && asset["season"] == 0 && asset["watch_first"] != true {
                title["assets"].as_array_mut().context("Invalid title assets")?.push(json!({
                    "id":id,"media_info":crate::media_inventory::cached(self,id)?,"role":"extra","name":extra_name(asset),"season":null,"episode":null,
                    "summary":"","thumbnail":null,"complete":asset["complete"],"size":asset["size"],"position":progress["position"].as_f64().unwrap_or(0.0),
                    "duration":progress["duration"].as_f64().unwrap_or(0.0),"watched":progress["watched"].as_bool().unwrap_or(false),
                    "last_played":progress["last_played"].as_f64().unwrap_or(0.0),"torrent_hash":asset["torrent_hash"]
                }));
                continue;
            }
            title["assets"].as_array_mut().context("Invalid title assets")?.push(json!({
                "id":id,"media_info":crate::media_inventory::cached(self,id)?,"role":"main","name":if asset["season"] == 0 { extra_name(asset) } else { episode["title"].as_str().unwrap_or(if kind=="movie"{name}else{&fallback}).to_owned() },
                "season":asset["season"],"episode":asset["episode"],"summary":episode["summary"].as_str().unwrap_or(""),
                "airdate":episode["airdate"],"thumbnail":episode["thumbnail"],"duration_minutes":episode.get("duration_minutes").unwrap_or(&entity["duration_minutes"]),
                "complete":asset["complete"],"size":asset["size"],"position":progress["position"].as_f64().unwrap_or(0.0),
                "duration":progress["duration"].as_f64().unwrap_or(0.0),"watched":progress["watched"].as_bool().unwrap_or(false),
                "last_played":progress["last_played"].as_f64().unwrap_or(0.0),"torrent_hash":asset["torrent_hash"]
            }));
        }
        for asset in assets
            .iter()
            .filter(|a| a["available"] == true && a["complete"] == true && a["role"] == "extra")
        {
            let Some(parent) = asset["parent_title_id"].as_str() else {
                continue;
            };
            let Some(title) = titles.get_mut(parent) else {
                continue;
            };
            let id = asset["id"].as_str().context("Invalid extra ID")?;
            let name = extra_name(asset);
            let media = format!(
                "{}:{}",
                asset["torrent_hash"].as_str().unwrap_or(id),
                asset["file_index"].as_u64().unwrap_or(0)
            );
            let progress = history.get(&media).cloned().unwrap_or(json!({}));
            title["assets"].as_array_mut().context("Invalid title assets")?.push(json!({
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
        let assets = self.visible_assets()?;
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
                // Matched files decide the parent; a stray unmatched file in the package
                // (a featurette outside its folder) must not leave every extra orphaned.
                let mains: Vec<_> = assets.iter().filter(|a| {
                    a["available"] == true && a["role"] != "extra" && a["torrent_hash"] == hash
                }).collect();
                let matched: Vec<_> = mains.iter().filter_map(|m| m["entity_id"].as_str()).collect();
                if matched.is_empty() {
                    parents.extend(mains.iter().filter_map(|m| m["id"].as_str()).map(str::to_owned));
                } else {
                    parents.extend(matched.into_iter().map(str::to_owned));
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

/// An extra's name from its file, without the series prefix, special number and
/// release tags: `Show (2003) - S00E22 - The Plan (1080p BluRay x265)` is `The Plan`.
/// A `display_name` on the asset (set by hand for a mislabelled file) wins.
fn extra_name(asset: &Value) -> String {
    if let Some(name) = asset["display_name"].as_str().filter(|n| !n.trim().is_empty()) {
        return name.to_owned();
    }
    static NUMBER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)^.*?\bs\d{1,2}e\d{1,3}(?:-?e\d{1,3})*\s*-\s*").expect("valid regex")
    });
    static TAGS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\s*\((?:[^()]*\b(?:480p|576p|720p|1080p|2160p|bluray|web-?dl|webrip|hdtv|dvdrip)\b[^()]*)\)\s*$").expect("valid regex")
    });
    let stem = std::path::Path::new(asset["path"].as_str().unwrap_or(""))
        .file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let spaced = if stem.contains(' ') { stem.clone() } else { stem.replace(['.', '_'], " ") };
    let name = TAGS.replace(&NUMBER.replace(&spaced, ""), "").trim().to_owned();
    if name.is_empty() { spaced } else { name }
}

#[cfg(test)]
mod completion_tests {
    use super::*;
    #[test]
    fn catalog_revision_ignores_heartbeats_but_tracks_library_changes() -> Result<()> {
        let mut db=Database {connection:rusqlite::Connection::open_in_memory()?};
        db.connection.execute_batch(include_str!("schema.sql"))?;
        db.migrate_profiles(9)?;
        db.initialize_catalog_revision()?;
        db.connection.execute("INSERT INTO organizer_state VALUES(1,'{}')",[])?;
        let first=db.catalog_revision()?;
        db.connection.execute("UPDATE organizer_state SET document='{\"last_poll\":42}'",[])?;
        db.connection.execute("INSERT INTO clients VALUES('test','TV','other',0,0)",[])?;
        assert_eq!(first,db.catalog_revision()?);
        db.connection.execute("INSERT INTO library_assets VALUES('a','{}')",[])?;
        let asset=db.catalog_revision()?;
        assert_ne!(first,asset);
        db.connection.execute("UPDATE library_assets SET document='{}'",[])?;
        assert_eq!(asset,db.catalog_revision()?);
        db.connection.execute("UPDATE library_assets SET document='{\"complete\":true}'",[])?;
        assert_ne!(asset,db.catalog_revision()?);
        let changed=db.catalog_revision()?;
        db.initialize_catalog_revision()?;
        assert_eq!(changed,db.catalog_revision()?);
        db.connection.execute("UPDATE organizer_state SET document='{\"library_scan\":{\"error\":\"failed\"}}'",[])?;
        assert_ne!(changed,db.catalog_revision()?);
        Ok(())
    }

    #[test]
    fn incomplete_assets_never_create_library_titles() -> Result<()> {
        let mut db = Database { connection: rusqlite::Connection::open_in_memory()? };
        db.connection.execute_batch(include_str!("schema.sql"))?;
        db.migrate_profiles(9)?;
        db.connection.execute("INSERT INTO organizer_state VALUES(1, ?)", ["{}"])?;
        db.connection.execute("INSERT INTO library_entities VALUES('friends',?)",[r#"{"id":"friends","kind":"tv","title":"Friends"}"#])?;
        for complete in [false, true] {
            let asset = json!({"id":"episode", "available":true,"complete":complete,
                "path":"/TV/Friends S01E01.mkv", "season":1,"episode":1,"entity_id":"friends","status":"matched","library_ready":true});
            db.connection.execute("INSERT OR REPLACE INTO library_assets VALUES(?,?)",
                rusqlite::params!["episode",asset.to_string()])?;
            assert_eq!(db.catalog(crate::profiles::LEGACY_PROFILE)?["titles"].as_array().unwrap().len(), usize::from(complete));
        }
        Ok(())
    }

    #[test]
    fn each_download_of_a_movie_is_its_own_title() -> Result<()> {
        let mut db = Database { connection: rusqlite::Connection::open_in_memory()? };
        db.connection.execute_batch(include_str!("schema.sql"))?;
        db.migrate_profiles(9)?;
        db.connection.execute("INSERT INTO organizer_state VALUES(1, ?)", ["{}"])?;
        db.connection.execute("INSERT INTO library_entities VALUES('movie',?)",[r#"{"id":"movie","kind":"movie","title":"Casino Royale"}"#])?;
        db.connection.execute("INSERT INTO library_entities VALUES('show',?)",[r#"{"id":"show","kind":"tv","title":"Friends"}"#])?;
        let assets = [("collection", "movie", "hash-b", 20.0, None), ("single", "movie", "hash-a", 10.0, None),
            ("part-two", "movie", "hash-a", 11.0, None), ("e1", "show", "hash-c", 1.0, Some(1)), ("e1-again", "show", "hash-d", 2.0, Some(1))];
        for (id, entity, hash, added, episode) in assets {
            let asset = json!({"id":id,"available":true,"complete":true,"path":format!("/Media/{id}.mp4"),"entity_id":entity,
                "status":"matched","library_ready":true,"torrent_hash":hash,"added_at":added,"season":episode.map(|_|1),"episode":episode});
            db.connection.execute("INSERT INTO library_assets VALUES(?,?)", rusqlite::params![id, asset.to_string()])?;
        }
        let catalog = db.catalog(crate::profiles::LEGACY_PROFILE)?;
        let titles = catalog["titles"].as_array().unwrap();
        let files = |id: &str| titles.iter().find(|t| t["id"] == id).map(|t| t["assets"].as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap().to_owned()).collect::<Vec<_>>());
        // The earliest download keeps the movie's own ID; files of one download stay together.
        assert_eq!(files("movie"), Some(vec!["single".into(), "part-two".into()]));
        assert_eq!(files("movie:copy:hash-b"), Some(vec!["collection".into()]));
        assert_eq!(titles.iter().find(|t| t["id"] == "movie:copy:hash-b").unwrap()["copy_of"], "movie");
        // Series still gather every download under one title.
        assert_eq!(files("show").map(|f| f.len()), Some(2));
        assert_eq!(titles.len(), 3);
        Ok(())
    }

    #[test]
    fn a_stray_unmatched_file_does_not_orphan_extras() -> Result<()> {
        let db = Database { connection: rusqlite::Connection::open_in_memory()? };
        db.connection.execute_batch(include_str!("schema.sql"))?;
        for (id, doc) in [("e1", json!({"id":"e1","available":true,"role":"main","entity_id":"show","torrent_hash":"h","path":"/M/Show/S01/Show - S01E01.mkv"})),
            ("oracle", json!({"id":"oracle","available":true,"role":"main","status":"review","torrent_hash":"h","path":"/M/Show/S01/The Oracle.mkv"})),
            ("bonus", json!({"id":"bonus","available":true,"role":"extra","torrent_hash":"h","path":"/M/Show/S01/Featurettes/Bonus.mkv"})),
            ("lone", json!({"id":"lone","available":true,"role":"main","status":"review","torrent_hash":"k","path":"/M/Other/clip.mkv"})),
            ("lone-extra", json!({"id":"lone-extra","available":true,"role":"extra","torrent_hash":"k","path":"/M/Other/Extras/x.mkv"}))] {
            db.connection.execute("INSERT INTO library_assets VALUES(?,?)", rusqlite::params![id, doc.to_string()])?;
        }
        db.link_extras()?;
        let parent = |id: &str| -> Result<Value> { Ok(db.asset(id)?["parent_title_id"].clone()) };
        assert_eq!(parent("bonus")?, "show");
        // With nothing matched, the unmatched file still anchors its extras.
        assert_eq!(parent("lone-extra")?, "lone");
        Ok(())
    }

    #[test]
    fn specials_are_extras_named_from_their_files() -> Result<()> {
        let mut db = Database { connection: rusqlite::Connection::open_in_memory()? };
        db.connection.execute_batch(include_str!("schema.sql"))?;
        db.migrate_profiles(9)?;
        db.connection.execute("INSERT INTO organizer_state VALUES(1, ?)", ["{}"])?;
        db.connection.execute("INSERT INTO library_entities VALUES('show',?)",[r#"{"id":"show","kind":"tv","title":"Show",
            "episodes":[{"season":0,"episode":22,"title":"Wrong TMDB Special"},{"season":1,"episode":1,"title":"Pilot"}]}"#])?;
        let assets = [("plan", "/Media/Show (2003) - S00E22 - The Plan (1080p BluRay x265 Celdra).mkv", Some(0), None),
            ("razor", "/Media/Show (2003) - S00E04-E05 - Razor (1080p BluRay x265 Celdra).mkv", Some(0), None),
            ("mini", "/Media/Show (2003) - S00E01 - The Lowdown.mkv", Some(0), Some("The Miniseries (Part 1)")),
            ("pilot-movie", "/Media/Show (2003) - S00E02 - The Miniseries (2).mkv", Some(0), None),
            ("pilot", "/Media/Show (2003) - S01E01 - Pilot.mkv", Some(1), None)];
        for (id, path, season, display) in assets {
            let episode = if id == "plan" { 22 } else { 1 };
            let asset = json!({"id":id,"available":true,"complete":true,"path":path,"entity_id":"show","display_name":display,"watch_first":id == "pilot-movie",
                "status":"matched","library_ready":true,"torrent_hash":"hash","added_at":1.0,"season":season,"episode":episode});
            db.connection.execute("INSERT INTO library_assets VALUES(?,?)", rusqlite::params![id, asset.to_string()])?;
        }
        let catalog = db.catalog(crate::profiles::LEGACY_PROFILE)?;
        let files = catalog["titles"][0]["assets"].as_array().unwrap().clone();
        let file = |id: &str| files.iter().find(|a| a["id"] == id).unwrap().clone();
        assert_eq!((file("pilot")["role"].clone(), file("pilot")["name"].clone()), (json!("main"), json!("Pilot")));
        // A special marked to watch first stays an episode of season 0, named from its file.
        assert_eq!((file("pilot-movie")["role"].clone(), file("pilot-movie")["season"].clone(), file("pilot-movie")["name"].clone()),
            (json!("main"), json!(0), json!("The Miniseries (2)")));
        for (id, name) in [("plan", "The Plan"), ("razor", "Razor"), ("mini", "The Miniseries (Part 1)")] {
            assert_eq!(file(id)["role"], "extra", "{id}");
            assert_eq!(file(id)["name"], name, "{id}");
            assert!(file(id)["season"].is_null() && file(id)["episode"].is_null(), "{id}");
        }
        Ok(())
    }
}

#[cfg(test)]
mod continue_watching_tests {
    #[test]
    fn series_run_is_reported_only_when_the_status_is_known() {
        use serde_json::json;
        let ended=json!({"series_status":"Ended","last_air_date":"2025-05-18"});
        assert_eq!(super::series_ongoing(&ended),json!(false));
        assert_eq!(super::series_end_year(&ended),json!("2025"));
        let running=json!({"series_status":"Returning Series","last_air_date":"2026-03-01"});
        assert_eq!(super::series_ongoing(&running),json!(true));
        assert!(super::series_end_year(&running).is_null());
        let unknown=json!({"year":"2018"});
        let path=std::path::Path::new("/Media/TV/Show (2014)/Season 1/Show - S01E01.mkv");
        assert_eq!(super::title_year(&json!({"year":"2018"}),path),json!("2018"));
        assert_eq!(super::title_year(&json!({"year":"","release_date":"2016-05-01"}),path),json!("2016"));
        assert_eq!(super::title_year(&json!({"episodes":[{"airdate":"2012-01-08"},{"airdate":"2011-10-02"}]}),path),json!("2011"));
        assert_eq!(super::title_year(&json!({}),path),json!("2014"));
        assert!(super::title_year(&json!({}),std::path::Path::new("/Media/Movies/Untitled/clip.mkv")).is_null());
        assert!(super::series_ongoing(&unknown).is_null());
        assert!(super::series_end_year(&unknown).is_null());
    }

    use super::*;
    #[test]
    fn hiding_preserves_progress_and_next_play_restores_only_that_profile() -> Result<()> {
        let mut db=Database{connection:rusqlite::Connection::open_in_memory()?};
        db.connection.execute_batch(include_str!("schema.sql"))?;
        db.migrate_profiles(0)?;
        let other=db.create_profile("Other")?["id"].as_str().unwrap().to_owned();
        let profile=crate::profiles::LEGACY_PROFILE;
        let client=uuid::Uuid::new_v4().to_string();db.register(&client,"TV","other")?;
        db.connection.execute("INSERT INTO library_assets(id,document) VALUES(?,?)",["asset",r#"{"id":"asset","entity_id":"show","torrent_hash":"hash","file_index":0}"#])?;
        db.begin(profile,"hash",0,"Episode",100,&client)?;
        db.connection.execute("UPDATE profile_playback SET position=123,duration=1000 WHERE profile_id=?",[profile])?;
        for person in [profile,other.as_str()] {db.connection.execute("INSERT INTO profile_hidden_titles VALUES(?,'show',1)",[person])?;}
        let next=db.begin(profile,"hash",0,"Episode",100,&client)?;
        assert_eq!(next["resume_position"],123.0);
        assert_eq!(db.rows("SELECT profile_id FROM profile_hidden_titles",[])?.len(),1);
        assert_eq!(db.rows("SELECT profile_id FROM profile_hidden_titles",[])?[0]["profile_id"],other);
        Ok(())
    }
}
