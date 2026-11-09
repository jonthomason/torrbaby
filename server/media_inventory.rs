//! Persistent media-language inventory and low-priority, daemon-owned subtitle preparation.
use crate::{api::Service, storage::{Database, now}};
use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};
use std::path::Path;

pub fn cached(db: &Database, id: &str) -> Result<Value> {
    let row: Option<(String,String)> = db.connection.query_row(
        "SELECT revision,document FROM media_inventory WHERE asset_id=?", [id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((revision, text)) = row {
        let asset = db.asset(id)?;
        if asset["path"].as_str().and_then(|p| crate::subtitle_jobs::revision(Path::new(p)).ok()).as_deref() == Some(&revision) {
            return Ok(serde_json::from_str(&text)?);
        }
    }
    Ok(json!({"state":"pending","audio_tracks":[],"subtitle_tracks":[],"prepared":[]}))
}
fn save(db: &Database, id: &str, revision: &str, info: &Value) -> Result<()> {
    db.connection.execute("INSERT INTO media_inventory VALUES(?,?,?,?) ON CONFLICT(asset_id) DO UPDATE SET revision=excluded.revision,document=excluded.document,updated=excluded.updated",
        rusqlite::params![id,revision,info.to_string(),now()])?;
    Ok(())
}
fn language(track: &Value) -> Option<&'static str> {
    let tag = track["language"].as_str()?.to_ascii_lowercase();
    if tag == "en" || tag == "eng" || tag.starts_with("en-") { Some("en") }
    else if tag == "zh" || tag == "zho" || tag == "chi" || tag.starts_with("zh-") { Some("zh-Hans") }
    else { None }
}

fn priorities(db: &Database) -> Result<std::collections::HashMap<String, i64>> {
    let mut statement = db.connection.prepare("SELECT asset_id,priority FROM subtitle_priorities")?;
    Ok(statement.query_map([], |row| Ok((row.get(0)?,row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn prioritize(db: &Database, id: &str) -> Result<Value> {
    let asset = db.asset(id)?;
    anyhow::ensure!(asset["available"] == true && asset["complete"] == true,
        "Wait for this download to finish before preparing subtitles");
    db.connection.execute("INSERT INTO subtitle_priorities(asset_id,priority) VALUES(?,(SELECT COALESCE(MAX(priority),0)+1 FROM subtitle_priorities)) ON CONFLICT(asset_id) DO UPDATE SET priority=excluded.priority", [id])?;
    Ok(json!({"accepted":true}))
}

fn group_key(asset: &Value) -> String {
    asset["entity_id"].as_str().or(asset["series_name"].as_str()).or(asset["id"].as_str()).unwrap_or("").to_owned()
}
fn queue_assets(db: &Database) -> Result<Vec<Value>> {
    let entities: std::collections::HashMap<String,Value> = db.documents("entities")?.into_iter()
        .filter_map(|e| e["id"].as_str().map(str::to_owned).map(|id|(id,e))).collect();
    let mut assets=db.documents("assets")?;
    for a in &mut assets {
        a["queue_title"]=a["entity_id"].as_str().and_then(|id|entities.get(id)).map(|e|e["title"].clone())
            .unwrap_or_else(||a["series_name"].clone());
    }
    Ok(assets)
}
fn order_assets(assets: &mut [Value], priorities: &std::collections::HashMap<String,i64>) {
    let mut groups=std::collections::HashMap::<String,i64>::new();
    for a in assets.iter() {
        let priority=*priorities.get(a["id"].as_str().unwrap_or("")).unwrap_or(&0);
        let entry=groups.entry(group_key(a)).or_default(); *entry=(*entry).max(priority);
    }
    let title=|a:&Value|a["queue_title"].as_str().or(a["series_name"].as_str()).or(a["id"].as_str()).unwrap_or("").to_lowercase();
    assets.sort_by(|a,b| groups[&group_key(b)].cmp(&groups[&group_key(a)])
        .then_with(||title(a).cmp(&title(b)))
        .then_with(||group_key(a).cmp(&group_key(b)))
        .then_with(||a["season"].as_u64().unwrap_or(0).cmp(&b["season"].as_u64().unwrap_or(0)))
        .then_with(||a["episode"].as_u64().unwrap_or(0).cmp(&b["episode"].as_u64().unwrap_or(0)))
        .then_with(||a["id"].as_str().cmp(&b["id"].as_str())));
}

pub async fn tick(s: &Service) -> Result<()> {
    if !s.effective_config()?.background_subtitles { return Ok(()); }
    let (mut assets, priorities) = {
        let db = s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        (queue_assets(&db)?, priorities(&db)?)
    };
    assets.retain(|a| a["available"]==true && a["complete"]==true);
    order_assets(&mut assets, &priorities);
    let mut inspected = false;
    for asset in assets {
        let Some(id) = asset["id"].as_str() else { continue; };
        let Some(path) = asset["path"].as_str() else { continue; };
        if !Path::new(path).extension().and_then(|s|s.to_str()).is_some_and(|e|["mkv","webm","mp4","m4v","mov"].contains(&e.to_ascii_lowercase().as_str())) { continue; }
        let Ok(revision) = crate::subtitle_jobs::revision(Path::new(path)) else { continue; };
        let mut info = {
            let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
            cached(&db,id)?
        };
        if info["retry_after"].as_f64().is_some_and(|t| t>now()) { continue; }
        if info["state"] == "pending" || info["state"] == "failed" {
            if inspected { continue; }
            inspected=true;
            let input = match crate::api::media_file(s,id,0).await { Ok(p)=>p, Err(_)=>continue };
            match crate::native_subtitles::tracks(&input).await {
                Ok(inventory)=> {
                    let sidecar_input=input.clone();
                    let sidecars=tokio::task::spawn_blocking(move || crate::subtitles::discover(&sidecar_input)).await??;
                    let mut subtitle_tracks=inventory["tracks"].as_array().cloned().unwrap_or_default();
                    subtitle_tracks.extend(sidecars.into_iter().map(|t|json!({"language":t.language,"name":t.name,"sidecar":true,"text_extractable":true,"extraction_supported":false,"forced":t.forced})));
                    info=json!({"state":"ready","audio_tracks":inventory["audio_tracks"].as_array().cloned().unwrap_or_default(),
                        "subtitle_tracks":subtitle_tracks,"prepared":[],"jobs":[],"scanned_at":now()});
                }
                Err(error)=> { info=json!({"state":"failed","error":error.to_string(),"retry_after":now()+3600.0,"audio_tracks":[],"subtitle_tracks":[],"prepared":[]}); }
            }
            let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
            save(&db,id,&revision,&info)?;
        }
        if info["state"] != "ready" { continue; }
        let jobs=info["jobs"].as_array().cloned().unwrap_or_default();
        let mut prepared=Vec::new();
        let mut active=false;
        let mut failed=false;
        let mut interrupted=false;
        for job_id in &jobs {
            let status=s.subtitle_jobs.status(job_id.as_str().unwrap_or(""));
            if let Ok(mut job)=status {
                if job["state"]=="ready" {
                    job["language"]=job["target"].as_str().or(job["source"].as_str()).map(|s|json!(s)).unwrap_or(json!("und"));
                    job["generated"]=json!(!job["target"].is_null());
                    prepared.push(job);
                } else if job["state"]=="queued" || job["state"]=="running" { active=true; }
                else if ["interrupted","expired","stale"].iter().any(|state|job["state"]==*state) { interrupted=true; }
                else { failed=true; }
            } else { failed=true; }
        }
        let old=info.clone();
        info["prepared"]=json!(prepared);
        info["preparation_state"]=json!(if active {"preparing"} else if failed {"failed"} else if !jobs.is_empty() {"ready"} else {"pending"});
        if interrupted { info["jobs"]=json!([]); }
        if failed { info["retry_after"]=json!(now()+3600.0); info["jobs"]=json!([]); }
        if info!=old {
            let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
            save(&db,id,&revision,&info)?;
        }
        if !jobs.is_empty() || s.subtitle_jobs.busy() { continue; }
        let tracks=info["subtitle_tracks"].as_array().context("Invalid track inventory")?;
        // Existing Chinese is already available; never translate over it. Unknown
        // languages and bitmap tracks require no guessing and no OCR.
        let has_chinese=tracks.iter().any(|t|language(t)==Some("zh-Hans") && t["text_extractable"]==true);
        let has_english=tracks.iter().any(|t|language(t)==Some("en") && t["text_extractable"]==true);
        let (source_language,target_language)=if !has_chinese { ("en","zh-Hans") }
            else if !has_english { ("zh-Hans","en") } else { continue; };
        let Some(source)=tracks.iter().find(|t| language(t)==Some(source_language) && t["extraction_supported"]==true && t["forced"]!=true) else { continue; };
        let Some(track)=source["track"].as_u64() else { continue; };
        let input=crate::api::media_file(s,id,0).await?;
        let original=s.subtitle_jobs.prepare(id,input.clone(),track,Some(source_language.into()),None)?;
        let translated=s.subtitle_jobs.prepare(id,input,track,Some(source_language.into()),Some(target_language.into()))?;
        info["jobs"]=json!([original["id"],translated["id"]]);
        info["preparation_state"]=json!("preparing");
        let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        save(&db,id,&revision,&info)?;
        break;
    }
    Ok(())
}

pub fn summary(db: &Database, jobs: &crate::subtitle_jobs::Manager) -> Result<Value> {
    let mut scanned=0usize;
    let mut translated=0usize;
    let mut failed=0usize;
    let mut current=Value::Null;
    let mut queue=Vec::new();
    let statuses=jobs.all_statuses()?;
    let priorities = priorities(db)?;
    let mut assets = queue_assets(db)?;
    order_assets(&mut assets, &priorities);
    for asset in assets {
        if asset["available"]!=true || asset["complete"]!=true { continue; }
        let Some(id)=asset["id"].as_str() else {continue};
        let info=cached(db,id)?;
        let title=asset["path"].as_str().and_then(|p|Path::new(p).file_stem()).and_then(|s|s.to_str()).unwrap_or(id);
        let tracks=info["subtitle_tracks"].as_array().cloned().unwrap_or_default();
        let has=|lang| tracks.iter().any(|t|language(t)==Some(lang) && t["text_extractable"]==true);
        let source=if !has("zh-Hans") {Some(("en","zh-Hans"))} else if !has("en") {Some(("zh-Hans","en"))} else {None};
        let eligible=source.filter(|(lang,_)|tracks.iter().any(|t|language(t)==Some(*lang) && t["extraction_supported"]==true && t["forced"]!=true));
        let mut listed=false;
        for status in statuses.iter().filter(|job|job["asset_id"]==id) {
            let mut job=status.clone();
            job["title"]=json!(title);
            queue.push(job);
            listed=true;
        }
        if !listed && (eligible.is_some() || info["state"]!="ready") {
            queue.push(json!({"id":id,"asset_id":id,"title":title,
                "state":if info["state"]=="failed" || info["preparation_state"]=="failed" {"failed"} else if info["state"]=="pending" {"scanning"} else {"queued"},
                "target":eligible.map(|(_,target)|target),"error":info["error"],"completed_cues":0,"total_cues":0}));
        }
        if info["state"]=="ready" { scanned+=1; }
        let mut ready=false;
        for job in statuses.iter().filter(|job|job["asset_id"]==id) {
            if job["target"].is_null() {continue;}
            if job["state"]=="ready" {ready=true;}
            if job["state"]=="failed" {failed+=1;}
            if job["state"]=="running" {
                current=json!({"asset_id":id,"language":job["target"],"completed_cues":job["completed_cues"],"total_cues":job["total_cues"]});
            }
        }
        if ready {translated+=1;}
        if !listed && info["preparation_state"]=="failed" {failed+=1;}
    }

    Ok(json!({"scanned":scanned,"translated":translated,"failed":failed,"waiting_for_idle":false,"current":current,"queue":queue}))
}

#[cfg(test)]
mod priority_tests {
    use super::*;
    #[test]
    fn shows_stay_together_in_episode_order_and_priority_moves_the_show() {
        let mut assets=vec![
            json!({"id":"a2","entity_id":"A","queue_title":"Alpha","season":1,"episode":2}),
            json!({"id":"b2","entity_id":"B","queue_title":"Beta","season":2,"episode":1}),
            json!({"id":"a1","entity_id":"A","queue_title":"Alpha","season":1,"episode":1}),
            json!({"id":"b1","entity_id":"B","queue_title":"Beta","season":1,"episode":1})];
        let mut priorities=std::collections::HashMap::new();
        order_assets(&mut assets,&priorities);
        assert_eq!(assets.iter().map(|a|a["id"].as_str().unwrap()).collect::<Vec<_>>(),vec!["a1","a2","b1","b2"]);
        priorities.insert("b2".into(),1);
        order_assets(&mut assets,&priorities);
        assert_eq!(assets.iter().map(|a|a["id"].as_str().unwrap()).collect::<Vec<_>>(),vec!["b1","b2","a1","a2"]);
    }
    #[test]
    fn clicked_items_move_to_front_and_order_is_persistent() -> Result<()> {
        let db = Database { connection: rusqlite::Connection::open_in_memory()? };
        db.connection.execute_batch(include_str!("schema.sql"))?;
        let mut assets = vec![json!({"id":"old","added_at":1,"available":true,"complete":true}),
            json!({"id":"new","added_at":2,"available":true,"complete":true})];
        for a in &assets { db.connection.execute("INSERT INTO library_assets VALUES(?,?)",
            rusqlite::params![a["id"].as_str().unwrap(),a.to_string()])?; }
        order_assets(&mut assets,&priorities(&db)?);
        assert_eq!(assets[0]["id"],"new");
        prioritize(&db,"old")?;
        order_assets(&mut assets,&priorities(&db)?);
        assert_eq!(assets[0]["id"],"old");
        prioritize(&db,"new")?;
        prioritize(&db,"old")?;
        order_assets(&mut assets,&priorities(&db)?);
        assert_eq!(assets[0]["id"],"old");
        assert_eq!(priorities(&db)?.len(),2);
        assert!(prioritize(&db,"missing").is_err());
        db.connection.execute("INSERT INTO library_assets VALUES('partial',?)",
            [json!({"id":"partial","available":true,"complete":false}).to_string()])?;
        assert!(prioritize(&db,"partial").is_err());
        Ok(())
    }
}
