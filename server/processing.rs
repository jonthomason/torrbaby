//! SQLite is the authority for queue membership, progress and final sign-off.
//! Subtitle/VTT and credits caches are reusable stage outputs, not separate queues.
use crate::{api::Service, storage::{Database, now}};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::Path;
const VERSION:i64=1;

fn enroll(db:&Database,id:&str,revision:&str)->Result<()> {
    db.connection.execute("INSERT INTO media_processing(asset_id,revision,version,state,stage,inventory_state,subtitles_state,credits_state,updated) VALUES(?,?,?,'queued','inventory','pending','pending','pending',?) ON CONFLICT(asset_id) DO UPDATE SET revision=excluded.revision,version=excluded.version,state='queued',stage='inventory',inventory_state='pending',subtitles_state='pending',credits_state='pending',detail='{}',error=NULL,retry_after=0,attempts=0,completed_at=NULL,updated=excluded.updated WHERE media_processing.revision!=excluded.revision OR media_processing.version!=excluded.version",rusqlite::params![id,revision,VERSION,now()])?;
    Ok(())
}
fn finished(value:&str)->bool {matches!(value,"ready"|"not_applicable")}
fn sign_off(db:&Database,id:&str,revision:&str)->Result<bool> {
    // The SQL predicate is deliberately the final authority: a worker becoming
    // idle, an English artifact, or an empty job list can never sign off a file.
    let complete=db.connection.execute("UPDATE media_processing SET state='ready',stage='complete',completed_at=?,updated=?,error=NULL WHERE asset_id=? AND revision=? AND inventory_state='ready' AND subtitles_state IN ('ready','not_applicable') AND credits_state IN ('ready','not_applicable')",rusqlite::params![now(),now(),id,revision])?==1;
    if complete {db.connection.execute("UPDATE processing_failures SET resolved_at=? WHERE asset_id=? AND revision=? AND resolved_at IS NULL",rusqlite::params![now(),id,revision])?;}
    Ok(complete)
}
fn fail(db:&Database,id:&str,revision:&str,error:&str)->Result<()> {
    db.connection.execute("INSERT INTO processing_failures(asset_id,revision,stage,error,first_seen,last_seen) SELECT asset_id,revision,stage,?,?,? FROM media_processing WHERE asset_id=? AND revision=? ON CONFLICT(asset_id,revision,stage,error) DO UPDATE SET occurrences=occurrences+1,last_seen=excluded.last_seen,resolved_at=NULL",rusqlite::params![error,now(),now(),id,revision])?;
    db.connection.execute("UPDATE media_processing SET state='failed',error=?,retry_after=?,updated=?,attempts=attempts+1 WHERE asset_id=? AND revision=?",rusqlite::params![error,now()+3600.0,now(),id,revision])?;Ok(())
}
fn assets(db:&Database)->Result<Vec<Value>> {
    let mut assets=crate::media_inventory::queue_assets(db)?;
    assets.retain(|a|a["available"]==true && a["complete"]==true);
    crate::media_inventory::order_assets(&mut assets,&crate::media_inventory::priorities(db)?);
    Ok(assets)
}
/// Recent authenticated playback heartbeats, including paused players, reserve capacity.
pub fn playback_active(s:&Service)->Result<bool> {
    let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
    Ok(db.connection.query_row("SELECT EXISTS(SELECT 1 FROM profile_playback p JOIN playback_sessions s ON p.active_session=s.id WHERE s.ended=0 AND p.last_played>?)",[now()-90.0],|r|r.get(0))?)
}
fn capacity(enabled:bool, preparing:bool, playing:bool, idle:usize)->usize {
    if !enabled || preparing {0} else if playing {1} else {idle.clamp(1,8)}
}
pub fn refresh_capacity(s:&Service)->Result<usize> {
    let preparing=s.virtual_media.preparing();
    s.subtitle_jobs.set_urgent(preparing);
    let config=s.effective_config()?;
    let limit=capacity(config.background_subtitles,preparing,playback_active(s)?,config.processing_concurrency);
    s.subtitle_jobs.set_concurrency(limit);
    s.subtitle_jobs.set_external(config.external_translation);
    Ok(limit)
}
/// Files with a stage task in flight. Aborting a task drops its future, and every
/// helper process is spawned with kill_on_drop, so backing down is immediate.
static IN_FLIGHT:std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String,tokio::task::AbortHandle>>>=std::sync::LazyLock::new(Default::default);
/// Catalog revision and time of the last enrollment pass.
static ENROLLED:std::sync::Mutex<Option<(String,f64)>>=std::sync::Mutex::new(None);

pub async fn tick(s:&std::sync::Arc<Service>)->Result<()> {
    let limit=refresh_capacity(s)?;
    let imported=s.subtitle_jobs.import_external()?;
    let (selected,excess)={
        let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        for id in &imported {db.connection.execute("UPDATE media_processing SET retry_after=0 WHERE asset_id=? AND state='queued'",[id])?;}
        let assets=assets(&db)?;
        let migrating:bool=db.connection.query_row("SELECT NOT EXISTS(SELECT 1 FROM media_processing)",[],|r|r.get(0))?;
        // Enrollment stats every media file; repeat it only when the catalog changes
        // or once a minute (to notice files replaced in place), not every tick.
        let revision=db.catalog_revision()?;
        let due=ENROLLED.lock().map(|e|e.as_ref().is_none_or(|(r,t)|*r!=revision || now()-t>=60.0)).unwrap_or(true);
        if due || migrating {
            for a in &assets {
                let Some(id)=a["id"].as_str() else{continue};
                let Some(path)=a["path"].as_str() else{continue};
                // Missing/unreadable files remain visible as failures, not vanished work.
                let revision=crate::subtitle_jobs::revision(Path::new(path));
                enroll(&db,id,revision.as_deref().unwrap_or("unavailable"))?;
                if let Err(error)=revision {fail(&db,id,"unavailable",&error.to_string())?;}
            }
            if let Ok(mut e)=ENROLLED.lock() {*e=Some((revision,now()));}
        }
        if migrating {
            let mut unfinished=s.subtitle_jobs.all_statuses()?;
            unfinished.retain(|j|matches!(j["state"].as_str(),Some("running"|"queued"|"interrupted")));
            unfinished.sort_by(|a,b|b["updated"].as_f64().unwrap_or(0.0).total_cmp(&a["updated"].as_f64().unwrap_or(0.0)));
            if let Some(job)=unfinished.iter().find(|j|assets.iter().any(|a|a["id"]==j["asset_id"])) {
                db.connection.execute("UPDATE media_processing SET state='running' WHERE asset_id=?",[job["asset_id"].as_str().unwrap_or("")])?;
            }
        }
        let rows=db.rows("SELECT asset_id,state,retry_after FROM media_processing",[])?;
        let running=|a:&Value|rows.iter().any(|r|r["asset_id"]==a["id"] && r["state"]=="running");
        // Retain claimed files first; fill idle slots in title/episode priority order.
        let mut selected:Vec<Value>=assets.iter().filter(|a|running(a)).take(limit).cloned().collect();
        for asset in &assets {
            if selected.len()>=limit {break;}
            if selected.iter().any(|a|a["id"]==asset["id"]) {continue;}
            if rows.iter().any(|r|r["asset_id"]==asset["id"] && r["state"]!="ready" && r["retry_after"].as_f64().unwrap_or(0.0)<=now()) {selected.push(asset.clone());}
        }
        // Claimed files beyond the current limit give their slot back now. Their
        // partial work may be discarded; they restart from their last finished stage.
        let excess:Vec<String>=rows.iter().filter(|r|r["state"]=="running")
            .filter_map(|r|r["asset_id"].as_str()).filter(|id|!selected.iter().any(|a|a["id"]==*id)).map(str::to_owned).collect();
        for id in &excess {db.connection.execute("UPDATE media_processing SET state='queued',updated=? WHERE asset_id=? AND state='running'",rusqlite::params![now(),id])?;}
        (selected,excess)
    };
    let selected_ids:Vec<String>=selected.iter().filter_map(|a|a["id"].as_str().map(str::to_owned)).collect();
    s.subtitle_jobs.yield_assets(&excess,&selected_ids);
    let mut flight=IN_FLIGHT.lock().map_err(|_|anyhow::anyhow!("Processing pool unavailable"))?;
    flight.retain(|id,handle|{
        if handle.is_finished() {return false;}
        if excess.contains(id) {handle.abort();tracing::info!(asset=id,"Processing yielded its slot");return false;}
        true
    });
    // Refill each free slot as soon as it frees up instead of waiting for a batch.
    for asset in selected {
        let Some(id)=asset["id"].as_str().map(str::to_owned) else{continue};
        if flight.contains_key(&id) {continue;}
        let service=s.clone();let task_id=id.clone();
        let handle=tokio::spawn(async move {
            if let Err(error)=process_file(&service,asset).await {tracing::warn!(asset=task_id,%error,"Media processing failed");}
        });
        flight.insert(id,handle.abort_handle());
    }
    Ok(())
}
async fn process_file(s:&Service,mut asset:Value)->Result<()> {
    let id=asset["id"].as_str().context("Missing asset ID")?.to_owned();
    let path=asset["path"].as_str().context("Missing asset path")?.to_owned();
    let revision=crate::subtitle_jobs::revision(Path::new(&path))?;
    let previous={let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        let row=db.rows("SELECT * FROM media_processing WHERE asset_id=?",[&id])?.pop().context("Missing processing record")?;
        db.connection.execute("UPDATE media_processing SET state='running',error=NULL,updated=? WHERE asset_id=?",rusqlite::params![now(),id])?;row};
    asset["retrying"]=json!(previous["attempts"].as_i64().unwrap_or(0)>0);
    let result=advance(s,&asset,&revision,&previous).await;
    if let Err(error)=result {
        let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        fail(&db,&id,&revision,&error.to_string())?;
        tracing::warn!(asset=id,%error,"Media processing needs attention");
    }
    Ok(())
}
async fn advance(s:&Service,asset:&Value,revision:&str,prior:&Value)->Result<()> {
    let id=asset["id"].as_str().context("Missing asset ID")?;
    let path=Path::new(asset["path"].as_str().context("Missing asset path")?);
    let supported=path.extension().and_then(|e|e.to_str()).is_some_and(|e|["mkv","webm","mp4","m4v","mov"].contains(&e.to_lowercase().as_str()));
    anyhow::ensure!(supported,"Unsupported media container for processing");
    if !finished(prior["subtitles_state"].as_str().unwrap_or("")) {
        let progress=crate::media_inventory::process(s,asset).await?;
        let state=progress["state"].as_str().unwrap_or("queued");
        let done=finished(state);
        {let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
            db.connection.execute("UPDATE media_processing SET inventory_state='ready',subtitles_state=?,stage=?,detail=?,updated=? WHERE asset_id=? AND revision=?",rusqlite::params![if done{state}else{"pending"},if done{"credits"}else{progress["stage"].as_str().unwrap_or("subtitles")},progress.to_string(),now(),id,revision])?;
        }
        anyhow::ensure!(state!="failed","{}",progress["error"].as_str().unwrap_or("Subtitle preparation failed"));
        if state=="external" {
            // Waiting on the external translation lane: give the slot to other files.
            let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
            db.connection.execute("UPDATE media_processing SET state='queued',retry_after=?,updated=? WHERE asset_id=? AND revision=?",rusqlite::params![now()+120.0,now(),id,revision])?;
        }
        if !done {return Ok(());}
    }
    if !finished(prior["credits_state"].as_str().unwrap_or("")) {
        let not_applicable=asset["role"]=="extra" || path.extension().is_some_and(|e|e.eq_ignore_ascii_case("webm"));
        {let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
            db.connection.execute("UPDATE media_processing SET stage='credits',detail='{}' WHERE asset_id=? AND revision=?",rusqlite::params![id,revision])?;}
        if !not_applicable {crate::credits::process(s,asset).await?;}
        let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        let credits=if not_applicable {json!({"state":"not_applicable","reason":"Credits analysis does not apply to extras or WebM"})}else{crate::credits::cached(&db,id)?};
        let state=credits["state"].as_str().unwrap_or("pending");
        db.connection.execute("UPDATE media_processing SET stage='credits',credits_state=?,detail=?,updated=? WHERE asset_id=? AND revision=?",rusqlite::params![state,credits.to_string(),now(),id,revision])?;
        anyhow::ensure!(!matches!(state,"failed"|"incomplete"),"{}",credits["error"].as_str().unwrap_or("Credits analysis incomplete"));
        if state=="deferred" {
            db.connection.execute("UPDATE media_processing SET state='queued',retry_after=?,error=?,updated=? WHERE asset_id=?",rusqlite::params![now()+60.0,credits["error"].as_str(),now(),id])?;
        }
        if !finished(state){return Ok(());}
    }
    // File replacement during an async stage invalidates the whole sign-off.
    anyhow::ensure!(crate::subtitle_jobs::revision(path)?==revision,"Media changed during processing");
    let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
    sign_off(&db,id,revision)?;
    Ok(())
}
pub fn summary(db:&Database)->Result<Value> {
    let rows=db.rows("SELECT * FROM media_processing",[])?;
    let mut queue=Vec::new();let mut running=Vec::new();let mut scanned=0;let mut completed=0;let mut failed=0;
    for asset in assets(db)? {
        let id=asset["id"].as_str().unwrap_or("");
        let row=rows.iter().find(|r|r["asset_id"]==id);
        let detail:Value=row.and_then(|r|r["detail"].as_str()).and_then(|v|serde_json::from_str(v).ok()).unwrap_or(json!({}));
        let state=row.and_then(|r|r["state"].as_str()).unwrap_or("queued");
        let stage=row.and_then(|r|r["stage"].as_str()).unwrap_or("inventory");
        let title=asset["path"].as_str().and_then(|p|Path::new(p).file_stem()).and_then(|p|p.to_str()).unwrap_or(id);
        let cues=detail["completed_cues"].as_u64().unwrap_or(0);let total=detail["total_cues"].as_u64().unwrap_or(0);
        if row.is_some_and(|r|r["inventory_state"]=="ready"){scanned+=1;}
        if state=="ready"{completed+=1;}
        if state=="failed"{failed+=1;}
        let item=json!({"id":id,"asset_id":id,"title":title,"state":state,"stage":stage,"target":detail["target"],"error":row.map(|r|r["error"].clone()).unwrap_or(Value::Null),"completed_cues":cues,"total_cues":total,
            "inventory_state":row.map(|r|r["inventory_state"].clone()),"subtitles_state":row.map(|r|r["subtitles_state"].clone()),"credits_state":row.map(|r|r["credits_state"].clone()),"completed_at":row.map(|r|r["completed_at"].clone())});
        if state=="running"{running.push(json!({"asset_id":id,"title":title,"language":detail["target"].as_str().unwrap_or(""),"stage":stage,"completed_cues":cues,"total_cues":total}));}
        queue.push(item);
    }
    Ok(json!({"scanned":scanned,"translated":completed,"completed":completed,"failed":failed,"waiting_for_idle":false,"current":running.first().cloned().unwrap_or(Value::Null),"running":running,"queue":queue}))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn database()->Result<Database>{let db=Database{connection:rusqlite::Connection::open_in_memory()?};db.connection.execute_batch(include_str!("schema.sql"))?;Ok(db)}
    #[test]
    fn failure_history_survives_retries_and_completion()->Result<()> {
        let db=database()?;enroll(&db,"episode","r1")?;
        fail(&db,"episode","r1","decode failed")?;fail(&db,"episode","r1","decode failed")?;
        assert_eq!(db.rows("SELECT occurrences FROM processing_failures",[])?[0]["occurrences"],2);
        enroll(&db,"next","r1")?;
        assert_eq!(db.rows("SELECT state FROM media_processing WHERE asset_id='next'",[])?[0]["state"],"queued");
        db.connection.execute("UPDATE media_processing SET inventory_state='ready',subtitles_state='ready',credits_state='ready' WHERE asset_id='episode'",[])?;
        assert!(sign_off(&db,"episode","r1")?);
        assert!(!db.rows("SELECT resolved_at FROM processing_failures",[])?[0]["resolved_at"].is_null());
        Ok(())
    }
    #[test]
    fn playback_capacity_reserves_resources() {
        assert_eq!(capacity(true,false,false,6),6);
        assert_eq!(capacity(true,false,false,20),8);
        assert_eq!(capacity(true,false,true,6),1);
        assert_eq!(capacity(true,true,false,6),0);
        assert_eq!(capacity(false,false,false,6),0);
    }
    #[test]
    fn all_stages_and_current_revision_are_required_for_sign_off()->Result<()> {
        let db=database()?;enroll(&db,"movie","r1")?;
        db.connection.execute("UPDATE media_processing SET inventory_state='ready',subtitles_state='ready'",[])?;
        assert!(!sign_off(&db,"movie","r1")?); // English/translation alone is insufficient.
        db.connection.execute("UPDATE media_processing SET credits_state='ready'",[])?;
        assert!(!sign_off(&db,"movie","r2")?);
        assert!(sign_off(&db,"movie","r1")?);
        enroll(&db,"movie","r1")?;assert_eq!(db.rows("SELECT state FROM media_processing",[])?[0]["state"],"ready");
        enroll(&db,"movie","r2")?;let row=db.rows("SELECT * FROM media_processing",[])?;
        assert_eq!(row[0]["state"],"queued");assert!(row[0]["completed_at"].is_null());
        Ok(())
    }
    #[test]
    fn extracted_text_roundtrips_for_translation_without_reextracting_video()->Result<()> {
        let cues=vec![crate::subtitles::TimedCue{start_ms:1234,end_ms:4567,text:"Hello & <world>\n你好".into()}];
        let vtt=crate::subtitles::webvtt(&cues);
        let recovered=crate::subtitles::parse_srt(vtt.strip_prefix("WEBVTT\n\n").unwrap())?;
        assert_eq!(recovered[0].start_ms,1234);assert_eq!(recovered[0].end_ms,4567);
        assert_eq!(recovered[0].text,cues[0].text);Ok(())
    }
    #[test]
    fn restart_keeps_stage_progress_and_failures_are_not_finished()->Result<()> {
        let db=database()?;enroll(&db,"episode","r1")?;
        db.connection.execute("UPDATE media_processing SET state='running',inventory_state='ready',subtitles_state='ready',stage='credits'",[])?;
        enroll(&db,"episode","r1")?;let row=db.rows("SELECT * FROM media_processing",[])?;
        assert_eq!(row[0]["stage"],"credits");assert_eq!(row[0]["subtitles_state"],"ready");
        fail(&db,"episode","r1","test failure")?;assert!(!sign_off(&db,"episode","r1")?);
        assert_eq!(db.rows("SELECT state FROM media_processing",[])?[0]["state"],"failed");Ok(())
    }
}
