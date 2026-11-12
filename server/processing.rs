//! SQLite is the authority for queue membership, progress and final sign-off.
//! Subtitle/VTT and credits caches are reusable stage outputs, not separate queues.
use crate::{api::Service, storage::{Database, now}};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::Path;
const VERSION:i64=1;

/// dev:inode:size:mtime:mtime_ns — what a move or rename leaves unchanged.
fn same_file(a:&str,b:&str)->bool {
    let content=|r:&str|r.split(':').take(5).map(str::to_owned).collect::<Vec<_>>();
    a!=b && content(a).len()==5 && content(a)==content(b)
}
/// Finished work follows a file that was only moved or renamed (a package leaving
/// the incomplete folder, the organizer filing it). True when it was carried over.
fn carry_over(db:&Database,id:&str,revision:&str)->Result<bool> {
    use rusqlite::OptionalExtension;
    let prior:Option<String>=db.connection.query_row("SELECT revision FROM media_processing WHERE asset_id=?",[id],|r|r.get(0)).optional()?;
    if !prior.as_deref().is_some_and(|prior|same_file(prior,revision)) {return Ok(false);}
    for table in ["media_processing","media_inventory","credits_analysis"] {
        db.connection.execute(&format!("UPDATE {table} SET revision=?1 WHERE asset_id=?2 AND revision=?3"),rusqlite::params![revision,id,prior])?;
    }
    Ok(true)
}
fn enroll(db:&Database,id:&str,revision:&str)->Result<()> {
    db.connection.execute("INSERT INTO media_processing(asset_id,revision,version,state,stage,inventory_state,subtitles_state,credits_state,updated) VALUES(?,?,?,'queued','inventory','pending','pending','pending',?) ON CONFLICT(asset_id) DO UPDATE SET revision=excluded.revision,version=excluded.version,state='queued',stage='inventory',inventory_state='pending',subtitles_state='pending',credits_state='pending',detail='{}',error=NULL,retry_after=0,attempts=0,completed_at=NULL,updated=excluded.updated WHERE media_processing.revision!=excluded.revision OR media_processing.version!=excluded.version",rusqlite::params![id,revision,VERSION,now()])?;
    Ok(())
}
/// Helper errors that a fresh attempt resolves on its own.
fn transient(error:&str)->bool { error.contains("Text recognition models changed") }
fn finished(value:&str)->bool {matches!(value,"ready"|"not_applicable")}
fn sign_off(db:&Database,id:&str,revision:&str)->Result<bool> {
    // The SQL predicate is deliberately the final authority: a worker becoming
    // idle, an English artifact, or an empty job list can never sign off a file.
    let complete=db.connection.execute("UPDATE media_processing SET state='ready',stage='complete',completed_at=?,updated=?,error=NULL WHERE asset_id=? AND revision=? AND inventory_state='ready' AND subtitles_state IN ('ready','not_applicable') AND credits_state IN ('ready','not_applicable')",rusqlite::params![now(),now(),id,revision])?==1;
    if complete {db.connection.execute("UPDATE processing_failures SET resolved_at=? WHERE asset_id=? AND revision=? AND resolved_at IS NULL",rusqlite::params![now(),id,revision])?;}
    Ok(complete)
}
/// Set when the server begins shutting down, so work its helpers lose is requeued without an error.
pub static SHUTTING_DOWN:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
fn fail(db:&Database,id:&str,revision:&str,error:&str)->Result<()> {
    db.connection.execute("INSERT INTO processing_failures(asset_id,revision,stage,error,first_seen,last_seen) SELECT asset_id,revision,stage,?,?,? FROM media_processing WHERE asset_id=? AND revision=? ON CONFLICT(asset_id,revision,stage,error) DO UPDATE SET occurrences=occurrences+1,last_seen=excluded.last_seen,resolved_at=NULL",rusqlite::params![error,now(),now(),id,revision])?;
    db.connection.execute("UPDATE media_processing SET state='failed',error=?,retry_after=?,updated=?,attempts=attempts+1 WHERE asset_id=? AND revision=?",rusqlite::params![error,now()+3600.0,now(),id,revision])?;Ok(())
}
/// A file with a video extension whose content is no video container (release-group
/// adverts) is not processing work. Record why, and retire its earlier failures
/// so it leaves the failed list. Real videos with a damaged body still fail.
fn not_video(db:&Database,id:&str,revision:&str)->Result<()> {
    db.connection.execute("UPDATE media_processing SET state='ready',stage='complete',inventory_state='not_applicable',subtitles_state='not_applicable',credits_state='not_applicable',detail=?,error=NULL,retry_after=0,completed_at=?,updated=? WHERE asset_id=? AND revision=? AND (state!='ready' OR inventory_state!='not_applicable')",
        rusqlite::params![json!({"state":"not_applicable","reason":"Not a video file"}).to_string(),now(),now(),id,revision])?;
    db.connection.execute("UPDATE processing_failures SET resolved_at=? WHERE asset_id=? AND resolved_at IS NULL",rusqlite::params![now(),id])?;
    Ok(())
}
/// Reopen the credits stage of files analysed by an older credits method, even if
/// they were signed off: a method upgrade should reach the whole library.
pub(crate) fn reopen_outdated(db:&Database)->Result<usize> {
    Ok(db.connection.execute("UPDATE media_processing SET credits_state='pending',stage='credits',state=CASE WHEN state IN ('ready','failed') THEN 'queued' ELSE state END,retry_after=0,attempts=0,completed_at=NULL,error=NULL,updated=? WHERE credits_state IN ('ready','failed','incomplete') AND state!='running' AND asset_id IN (SELECT asset_id FROM credits_analysis WHERE version<?)",rusqlite::params![now(),crate::credits::VERSION])?)
}
/// Files settled with no subtitle source before speech transcription existed
/// (or before it reached a verdict) reopen their subtitle stage once; the
/// transcript record then keeps them settled.
/// Titles transcribed by an older transcript version are redone the same way.
pub(crate) fn reopen_untranscribed(db:&Database,data:&Path)->Result<usize> {
    let transcribed=crate::speech_subtitles::transcribed_assets(data);
    let rows=db.rows("SELECT asset_id,revision,subtitles_state FROM media_processing WHERE inventory_state='ready' AND state!='running' AND subtitles_state IN ('not_applicable','ready')",[])?;
    let rows:Vec<_>=rows.into_iter().filter(|r|r["subtitles_state"]=="not_applicable" || r["asset_id"].as_str().is_some_and(|id|transcribed.contains(id))).collect();
    let mut reopened=0;
    for row in rows {
        let (Some(id),Some(revision))=(row["asset_id"].as_str(),row["revision"].as_str()) else{continue};
        if crate::speech_subtitles::decided(data,id,revision) {continue;}
        reopened+=db.connection.execute("UPDATE media_processing SET subtitles_state='pending',stage='subtitles',state=CASE WHEN state IN ('ready','failed') THEN 'queued' ELSE state END,retry_after=0,completed_at=NULL,error=NULL,updated=? WHERE asset_id=? AND revision=?",rusqlite::params![now(),id,revision])?;
    }
    Ok(reopened)
}
/// Operator rescans. `scope`: "failed" re-queues every failed file now (any
/// stage); with `stage` "credits", "no_credits" reopens credits wherever none were
/// found and "all" reopens credits everywhere. `asset_ids` limits any scope.
pub fn rescan(db:&Database,body:&Value)->Result<Value> {
    let scope=body["scope"].as_str().context("scope must be failed, no_credits or all")?;
    let ids:Option<Vec<String>>=body.get("asset_ids").map(|v|v.as_array().context("asset_ids must be an array").and_then(|a|a.iter().map(|i|i.as_str().map(str::to_owned).context("Invalid asset ID")).collect())).transpose()?;
    let rows=db.rows("SELECT m.asset_id,m.state,c.document AS credits FROM media_processing m LEFT JOIN credits_analysis c ON c.asset_id=m.asset_id",[])?;
    let mut changed=0;
    for row in rows {
        let Some(id)=row["asset_id"].as_str() else{continue};
        if ids.as_ref().is_some_and(|ids|!ids.iter().any(|i|i==id)) || row["state"]=="running" {continue;}
        let credits:Value=row["credits"].as_str().and_then(|t|serde_json::from_str(t).ok()).unwrap_or(json!({}));
        let none_found=credits["state"]=="ready" && credits["segments"].as_array().is_none_or(|s|s.is_empty());
        let sql=match (scope,body["stage"].as_str()) {
            ("failed",_) if row["state"]=="failed" => "UPDATE media_processing SET state='queued',retry_after=0,updated=? WHERE asset_id=?",
            ("no_credits",Some("credits")) if none_found => "UPDATE media_processing SET credits_state='pending',stage='credits',state='queued',retry_after=0,completed_at=NULL,error=NULL,updated=? WHERE asset_id=?",
            ("all",Some("credits")) => "UPDATE media_processing SET credits_state='pending',stage='credits',state='queued',retry_after=0,completed_at=NULL,error=NULL,updated=? WHERE asset_id=?",
            ("failed",_)|("no_credits",Some("credits")) => continue,
            _ => anyhow::bail!("Use scope failed, or stage credits with scope no_credits or all"),
        };
        changed+=db.connection.execute(sql,rusqlite::params![now(),id])?;
        // Mark the cached result outdated so credits actually rerun (as with a
        // method upgrade, players see it as pending until the new result lands).
        if scope!="failed" {db.connection.execute("UPDATE credits_analysis SET version=0 WHERE asset_id=?",[id])?;}
    }
    Ok(json!({"queued":changed}))
}
fn assets(db:&Database)->Result<Vec<Value>> {
    let mut assets=crate::media_inventory::queue_assets(db)?;
    // Finished files of a package still downloading are prepared where they sit.
    for a in &mut assets {
        if a["complete"]==true {continue;}
        if let Some(path)=crate::scanner::work_path(a) {
            a["path"]=json!(path.to_string_lossy());a["available"]=json!(true);a["complete"]=json!(true);a["early"]=json!(true);
        }
    }
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
/// Files whose only remaining work is finishing an imported translation. They
/// run beside the limit: the work is a few seconds of validation and writing.
static FAST:std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>>=std::sync::LazyLock::new(Default::default);
/// Catalog revision and time of the last enrollment pass.
static ENROLLED:std::sync::Mutex<Option<(String,f64)>>=std::sync::Mutex::new(None);

pub async fn tick(s:&std::sync::Arc<Service>)->Result<()> {
    let limit=refresh_capacity(s)?;
    let imported=s.subtitle_jobs.import_external()?;
    let (selected,excess)={
        let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        for id in &imported {
            db.connection.execute("UPDATE media_processing SET retry_after=0 WHERE asset_id=? AND state='queued'",[id])?;
            let credits_done:bool=db.connection.query_row("SELECT EXISTS(SELECT 1 FROM media_processing WHERE asset_id=? AND credits_state IN ('ready','not_applicable'))",[id],|r|r.get(0))?;
            if credits_done {if let Ok(mut fast)=FAST.lock() {fast.insert(id.clone());}}
        }
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
                // An early file that has just moved keeps its work until the next scan finds it.
                if revision.is_err() && a["early"]==true {continue;}
                if let Ok(revision)=&revision {
                    if carry_over(&db,id,revision)? {
                        let moved=s.subtitle_jobs.relocate(id,Path::new(path));
                        tracing::info!(asset=id,jobs=moved,"Processing followed a moved file");
                    }
                }
                enroll(&db,id,revision.as_deref().unwrap_or("unavailable"))?;
                if let Err(error)=revision {fail(&db,id,"unavailable",&error.to_string())?;}
            }
            // The scanner hides non-video files from the library; settle their records.
            for a in db.documents("assets")?.iter().filter(|a|crate::container::tagged_not_media(a) && a["available"]==true) {
                let (Some(id),Some(path))=(a["id"].as_str(),a["path"].as_str()) else{continue};
                let Ok(revision)=crate::subtitle_jobs::revision(Path::new(path)) else{continue};
                enroll(&db,id,&revision)?;
                not_video(&db,id,&revision)?;
            }
            reopen_outdated(&db)?;
            reopen_untranscribed(&db,&s.data_dir)?;
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
        let fast:std::collections::HashSet<String>={
            let mut fast=FAST.lock().map_err(|_|anyhow::anyhow!("Processing pool unavailable"))?;
            fast.retain(|id|rows.iter().any(|r|r["asset_id"]==id.as_str() && matches!(r["state"].as_str(),Some("queued"|"running"))));
            if limit==0 {Default::default()} else {fast.clone()}
        };
        let is_fast=|a:&Value|a["id"].as_str().is_some_and(|id|fast.contains(id));
        let running=|a:&Value|!is_fast(a) && rows.iter().any(|r|r["asset_id"]==a["id"] && r["state"]=="running");
        // Retain claimed files first; fill idle slots in title/episode priority order.
        let mut selected:Vec<Value>=assets.iter().filter(|a|running(a)).take(limit).cloned().collect();
        for asset in &assets {
            if selected.len()>=limit {break;}
            if is_fast(asset) || selected.iter().any(|a|a["id"]==asset["id"]) {continue;}
            if rows.iter().any(|r|r["asset_id"]==asset["id"] && r["state"]!="ready" && r["retry_after"].as_f64().unwrap_or(0.0)<=now()) {selected.push(asset.clone());}
        }
        // Claimed files beyond the current limit give their slot back now. Their
        // partial work may be discarded; they restart from their last finished stage.
        selected.extend(assets.iter().filter(|a|is_fast(a)).cloned());
        let excess:Vec<String>=rows.iter().filter(|r|r["state"]=="running")
            .filter_map(|r|r["asset_id"].as_str()).filter(|id|!selected.iter().any(|a|a["id"]==*id)).map(str::to_owned).collect();
        for id in &excess {db.connection.execute("UPDATE media_processing SET state='queued',updated=? WHERE asset_id=? AND state='running'",rusqlite::params![now(),id])?;}
        (selected,excess)
    };
    let selected_ids:Vec<String>=selected.iter().filter_map(|a|a["id"].as_str().map(str::to_owned)).collect();
    s.subtitle_jobs.yield_assets(&excess,&selected_ids);
    let idle={
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
    flight.is_empty()
    };
    if idle && limit>0 {warm_playback_index(s).await?;}
    Ok(())
}
/// Files whose playback index was rebuilt (or tried) in this run, and when the
/// last warm-up pass ran.
static WARMED:std::sync::LazyLock<std::sync::Mutex<(std::collections::HashSet<String>,f64)>>=std::sync::LazyLock::new(Default::default);
/// Processed Matroska files can lose their playback index to the cache's space
/// reclaim, and then the next play waits while the whole file is re-read. While
/// nothing else is running, rebuild one missing index at a time in queue order.
async fn warm_playback_index(s:&Service)->Result<()> {
    {let mut warmed=WARMED.lock().map_err(|_|anyhow::anyhow!("Processing pool unavailable"))?;
        if now()-warmed.1<60.0 {return Ok(());}
        warmed.1=now();}
    if !s.virtual_media.idle() || !s.virtual_media.can_preindex() || playback_active(s)? {return Ok(());}
    let candidates:Vec<Value>={
        let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        let busy:bool=db.connection.query_row("SELECT EXISTS(SELECT 1 FROM media_processing WHERE state='running')",[],|r|r.get(0))?;
        if busy {return Ok(());}
        let ready:std::collections::HashSet<String>=db.rows("SELECT asset_id FROM media_processing WHERE state='ready'",[])?
            .iter().filter_map(|r|r["asset_id"].as_str().map(str::to_owned)).collect();
        assets(&db)?.into_iter().filter(|a|a["role"]!="extra" && a["id"].as_str().is_some_and(|id|ready.contains(id))).collect()
    };
    for asset in candidates {
        let Some(id)=asset["id"].as_str() else{continue};
        let Some(path)=asset["path"].as_str() else{continue};
        let path=Path::new(path);
        let Ok(revision)=crate::subtitle_jobs::revision(path) else{continue};
        let key=format!("{id}:{revision}");
        if WARMED.lock().is_ok_and(|w|w.0.contains(&key)) || !crate::container::is_matroska(path) {continue;}
        if asset["early"]==true {continue;}
        let Ok(input)=crate::api::media_file(s,id,0).await else{continue};
        if s.virtual_media.indexed(id,&input) {continue;}
        if let Ok(mut warmed)=WARMED.lock() {warmed.0.insert(key);}
        match s.virtual_media.prepare_analysis(id,input) {
            Ok(_)=>tracing::info!(asset=id,"Rebuilding a missing playback index"),
            Err(error)=>tracing::warn!(asset=id,%error,"Playback index warm-up failed"),
        }
        return Ok(());
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
        let message=error.to_string();
        if SHUTTING_DOWN.load(std::sync::atomic::Ordering::SeqCst) {
            // The server is stopping and took the helpers with it: not the file's fault.
            db.connection.execute("UPDATE media_processing SET state='queued',retry_after=0,updated=? WHERE asset_id=?",rusqlite::params![now(),id])?;
            tracing::info!(asset=id,"Processing stopped for shutdown");
        } else if !Path::new(&path).exists() {
            // Moved mid-stage (its package finished, or it was filed): pick it up at its new path.
            db.connection.execute("UPDATE media_processing SET state='queued',retry_after=?,updated=? WHERE asset_id=?",rusqlite::params![now()+15.0,now(),id])?;
            tracing::info!(asset=id,"Processing paused while its file moves");
        } else if transient(&message) {
            // Another helper replaced the shared Vision models mid-job. The next
            // attempt re-warms them under the helpers' lock; it is not a failure.
            db.connection.execute("UPDATE media_processing SET state='queued',retry_after=?,attempts=attempts+1,error=?,updated=? WHERE asset_id=? AND revision=?",rusqlite::params![now()+60.0,message,now(),id,revision])?;
            tracing::info!(asset=id,"Processing retrying after a shared model change");
        } else {
            fail(&db,&id,&revision,&message)?;
            tracing::warn!(asset=id,%error,"Media processing needs attention");
        }
    }
    Ok(())
}
async fn advance(s:&Service,asset:&Value,revision:&str,prior:&Value)->Result<()> {
    let id=asset["id"].as_str().context("Missing asset ID")?;
    let path=Path::new(asset["path"].as_str().context("Missing asset path")?);
    // By content, not extension: Matroska named .mp4 goes through the Matroska
    // readers, and adverts named .mkv/.mp4 are not processing work at all.
    use crate::container::Container;
    let container=crate::container::sniff(path).context("Torrbaby Server cannot read this media file")?;
    if container==Container::NotMedia {
        let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        return not_video(&db,id,revision);
    }
    anyhow::ensure!(matches!(container,Container::Matroska|Container::Mp4),"Unsupported media container for processing");
    let mut external=false;
    if !finished(prior["subtitles_state"].as_str().unwrap_or("")) {
        let progress=crate::media_inventory::process(s,asset).await?;
        let state=progress["state"].as_str().unwrap_or("queued");
        let done=finished(state);
        {let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
            db.connection.execute("UPDATE media_processing SET inventory_state='ready',subtitles_state=?,stage=?,detail=?,updated=? WHERE asset_id=? AND revision=?",rusqlite::params![if done{state}else{"pending"},if done{"credits"}else{progress["stage"].as_str().unwrap_or("subtitles")},progress.to_string(),now(),id,revision])?;
        }
        anyhow::ensure!(state!="failed","{}",progress["error"].as_str().unwrap_or("Subtitle preparation failed"));
        // While the external lane translates, use this slot for credits analysis.
        external=state=="external";
        if !done && !external {return Ok(());}
    }
    if !finished(prior["credits_state"].as_str().unwrap_or("")) {
        // Specials list as extras (except a watch-first pilot), so no Next Episode offer needs their credits.
        let special=asset["season"]==0 && asset["watch_first"]!=true;
        let not_applicable=asset["role"]=="extra" || special || path.extension().is_some_and(|e|e.eq_ignore_ascii_case("webm"));
        {let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
            db.connection.execute("UPDATE media_processing SET stage='credits',detail='{}' WHERE asset_id=? AND revision=?",rusqlite::params![id,revision])?;}
        if !not_applicable {crate::credits::process(s,asset).await?;}
        let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        let credits=if not_applicable {json!({"state":"not_applicable","reason":"Credits analysis does not apply to extras, specials or WebM"})}else{crate::credits::cached(&db,id)?};
        let state=credits["state"].as_str().unwrap_or("pending");
        db.connection.execute("UPDATE media_processing SET stage='credits',credits_state=?,detail=?,updated=? WHERE asset_id=? AND revision=?",rusqlite::params![state,credits.to_string(),now(),id,revision])?;
        anyhow::ensure!(!matches!(state,"failed"|"incomplete"),"{}",credits["error"].as_str().unwrap_or("Credits analysis incomplete"));
        if state=="deferred" {
            db.connection.execute("UPDATE media_processing SET state='queued',retry_after=?,error=?,updated=? WHERE asset_id=?",rusqlite::params![now()+60.0,credits["error"].as_str(),now(),id])?;
        }
        if !finished(state){return Ok(());}
    }
    if external {
        // Only the returned translation is missing: release the slot until the
        // import arrives (which resets retry_after and uses the fast lane).
        let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        db.connection.execute("UPDATE media_processing SET state='queued',retry_after=?,updated=? WHERE asset_id=? AND revision=?",rusqlite::params![now()+1800.0,now(),id,revision])?;
        return Ok(());
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
        let item=json!({"id":id,"asset_id":id,"title":title,"state":state,"stage":stage,"target":detail["target"],"error":row.map(|r|r["error"].clone()).unwrap_or(Value::Null),"completed_cues":cues,"total_cues":total,"method":detail["extraction_method"],
            "inventory_state":row.map(|r|r["inventory_state"].clone()),"subtitles_state":row.map(|r|r["subtitles_state"].clone()),"credits_state":row.map(|r|r["credits_state"].clone()),"completed_at":row.map(|r|r["completed_at"].clone())});
        if state=="running"{running.push(json!({"asset_id":id,"title":title,"language":detail["target"].as_str().unwrap_or(""),"stage":stage,"method":detail["extraction_method"],"completed_cues":cues,"total_cues":total}));}
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
    fn finished_work_follows_a_moved_file_but_not_a_changed_one()->Result<()> {
        let db=database()?;
        enroll(&db,"a","1:2:300:400:5:600:7")?;
        db.connection.execute("UPDATE media_processing SET state='ready',stage='complete',inventory_state='ready',subtitles_state='ready',credits_state='ready'",[])?;
        db.connection.execute("INSERT INTO media_inventory VALUES('a','1:2:300:400:5:600:7','{}',0)",[])?;
        // Renamed: only the change time differs.
        assert!(carry_over(&db,"a","1:2:300:400:5:999:9")?);
        enroll(&db,"a","1:2:300:400:5:999:9")?;
        let row=db.rows("SELECT state,revision FROM media_processing",[])?;
        assert_eq!((row[0]["state"].as_str(),row[0]["revision"].as_str()),(Some("ready"),Some("1:2:300:400:5:999:9")));
        assert_eq!(db.rows("SELECT revision FROM media_inventory",[])?[0]["revision"],"1:2:300:400:5:999:9");
        // Rewritten: a new size starts over.
        assert!(!carry_over(&db,"a","1:2:301:400:5:999:9")?);
        enroll(&db,"a","1:2:301:400:5:999:9")?;
        assert_eq!(db.rows("SELECT state FROM media_processing",[])?[0]["state"],"queued");
        assert!(!carry_over(&db,"new","1:2:301:400:5:999:9")?);
        Ok(())
    }
    #[test]
    fn non_video_files_settle_as_not_applicable_and_leave_the_queue()->Result<()> {
        let db=database()?;
        for (id,container) in [("advert","not_media"),("movie","matroska")] {
            db.connection.execute("INSERT INTO library_assets VALUES(?,?)",rusqlite::params![id,json!({"id":id,"path":format!("/fixture/{id}.mkv"),"available":true,"complete":true,"container":container}).to_string()])?;
            enroll(&db,id,"r1")?;
            fail(&db,id,"r1","Native text subtitle extraction failed")?;
        }
        not_video(&db,"advert","r1")?;
        let row=db.rows("SELECT * FROM media_processing WHERE asset_id='advert'",[])?.remove(0);
        assert_eq!((row["state"].as_str(),row["inventory_state"].as_str(),row["credits_state"].as_str()),(Some("ready"),Some("not_applicable"),Some("not_applicable")));
        assert_eq!(serde_json::from_str::<Value>(row["detail"].as_str().unwrap())?["reason"],"Not a video file");
        // Only the advert's failure is retired; the real (damaged) video still fails visibly.
        let open=db.rows("SELECT asset_id FROM processing_failures WHERE resolved_at IS NULL",[])?;
        assert_eq!(open.len(),1);assert_eq!(open[0]["asset_id"],"movie");
        let queue=summary(&db)?;
        assert_eq!(queue["failed"],1);
        assert!(queue["queue"].as_array().unwrap().iter().all(|a|a["asset_id"]=="movie"));
        Ok(())
    }
    #[test]
    fn method_upgrades_and_rescans_reopen_only_the_credits_stage()->Result<()> {
        let db=database()?;
        for id in ["old","current","failed","busy"] {enroll(&db,id,"r1")?;}
        db.connection.execute("UPDATE media_processing SET state='ready',inventory_state='ready',subtitles_state='ready',credits_state='ready',stage='complete',completed_at=1",[])?;
        db.connection.execute("UPDATE media_processing SET state='failed',credits_state='incomplete' WHERE asset_id='failed'",[])?;
        db.connection.execute("UPDATE media_processing SET state='running' WHERE asset_id='busy'",[])?;
        for (id,version,doc) in [("old",0,r#"{"state":"ready","segments":[]}"#),("current",crate::credits::VERSION,r#"{"state":"ready","segments":[{"start":1}]}"#),("failed",0,r#"{"state":"incomplete"}"#),("busy",0,r#"{"state":"ready","segments":[]}"#)] {
            db.connection.execute("INSERT INTO credits_analysis VALUES(?,?,?,?,0)",rusqlite::params![id,"r1",version,doc])?;
        }
        assert_eq!(reopen_outdated(&db)?,2);
        let state=|id:&str|->Result<(String,String,String)>{Ok(db.connection.query_row("SELECT state,credits_state,subtitles_state FROM media_processing WHERE asset_id=?",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?)};
        assert_eq!(state("old")?,("queued".into(),"pending".into(),"ready".into()));
        assert_eq!(state("failed")?.0,"queued");
        assert_eq!(state("current")?.0,"ready");
        assert_eq!(state("busy")?.0,"running","running files are left to finish");
        assert_eq!(rescan(&db,&json!({"scope":"no_credits","stage":"credits"}))?["queued"],1,"only `old` (none found); busy is running, current has credits");
        assert_eq!(rescan(&db,&json!({"scope":"all","stage":"credits","asset_ids":["current"]}))?["queued"],1);
        assert_eq!(state("current")?,("queued".into(),"pending".into(),"ready".into()));
        let version:i64=db.connection.query_row("SELECT version FROM credits_analysis WHERE asset_id='current'",[],|r|r.get(0))?;
        assert_eq!(version,0,"cached result must not short-circuit the rescan");
        assert!(rescan(&db,&json!({"scope":"bogus"})).is_err());
        Ok(())
    }
    #[test]
    fn files_without_subtitles_reopen_once_for_transcription()->Result<()> {
        let db=database()?;
        let data=std::env::temp_dir().join(format!("torrbaby-transcribe-{}",uuid::Uuid::new_v4()));
        for id in ["silent","advert","subtitled"] {enroll(&db,id,"r1")?;}
        db.connection.execute("UPDATE media_processing SET state='ready',stage='complete',inventory_state='ready',subtitles_state='not_applicable',credits_state='ready',completed_at=1",[])?;
        db.connection.execute("UPDATE media_processing SET inventory_state='not_applicable' WHERE asset_id='advert'",[])?;
        db.connection.execute("UPDATE media_processing SET subtitles_state='ready' WHERE asset_id='subtitled'",[])?;
        assert_eq!(reopen_untranscribed(&db,&data)?,1);
        let row=db.rows("SELECT state,subtitles_state,credits_state FROM media_processing WHERE asset_id='silent'",[])?.remove(0);
        assert_eq!((row["state"].as_str(),row["subtitles_state"].as_str(),row["credits_state"].as_str()),(Some("queued"),Some("pending"),Some("ready")));
        // Once transcription reaches a verdict the file stays settled.
        db.connection.execute("UPDATE media_processing SET state='ready',subtitles_state='not_applicable' WHERE asset_id='silent'",[])?;
        std::fs::create_dir_all(crate::speech_subtitles::directory(&data))?;
        std::fs::write(crate::speech_subtitles::record(&data,"silent","r1"),r#"{"state":"rejected"}"#)?;
        assert_eq!(reopen_untranscribed(&db,&data)?,0);
        // A finished title transcribed by an older version is redone once.
        std::fs::write(crate::speech_subtitles::directory(&data).join("old.json"),r#"{"state":"ready","asset_id":"subtitled"}"#)?;
        assert_eq!(reopen_untranscribed(&db,&data)?,1);
        std::fs::write(crate::speech_subtitles::record(&data,"subtitled","r1"),r#"{"state":"ready","asset_id":"subtitled"}"#)?;
        db.connection.execute("UPDATE media_processing SET state='ready',subtitles_state='ready' WHERE asset_id='subtitled'",[])?;
        assert_eq!(reopen_untranscribed(&db,&data)?,0);
        std::fs::remove_dir_all(data)?;
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
