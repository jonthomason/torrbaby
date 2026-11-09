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

pub async fn tick(s: &Service) -> Result<()> {
    if !s.effective_config()?.background_subtitles { return Ok(()); }
    let (mut assets, watching) = {
        let db = s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        let watching: bool = db.connection.query_row("SELECT EXISTS(SELECT 1 FROM playback p JOIN playback_sessions s ON s.id=p.active_session WHERE s.ended=0 AND p.last_played>?)", [now()-90.0], |r|r.get(0))?;
        (db.documents("assets")?, watching)
    };
    assets.retain(|a| a["available"]==true && a["complete"]==true);
    assets.sort_by(|a,b| b["added_at"].as_f64().unwrap_or(0.0).total_cmp(&a["added_at"].as_f64().unwrap_or(0.0)));
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
            if inspected || watching { continue; }
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
        if watching || !jobs.is_empty() || s.subtitle_jobs.busy() { continue; }
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
