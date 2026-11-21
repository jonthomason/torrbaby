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
/// A file was copied to another drive and the library now points at the copy: its
/// finished work (subtitles, language inventory, credits) belongs to the copy.
pub fn follow(db:&Database,id:&str,revision:&str)->Result<()> {
    use rusqlite::OptionalExtension;
    let Some(prior):Option<String>=db.connection.query_row("SELECT revision FROM media_processing WHERE asset_id=?",[id],|r|r.get(0)).optional()? else {return Ok(())};
    for table in ["media_processing","media_inventory","credits_analysis"] {
        db.connection.execute(&format!("UPDATE {table} SET revision=?1 WHERE asset_id=?2 AND revision=?3"),rusqlite::params![revision,id,prior])?;
    }
    db.connection.execute("UPDATE processing_failures SET revision=?1 WHERE asset_id=?2 AND revision=?3",rusqlite::params![revision,id,prior])?;
    Ok(())
}
fn enroll(db:&Database,id:&str,revision:&str)->Result<()> {
    db.connection.execute("INSERT INTO media_processing(asset_id,revision,version,state,stage,inventory_state,subtitles_state,credits_state,updated) VALUES(?,?,?,'queued','inventory','pending','pending','pending',?) ON CONFLICT(asset_id) DO UPDATE SET revision=excluded.revision,version=excluded.version,state='queued',stage='inventory',inventory_state='pending',subtitles_state='pending',credits_state='pending',detail='{}',error=NULL,retry_after=0,attempts=0,completed_at=NULL,updated=excluded.updated WHERE media_processing.revision!=excluded.revision OR media_processing.version!=excluded.version",rusqlite::params![id,revision,VERSION,now()])?;
    Ok(())
}
/// Helper errors that a fresh attempt resolves on its own.
/// "Media is unavailable or changed": the file was replaced and the scan has not caught up.
/// "Download is not complete": its torrent is still downloading; the file is read once it finishes.
fn transient(error:&str)->bool { error.contains("Text recognition models changed") || error.contains(MEDIA_CHANGED) || error.contains(DOWNLOAD_INCOMPLETE) }
const MEDIA_CHANGED:&str="Media is unavailable or changed";
const DOWNLOAD_INCOMPLETE:&str="Download is not complete";
/// Written in the last ten minutes: a failure may be the file still arriving (four episodes
/// of 逐玉 were read mid-copy and skipped for good with "integers can be at most 8 bytes").
/// Retried, not skipped.
fn settling(path:&Path)->bool {
    std::fs::metadata(path).and_then(|m|m.modified()).ok().and_then(|t|t.elapsed().ok()).is_some_and(|age|age<std::time::Duration::from_secs(600))
}
/// Files whose subtitle stage was skipped because the file had just been replaced (that was
/// not retried before), so no language inventory was ever saved: do the stage again.
fn reopen_replaced(db:&Database)->Result<usize> {
    Ok(db.connection.execute("UPDATE media_processing SET subtitles_state='pending',stage='subtitles',state='queued',retry_after=0,completed_at=NULL,error=NULL,updated=?1 \
        WHERE state!='running' AND subtitles_state='skipped' AND json_extract(detail,'$.error')=?2 \
        AND NOT EXISTS(SELECT 1 FROM media_inventory i WHERE i.asset_id=media_processing.asset_id AND i.revision=media_processing.revision)",rusqlite::params![now(),MEDIA_CHANGED])?)
}
/// Titles whose English the server made from text it read (bitmap subtitles, or Chinese read
/// from the picture and translated) and that OpenSubtitles has not been asked about: done
/// again once, so its English replaces them (the user's rule, 2026-11-15). Each waits behind
/// titles that have no subtitles.
fn reopen_for_people_english(db:&Database,flags:Flags)->Result<usize> {
    if !flags.read_english {return Ok(0);}
    let rows=db.rows("SELECT m.asset_id,m.revision,i.document FROM media_processing m JOIN media_inventory i ON i.asset_id=m.asset_id AND i.revision=m.revision LEFT JOIN library_assets a ON a.id=m.asset_id WHERE m.state='ready' AND m.subtitles_state='ready' AND coalesce(json_extract(a.document,'$.role'),'')!='extra'",[])?;
    let mut reopened=0;
    for row in rows {
        let (Some(id),Some(revision),Some(document))=(row["asset_id"].as_str(),row["revision"].as_str(),row["document"].as_str()) else{continue};
        let Ok(info)=serde_json::from_str::<Value>(document) else{continue};
        let read=info["prepared"].as_array().is_some_and(|jobs|jobs.iter().any(|j|j["state"]=="ready" && j["language"]=="en" && j["extraction_method"]=="ocr"));
        if !read || crate::speech_subtitles::people_decided(db,id,revision,"en") {continue;}
        let changed=db.connection.execute("UPDATE media_processing SET subtitles_state='pending',stage='subtitles',state='queued',retry_after=0,completed_at=NULL,error=NULL,updated=? WHERE asset_id=? AND revision=?",rusqlite::params![now(),id,revision])?;
        if changed>0 {db.connection.execute("INSERT INTO subtitle_priorities(asset_id,priority) VALUES(?,-1) ON CONFLICT(asset_id) DO NOTHING",[id])?;}
        reopened+=changed;
    }
    if reopened>0 {tracing::info!(reopened,"Titles with English read from text will look for English from OpenSubtitles");}
    Ok(reopened)
}
/// Titles whose English came from OpenSubtitles and whose Chinese was translated on device
/// while Chinese downloads were off (Parasite): done again once, so OpenSubtitles' Chinese
/// replaces the translation when it has some.
fn reopen_for_people_chinese(db:&Database,flags:Flags)->Result<usize> {
    if !flags.chinese {return Ok(0);}
    let rows=db.rows("SELECT m.asset_id,m.revision,i.document,json_extract(a.document,'$.season') AS season FROM media_processing m JOIN media_inventory i ON i.asset_id=m.asset_id AND i.revision=m.revision LEFT JOIN library_assets a ON a.id=m.asset_id WHERE m.state='ready' AND m.subtitles_state='ready' AND coalesce(json_extract(a.document,'$.role'),'')!='extra'",[])?;
    let mut reopened=0;
    for row in rows {
        let (Some(id),Some(revision),Some(document))=(row["asset_id"].as_str(),row["revision"].as_str(),row["document"].as_str()) else{continue};
        let Ok(info)=serde_json::from_str::<Value>(document) else{continue};
        let people=info["prepared"].as_array().is_some_and(|jobs|jobs.iter().any(|j|j["state"]=="ready" && j["language"]=="en" && j["extraction_method"]=="opensubtitles"));
        if !people || crate::speech_subtitles::people_settled(db,flags.everything,id,revision,"zh",row["season"]==0) {continue;}
        reopened+=db.connection.execute("UPDATE media_processing SET subtitles_state='pending',stage='subtitles',state='queued',retry_after=0,completed_at=NULL,error=NULL,updated=? WHERE asset_id=? AND revision=?",rusqlite::params![now(),id,revision])?;
    }
    if reopened>0 {tracing::info!(reopened,"Titles with English from OpenSubtitles will look for its Chinese");}
    Ok(reopened)
}
/// While every title is to take OpenSubtitles' subtitles: each title not yet looked up in
/// both languages is done again, behind titles that have none.
fn reopen_for_everything(db:&Database,flags:Flags)->Result<usize> {
    if !flags.everything {return Ok(0);}
    let rows=db.rows("SELECT m.asset_id,m.revision,json_extract(a.document,'$.season') AS season FROM media_processing m LEFT JOIN library_assets a ON a.id=m.asset_id WHERE m.state='ready' AND m.subtitles_state IN ('ready','not_applicable','skipped') AND m.inventory_state='ready' AND coalesce(json_extract(a.document,'$.role'),'')!='extra'",[])?;
    let mut reopened=0;
    for row in rows {
        let (Some(id),Some(revision))=(row["asset_id"].as_str(),row["revision"].as_str()) else{continue};
        if ["en","zh"].iter().all(|tag|crate::speech_subtitles::people_settled(db,true,id,revision,tag,row["season"]==0)) {continue;}
        let changed=db.connection.execute("UPDATE media_processing SET subtitles_state='pending',stage='subtitles',state='queued',retry_after=0,completed_at=NULL,error=NULL,updated=? WHERE asset_id=? AND revision=?",rusqlite::params![now(),id,revision])?;
        if changed>0 {db.connection.execute("INSERT INTO subtitle_priorities(asset_id,priority) VALUES(?,-1) ON CONFLICT(asset_id) DO NOTHING",[id])?;}
        reopened+=changed;
    }
    if reopened>0 {tracing::info!(reopened,"Every title will take OpenSubtitles' English and Chinese where it has them");}
    Ok(reopened)
}
/// Finished titles (not bonus material) with subtitles in one of English and Chinese only:
/// done again, behind titles that have none. Files with no subtitle tracks of their own were
/// signed off with OpenSubtitles' English alone, the Chinese they had dropped (216 titles,
/// 2026-11-21); a pass now keeps or makes the other language.
fn reopen_one_language(db:&Database)->Result<usize> {
    let rows=db.rows("SELECT m.asset_id,m.revision,i.document FROM media_processing m JOIN media_inventory i ON i.asset_id=m.asset_id AND i.revision=m.revision LEFT JOIN library_assets a ON a.id=m.asset_id WHERE m.state='ready' AND m.subtitles_state='ready' AND coalesce(json_extract(a.document,'$.role'),'')!='extra'",[])?;
    let mut reopened=0;
    for row in rows {
        let (Some(id),Some(revision),Some(document))=(row["asset_id"].as_str(),row["revision"].as_str(),row["document"].as_str()) else{continue};
        let Ok(info)=serde_json::from_str::<Value>(document) else{continue};
        let has=|language:&str|info["prepared"].as_array().is_some_and(|jobs|jobs.iter().any(|j|j["state"]=="ready" && j["language"]==language));
        if has("en")==has("zh-Hans") {continue;}
        let changed=db.connection.execute("UPDATE media_processing SET subtitles_state='pending',stage='subtitles',state='queued',retry_after=0,completed_at=NULL,error=NULL,updated=? WHERE asset_id=? AND revision=?",rusqlite::params![now(),id,revision])?;
        if changed>0 {db.connection.execute("INSERT INTO subtitle_priorities(asset_id,priority) VALUES(?,-1) ON CONFLICT(asset_id) DO NOTHING",[id])?;}
        reopened+=changed;
    }
    if reopened>0 {tracing::warn!(reopened,"Titles with subtitles in one language only will make the other");}
    Ok(reopened)
}
/// Subtitle errors a later build no longer has: the subtitle store's old 4,096-file cap, and
/// a subtitle track in neither English nor Chinese blocking the picture and the speech.
const RETIRED_SUBTITLE_ERRORS:[&str;5]=[crate::subtitle_jobs::STORAGE_FULL,crate::ocr_subtitles::UNIDENTIFIED_LANGUAGE,
    // A file read before its download finished (Outlander S04E13 kept no Chinese): now retried.
    DOWNLOAD_INCOMPLETE,
    // The Matroska reader on a file still being copied in (see `settling`).
    "integers can be at most 8 bytes%",
    // A bitmap track in a large file, cut off by the scan's old fixed three minutes.
    "Bitmap subtitles could not be decoded (Bitmap scan timed out)%"];
/// Files whose subtitles were skipped for one of those errors: prepare them again.
fn reopen_storage_full(db:&Database)->Result<usize> {
    let mut reopened=0;
    for error in RETIRED_SUBTITLE_ERRORS {
        reopened+=db.connection.execute("UPDATE media_processing SET subtitles_state='pending',stage='subtitles',state='queued',retry_after=0,completed_at=NULL,error=NULL,updated=?1 \
        WHERE state!='running' AND subtitles_state='skipped' AND (json_extract(detail,'$.error') LIKE ?2 \
          OR (EXISTS(SELECT 1 FROM processing_failures f WHERE f.asset_id=media_processing.asset_id AND f.stage='subtitles_skipped' AND f.error LIKE ?2) \
            AND NOT EXISTS(SELECT 1 FROM processing_failures f WHERE f.asset_id=media_processing.asset_id AND f.stage='subtitles_skipped' AND f.error NOT LIKE ?2 AND f.resolved_at IS NULL)))",
        rusqlite::params![now(),error])?;
    }
    if reopened>0 {tracing::info!(reopened,"Subtitles skipped for an error since fixed will be prepared again");}
    Ok(reopened)
}
/// The OpenSubtitles switches the reopening sweeps depend on.
#[derive(Clone,Copy,PartialEq,Eq,Debug)]
pub(crate) struct Flags { read_english:bool, chinese:bool, everything:bool }
impl Flags {
    fn read(data:&Path)->Self { Self { read_english:crate::opensubtitles::read_english(data), chinese:crate::opensubtitles::chinese(data), everything:crate::opensubtitles::everything(data) } }
}
/// The sweeps that look through every finished file's subtitle records for one to do again.
/// A record only changes when processing itself writes it, so they run at start, when the
/// OpenSubtitles switches change, and when the earliest answer that said "ask again later"
/// comes due (they ran every 2 s, reading thousands of record files, because processing's
/// own writes looked like library changes).
fn sweep(db:&Database,flags:Flags)->Result<()> {
    reopen_for_people_english(db,flags)?;
    reopen_for_people_chinese(db,flags)?;
    reopen_for_everything(db,flags)?;
    reopen_one_language(db)?;
    reopen_untranscribed(db)?;
    Ok(())
}
/// The switches the last sweep ran with, and when the next "later" answer comes due.
static SWEPT:std::sync::Mutex<Option<(Flags,Option<f64>)>>=std::sync::Mutex::new(None);
/// When the earliest OpenSubtitles "later" (a record's `after`, a transcript's `opensubtitles_after`) is due.
fn next_later(db:&Database)->Result<Option<f64>> {
    Ok(db.connection.query_row("SELECT min(t) FROM (SELECT json_extract(document,'$.after') AS t FROM subtitle_records WHERE json_extract(document,'$.state')='later' \
        UNION ALL SELECT json_extract(document,'$.opensubtitles_after') FROM subtitle_records WHERE kind='transcript') WHERE t>?",[now()],|r|r.get(0))?)
}
/// When a processing worker must look again although nothing woke it: a retry comes due,
/// playback's heartbeat goes stale (its slots come back), or an OpenSubtitles "later" is due.
fn next_deadline(db:&Database)->Result<Option<std::time::Instant>> {
    let retry:Option<f64>=db.connection.query_row("SELECT min(retry_after) FROM media_processing WHERE state='queued' AND retry_after>?",[now()],|r|r.get(0))?;
    let playing:Option<f64>=db.connection.query_row("SELECT max(p.last_played) FROM profile_playback p JOIN playback_sessions s ON p.active_session=s.id WHERE s.ended=0 AND p.last_played>?",[now()-90.0],|r|r.get(0))?;
    let later=SWEPT.lock().ok().and_then(|w|w.as_ref().and_then(|(_,t)|*t));
    Ok([retry,playing.map(|t|t+90.0),later].into_iter().flatten().map(crate::wake::at_epoch).min())
}
/// `skipped`: credits analysis failed. It is optional, so the file finishes without it.
fn finished(value:&str)->bool {matches!(value,"ready"|"not_applicable"|"skipped")}
fn sign_off(db:&Database,id:&str,revision:&str)->Result<bool> {
    // The SQL predicate is deliberately the final authority: a worker becoming
    // idle, an English artifact, or an empty job list can never sign off a file.
    let complete=db.connection.execute("UPDATE media_processing SET state='ready',stage='complete',completed_at=?,updated=?,error=NULL WHERE asset_id=? AND revision=? AND inventory_state='ready' AND subtitles_state IN ('ready','not_applicable','skipped') AND credits_state IN ('ready','not_applicable','skipped')",rusqlite::params![now(),now(),id,revision])?==1;
    // Skipped stages stay on record (unresolved) so their errors can be mined for fixes.
    if complete {
        // A skipped stage stays on record while it is still skipped; once it succeeds it is resolved too.
        db.connection.execute("UPDATE processing_failures SET resolved_at=?,resolution='completed' WHERE asset_id=? AND revision=? AND resolved_at IS NULL \
            AND (stage NOT LIKE '%_skipped' \
              OR (stage='subtitles_skipped' AND (SELECT subtitles_state FROM media_processing WHERE asset_id=processing_failures.asset_id)!='skipped') \
              OR (stage='credits_skipped' AND (SELECT credits_state FROM media_processing WHERE asset_id=processing_failures.asset_id)!='skipped'))",rusqlite::params![now(),id,revision])?;
    }
    Ok(complete)
}
/// Set when the server begins shutting down, so work its helpers lose is requeued without an error.
pub static SHUTTING_DOWN:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
/// Unresolved processing problems the user has not marked as seen, newest first: errors,
/// and warnings for optional stages that were skipped. Files no longer in the library drop out.
pub fn attention(db:&Database)->Result<Value> {
    let rows=db.rows("SELECT f.rowid AS id,f.asset_id,f.stage,f.error,f.occurrences,f.first_seen,f.last_seen,json_extract(a.document,'$.path') AS path \
        FROM processing_failures f JOIN library_assets a ON a.id=f.asset_id \
        JOIN media_processing m ON m.asset_id=f.asset_id AND m.revision=f.revision \
        WHERE f.resolved_at IS NULL AND COALESCE(json_extract(a.document,'$.available'),0)=1 \
        AND NOT EXISTS (SELECT 1 FROM attention_dismissals d WHERE d.asset_id=f.asset_id AND d.revision=f.revision AND d.stage=f.stage AND d.error=f.error) \
        ORDER BY f.last_seen DESC LIMIT 500",[])?;
    let items:Vec<Value>=rows.into_iter().map(|row|{
        let stage=row["stage"].as_str().unwrap_or("");
        let title=row["path"].as_str().and_then(|p|Path::new(p).file_stem()).map(|s|s.to_string_lossy().into_owned()).unwrap_or_default();
        let (severity,summary)=match stage {
            "credits_skipped"=>("warning","Credits not detected"),
            "subtitles_skipped"=>("warning","Subtitles not prepared"),
            _=>("error","Processing failed"),
        };
        json!({"id":row["id"],"asset_id":row["asset_id"],"title":title,"stage":stage,"severity":severity,"summary":summary,
            "error":row["error"],"occurrences":row["occurrences"],"first_seen":row["first_seen"],"last_seen":row["last_seen"]})
    }).collect();
    Ok(json!({"items":items}))
}
/// Mark attention items as seen: `{"ids":[...]}`, or `{"all":true}` for every one listed.
pub fn dismiss_attention(db:&Database,body:&Value)->Result<Value> {
    let insert="INSERT OR IGNORE INTO attention_dismissals(asset_id,revision,stage,error,dismissed_at) SELECT asset_id,revision,stage,error,? FROM processing_failures";
    let changed=if body["all"]==true {
        db.connection.execute(&format!("{insert} WHERE resolved_at IS NULL"),[now()])?
    } else {
        let ids:Vec<i64>=body["ids"].as_array().context("Expected ids or all")?.iter().filter_map(Value::as_i64).take(1000).collect();
        let mut total=0;
        for id in ids {total+=db.connection.execute(&format!("{insert} WHERE rowid=?"),rusqlite::params![now(),id])?;}
        total
    };
    Ok(json!({"dismissed":changed}))
}
/// Record an optional stage that failed (`credits_skipped`, `subtitles_skipped`) without
/// failing the file, so repeated errors can be found and fixed later.
fn log_failure(db:&Database,id:&str,revision:&str,stage:&str,error:&str)->Result<()> {
    db.connection.execute("INSERT INTO processing_failures(asset_id,revision,stage,error,first_seen,last_seen) VALUES(?,?,?,?,?,?) ON CONFLICT(asset_id,revision,stage,error) DO UPDATE SET occurrences=occurrences+1,last_seen=excluded.last_seen,resolved_at=NULL",rusqlite::params![id,revision,stage,error,now(),now()])?;
    tracing::warn!(asset=id,stage,error,"Processing stage skipped");
    Ok(())
}
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
/// Retire problems that no longer apply, keeping them on record with the reason: the file
/// has changed since (`superseded`), or it has left the library (`removed`).
pub(crate) fn retire_obsolete_failures(db:&Database)->Result<usize> {
    let superseded=db.connection.execute("UPDATE processing_failures SET resolved_at=?,resolution='superseded' WHERE resolved_at IS NULL \
        AND EXISTS(SELECT 1 FROM media_processing m WHERE m.asset_id=processing_failures.asset_id AND m.revision!=processing_failures.revision)",[now()])?;
    let removed=db.connection.execute("UPDATE processing_failures SET resolved_at=?,resolution='removed' WHERE resolved_at IS NULL \
        AND NOT EXISTS(SELECT 1 FROM library_assets a WHERE a.id=processing_failures.asset_id)",[now()])?;
    Ok(superseded+removed)
}
pub(crate) fn reopen_outdated(db:&Database)->Result<usize> {
    Ok(db.connection.execute("UPDATE media_processing SET credits_state='pending',stage='credits',state=CASE WHEN state IN ('ready','failed') THEN 'queued' ELSE state END,retry_after=0,attempts=0,completed_at=NULL,error=NULL,updated=? WHERE credits_state IN ('ready','failed','incomplete','skipped') AND state!='running' AND asset_id IN (SELECT asset_id FROM credits_analysis WHERE version<?)",rusqlite::params![now(),crate::credits::VERSION])?)
}
/// Experimental closing credits by sound (credits.rs): every episode of a series with an
/// episode whose closing credits the picture did not find is heard once, as each episode is
/// compared with the others. Their credits stage reopens until each has been heard.
pub(crate) fn reopen_for_sound(db:&Database)->Result<usize> {
    let rows=db.rows("SELECT a.id AS id,json_extract(a.document,'$.entity_id') AS entity,json_extract(a.document,'$.season') AS season,json_extract(a.document,'$.watch_first') AS first,json_extract(a.document,'$.role') AS role,c.document AS analysis,m.revision AS revision,m.credits_state AS credits,m.state AS state,p.revision AS heard FROM library_assets a JOIN media_processing m ON m.asset_id=a.id LEFT JOIN credits_analysis c ON c.asset_id=a.id AND c.version=? LEFT JOIN credit_prints p ON p.asset_id=a.id WHERE json_extract(a.document,'$.available')=1 AND json_extract(a.document,'$.complete')=1 AND json_extract(a.document,'$.season') IS NOT NULL AND json_extract(a.document,'$.entity_id') IS NOT NULL",[crate::credits::VERSION])?;
    let applicable=|r:&Value| r["role"]!="extra" && (r["season"]!=0 || r["first"]==1) && r["credits"]!="not_applicable";
    let missing:std::collections::HashSet<&str>=rows.iter().filter(|r|applicable(r)).filter(|r| {
        !r["analysis"].as_str().and_then(|t|serde_json::from_str::<Value>(t).ok()).is_some_and(|d|d["state"]=="ready" && crate::credits::seen_closing(&d))
    }).filter_map(|r|r["entity"].as_str()).collect();
    let mut reopened=0;
    for r in rows.iter().filter(|r|applicable(r) && r["entity"].as_str().is_some_and(|e|missing.contains(e))) {
        let (Some(id),Some(revision))=(r["id"].as_str(),r["revision"].as_str()) else {continue};
        if r["heard"].as_str()==Some(revision) || r["state"]=="running" || !finished(r["credits"].as_str().unwrap_or("")) {continue;}
        reopened+=db.connection.execute("UPDATE media_processing SET credits_state='pending',stage='credits',state=CASE WHEN state IN ('ready','failed') THEN 'queued' ELSE state END,retry_after=0,attempts=0,completed_at=NULL,error=NULL,updated=? WHERE asset_id=? AND revision=? AND state!='running'",rusqlite::params![now(),id,revision])?;
    }
    if reopened>0 {tracing::info!(reopened,"Credits: episodes queued to be heard");}
    Ok(reopened)
}
/// Files settled with no subtitle source before speech transcription existed
/// (or before it reached a verdict) reopen their subtitle stage once; the
/// transcript record then keeps them settled.
/// Titles transcribed by an older transcript version are redone the same way.
pub(crate) fn reopen_untranscribed(db:&Database)->Result<usize> {
    let transcribed=crate::speech_subtitles::transcribed_assets(db)?;
    // Bonus material is never transcribed (see media_inventory::process).
    let rows=db.rows("SELECT m.asset_id,m.revision,m.subtitles_state FROM media_processing m LEFT JOIN library_assets a ON a.id=m.asset_id WHERE m.inventory_state='ready' AND m.state!='running' AND m.subtitles_state IN ('not_applicable','ready') AND coalesce(json_extract(a.document,'$.role'),'')!='extra'",[])?;
    let rows:Vec<_>=rows.into_iter().filter(|r|r["subtitles_state"]=="not_applicable" || r["asset_id"].as_str().is_some_and(|id|transcribed.contains(id))).collect();
    let mut reopened=0;
    for row in rows {
        let (Some(id),Some(revision))=(row["asset_id"].as_str(),row["revision"].as_str()) else{continue};
        if crate::speech_subtitles::decided(db,id,revision) {continue;}
        let changed=db.connection.execute("UPDATE media_processing SET subtitles_state='pending',stage='subtitles',state=CASE WHEN state IN ('ready','failed') THEN 'queued' ELSE state END,retry_after=0,completed_at=NULL,error=NULL,updated=? WHERE asset_id=? AND revision=?",rusqlite::params![now(),id,revision])?;
        // A file that already has subtitles (from its audio) is only being checked for
        // subtitles in its picture: that waits behind every file that has none yet.
        if changed>0 && row["subtitles_state"]=="ready" {
            db.connection.execute("INSERT INTO subtitle_priorities(asset_id,priority) VALUES(?,-1) ON CONFLICT(asset_id) DO NOTHING",[id])?;
        }
        reopened+=changed;
    }
    Ok(reopened)
}
/// Files whose Chinese subtitles were translated to English a line at a time go through
/// their subtitle stage again; what is already right is found ready and only the
/// translation is made anew.
pub(crate) fn reopen_line_translated(db:&Database,assets:&std::collections::BTreeSet<String>)->Result<usize> {
    let mut reopened=0;
    for id in assets {
        reopened+=db.connection.execute("UPDATE media_processing SET subtitles_state='pending',stage='subtitles',state=CASE WHEN state IN ('ready','failed') THEN 'queued' ELSE state END,retry_after=0,completed_at=NULL,error=NULL,updated=? WHERE asset_id=? AND inventory_state='ready' AND state!='running' AND subtitles_state IN ('ready','skipped')",rusqlite::params![now(),id])?;
    }
    if reopened>0 {tracing::info!(reopened,"Chinese subtitles will be translated again, a sentence at a time");}
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
        if a["offline"]==true || a["complete"]==true {continue;}
        if let Some(path)=crate::scanner::work_path(a) {
            a["path"]=json!(path.to_string_lossy());a["available"]=json!(true);a["complete"]=json!(true);a["early"]=json!(true);
        }
    }
    // A file being moved to another drive is left alone until it lands.
    assets.retain(|a|a["available"]==true && a["complete"]==true && a["moving"]!=true);
    crate::media_inventory::order_assets(&mut assets,&crate::media_inventory::priorities(db)?);
    Ok(assets)
}
/// The queue's files in order, kept until the library or the priorities change: it was
/// built from every asset and title record, parsed, on every 2-second tick.
fn queue(db:&Database)->Result<std::sync::Arc<Vec<Value>>> {
    static QUEUE:std::sync::Mutex<Option<(String,std::sync::Arc<Vec<Value>>)>>=std::sync::Mutex::new(None);
    let mut priorities:Vec<(String,i64)>=crate::media_inventory::priorities(db)?.into_iter().collect();
    priorities.sort();
    let key=format!("{}|{priorities:?}",db.part_revision("content")?);
    let mut cache=QUEUE.lock().map_err(|_|anyhow::anyhow!("Processing queue unavailable"))?;
    if let Some((cached,list))=cache.as_ref() && *cached==key {return Ok(list.clone());}
    let list=std::sync::Arc::new(assets(db)?);
    *cache=Some((key,list.clone()));
    Ok(list)
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
    // Every stage runs on all the slots. On-device translation is serial, so it has its own
    // small lane: a file waiting only for it leaves its slot to other work.
    let limit=capacity(config.background_subtitles,preparing,playback_active(s)?,config.processing_concurrency);
    s.subtitle_jobs.set_concurrency(limit);
    s.subtitle_jobs.set_lane(if config.external_translation {limit} else {config.on_device_processing_concurrency.min(limit)});
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

/// One pass of the queue; returns when it must look again if nothing wakes it first.
pub async fn tick(s:&std::sync::Arc<Service>)->Result<Option<std::time::Instant>> {
    let limit=refresh_capacity(s)?;
    // The external fleet's answers, only while it is in use (a watch on its inbox wakes this).
    let imported=if s.effective_config()?.external_translation {s.subtitle_jobs.import_external()?} else {Vec::new()};
    // Before the database lock: reading the settings takes it.
    let drives=crate::drives::Drives::of(&s.effective_config()?);
    let online=drives.online();
    let flags=Flags::read(&s.data_dir);
    // Enrollment compares each file's identity with what was prepared for it. The library scan
    // records that identity on the asset (`file_revision`) from the metadata it reads anyway, so
    // a better copy dropped over a file at the same path is seen without processing reading
    // every file itself (it used to, once a minute, under the database lock). Only a file the
    // scan has not recorded (a finished file inside a package still downloading) is read here,
    // before the lock. Done when the catalog changes (each scan, file operations), not by a timer.
    type Stat = (String, String, bool, std::result::Result<String, String>);
    let planned: Option<(Vec<Stat>, Vec<(String, String)>, String, bool)> = {
        let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        let migrating:bool=db.connection.query_row("SELECT NOT EXISTS(SELECT 1 FROM media_processing)",[],|r|r.get(0))?;
        // The library's files and titles, not processing's own records (which change every few
        // seconds while it works, and made this pass run on every tick).
        let revision=db.part_revision("content")?;
        let due=ENROLLED.lock().map(|e|e.as_ref().is_none_or(|(r,_)|*r!=revision)).unwrap_or(true);
        if due || migrating {
            let files: Vec<Stat> = queue(&db)?.iter().filter_map(|a| {
                let recorded = a["file_revision"].as_str().filter(|_| a["early"]!=true).map(|r| Ok(r.to_owned()));
                Some((a["id"].as_str()?.to_owned(), a["path"].as_str()?.to_owned(), a["early"]==true, recorded.unwrap_or(Err(String::new()))))
            }).collect();
            let not_media: Vec<(String, String)> = db.documents("assets")?.iter().filter(|a|crate::container::tagged_not_media(a) && a["available"]==true)
                .filter_map(|a| Some((a["id"].as_str()?.to_owned(), a["path"].as_str()?.to_owned()))).collect();
            Some((files, not_media, revision, migrating))
        } else { None }
    };
    let planned = match planned {
        Some((files, not_media, revision, migrating)) => Some(tokio::task::spawn_blocking(move || {
            let files: Vec<Stat> = files.into_iter().map(|(id, path, early, recorded)| {
                // Recorded by the scan: no disk read.
                if recorded.is_ok() { return (id, path, early, recorded) }
                let revision = crate::subtitle_jobs::revision(Path::new(&path)).map_err(|e| e.to_string());
                (id, path, early, revision)
            }).collect();
            let not_media: Vec<(String, String)> = not_media.into_iter().filter_map(|(id, path)| Some((id, crate::subtitle_jobs::revision(Path::new(&path)).ok()?))).collect();
            (files, not_media, revision, migrating)
        }).await?),
        None => None,
    };
    let (selected,excess)={
        let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        for id in s.subtitle_jobs.take_translated() {
            db.connection.execute("UPDATE media_processing SET retry_after=0 WHERE asset_id=? AND state='queued'",[&id])?;
        }
        for id in &imported {
            db.connection.execute("UPDATE media_processing SET retry_after=0 WHERE asset_id=? AND state='queued'",[id])?;
            let credits_done:bool=db.connection.query_row("SELECT EXISTS(SELECT 1 FROM media_processing WHERE asset_id=? AND credits_state IN ('ready','not_applicable','skipped'))",[id],|r|r.get(0))?;
            if credits_done {if let Ok(mut fast)=FAST.lock() {fast.insert(id.clone());}}
        }
        let assets=queue(&db)?;
        let migrating=planned.as_ref().is_some_and(|p| p.3);
        if let Some((files, not_media, revision, _)) = planned {
            for (id, path, early, revision) in &files {
                let (id, path) = (id.as_str(), path.as_str());
                // Missing/unreadable files remain visible as failures, not vanished work.
                // An early file that has just moved keeps its work until the next scan finds it.
                if revision.is_err() && *early {continue;}
                // Its drive went away before the scan noticed: not a failure, and its finished work stays.
                if revision.is_err() && drives.offline(Path::new(path),&online) {continue;}
                if let Ok(revision)=&revision {
                    if carry_over(&db,id,revision)? {
                        let moved=s.subtitle_jobs.relocate(id,Path::new(path));
                        tracing::info!(asset=id,jobs=moved,"Processing followed a moved file");
                    }
                }
                enroll(&db,id,revision.as_deref().unwrap_or("unavailable"))?;
                if let Err(error)=revision {fail(&db,id,"unavailable",error)?;}
            }
            // The scanner hides non-video files from the library; settle their records.
            for (id, revision) in &not_media {
                enroll(&db,id,revision)?;
                not_video(&db,id,revision)?;
            }
            reopen_outdated(&db)?;
            reopen_for_sound(&db)?;
            reopen_replaced(&db)?;
            reopen_storage_full(&db)?;
            retire_obsolete_failures(&db)?;
            reopen_line_translated(&db,&s.subtitle_jobs.line_translated())?;
            if let Ok(mut e)=ENROLLED.lock() {*e=Some((revision,now()));}
        }
        if SWEPT.lock().map(|w|w.is_none_or(|(f,t)|f!=flags || t.is_some_and(|t|t<=now()))).unwrap_or(true) {
            sweep(&db,flags)?;
            let later=next_later(&db)?;
            if let Ok(mut w)=SWEPT.lock() {*w=Some((flags,later));}
        }
        if migrating {
            let mut unfinished=s.subtitle_jobs.all_statuses()?;
            unfinished.retain(|j|matches!(j["state"].as_str(),Some("running"|"queued"|"interrupted")));
            unfinished.sort_by(|a,b|b["updated"].as_f64().unwrap_or(0.0).total_cmp(&a["updated"].as_f64().unwrap_or(0.0)));
            if let Some(job)=unfinished.iter().find(|j|assets.iter().any(|a|a["id"]==j["asset_id"])) {
                db.connection.execute("UPDATE media_processing SET state='running' WHERE asset_id=?",[job["asset_id"].as_str().unwrap_or("")])?;
            }
        }
        let rows=db.rows("SELECT asset_id,state,retry_after,detail FROM media_processing",[])?;
        let by_asset:std::collections::HashMap<&str,&Value>=rows.iter().filter_map(|r|Some((r["asset_id"].as_str()?,r))).collect();
        let row=|a:&Value|a["id"].as_str().and_then(|id|by_asset.get(id).copied());
        // A file waiting for a translation slot comes back as soon as the lane has room.
        let lane_free=s.subtitle_jobs.lane_free();
        let due=|r:&Value|r["retry_after"].as_f64().unwrap_or(0.0)<=now() || (lane_free && awaiting_lane(r["detail"].as_str().unwrap_or("")));
        let fast:std::collections::HashSet<String>={
            let mut fast=FAST.lock().map_err(|_|anyhow::anyhow!("Processing pool unavailable"))?;
            fast.retain(|id|by_asset.get(id.as_str()).is_some_and(|r|matches!(r["state"].as_str(),Some("queued"|"running"))));
            if limit==0 {Default::default()} else {fast.clone()}
        };
        let is_fast=|a:&Value|a["id"].as_str().is_some_and(|id|fast.contains(id));
        let running=|a:&Value|!is_fast(a) && row(a).is_some_and(|r|r["state"]=="running");
        // Retain claimed files first; fill idle slots in title/episode priority order.
        let mut selected:Vec<Value>=assets.iter().filter(|a|running(a)).take(limit).cloned().collect();
        for asset in assets.iter() {
            if selected.len()>=limit {break;}
            if is_fast(asset) || selected.iter().any(|a|a["id"]==asset["id"]) {continue;}
            if row(asset).is_some_and(|r|r["state"]!="ready" && due(r)) {selected.push(asset.clone());}
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
            // Its slot is free: the next file starts now.
            crate::wake::PROCESSING.wake();
        });
        flight.insert(id,handle.abort_handle());
    }
    flight.is_empty()
    };
    // Alongside subtitle work, not only when the queue is empty (it can be busy for days).
    let _=idle;
    if limit>0 {warm_playback_index(s).await?;}
    next_deadline(&*s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?)
}
/// Files whose index build was started in this run ("id:file identity"): a build that
/// finishes reports itself (virtual_media::take_built), one that fails is not tried again
/// until the server restarts.
static WARMED:std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>>=std::sync::LazyLock::new(Default::default);
/// Processed Matroska files can lose their playback index (the cache's space reclaim, or
/// the cache moving to its own drive), and then the next play waits while the whole file
/// is re-read. Rebuild one missing index at a time, the most recently played and added
/// first, alongside subtitle work: that work can fill the queue for days, and an index is
/// only a read of the file.
///
/// playback_indexes records each file's index (its folder, or none for a file that needs
/// none), for the file identity the scan recorded; a removed index removes its row. Only
/// files without a row are looked at. (This pass used to read every file in the library,
/// and walk the whole index cache, every 5 s.)
async fn warm_playback_index(s:&Service)->Result<()> {
    let (gone,built)=(crate::virtual_media::take_removed(),crate::virtual_media::take_built());
    if !gone.is_empty() || !built.is_empty() {
        let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        for path in &gone {db.connection.execute("DELETE FROM playback_indexes WHERE path=?",[path.to_string_lossy()])?;}
        // Built for the file as the library knows it now (a playback request builds too).
        for (id,path) in &built {
            db.connection.execute("INSERT INTO playback_indexes SELECT id,json_extract(document,'$.file_revision'),?2 FROM library_assets WHERE id=?1 AND json_extract(document,'$.file_revision') IS NOT NULL ON CONFLICT(asset_id) DO UPDATE SET file_revision=excluded.file_revision,path=excluded.path",
                rusqlite::params![id,path.to_string_lossy()])?;
        }
    }
    let candidates:Vec<Value>={
        let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        let recorded:std::collections::HashMap<String,String>=db.rows("SELECT asset_id,file_revision FROM playback_indexes",[])?
            .iter().filter_map(|r|Some((r["asset_id"].as_str()?.to_owned(),r["file_revision"].as_str()?.to_owned()))).collect();
        let warmed=WARMED.lock().map_err(|_|anyhow::anyhow!("Processing pool unavailable"))?;
        let mut list:Vec<Value>=queue(&db)?.iter().filter(|a|a["role"]!="extra" && a["early"]!=true && a["available"]==true && a["complete"]==true)
            .filter(|a|match (a["id"].as_str(),a["file_revision"].as_str()) {
                (Some(id),Some(revision))=>recorded.get(id).is_none_or(|r|r!=revision) && !warmed.contains(&format!("{id}:{revision}")),
                _=>false,
            }).cloned().collect();
        drop(warmed);
        if list.is_empty() {return Ok(());}
        let played:std::collections::HashMap<String,f64>=db.rows("SELECT media_id,MAX(last_played) AS at FROM profile_playback GROUP BY media_id",[])?
            .iter().filter_map(|r|Some((r["media_id"].as_str()?.to_owned(),r["at"].as_f64().unwrap_or(0.0)))).collect();
        let recency=|a:&Value|{
            let media=format!("{}:{}",a["torrent_hash"].as_str().unwrap_or(""),a["file_index"].as_i64().unwrap_or(0));
            played.get(&media).copied().unwrap_or(0.0).max(a["added_at"].as_f64().unwrap_or(0.0))
        };
        list.sort_by(|a,b|recency(b).total_cmp(&recency(a)));
        list
    };
    let record=|id:&str,revision:&str,path:Option<&Path>|->Result<()>{
        s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?.connection.execute(
            "INSERT INTO playback_indexes VALUES(?,?,?) ON CONFLICT(asset_id) DO UPDATE SET file_revision=excluded.file_revision,path=excluded.path",
            rusqlite::params![id,revision,path.map(|p|p.to_string_lossy().into_owned())])?;
        Ok(())
    };
    for asset in candidates {
        let (Some(id),Some(revision),Some(path))=(asset["id"].as_str(),asset["file_revision"].as_str(),asset["path"].as_str()) else{continue};
        // Only Matroska files are indexed; the rest are read as they are.
        if !crate::container::is_matroska(Path::new(path)) {record(id,revision,None)?;continue;}
        let Ok(input)=crate::api::media_file(s,id,0).await else{continue};
        if let Some(index)=s.virtual_media.index_of(id,&input) {record(id,revision,Some(&index))?;continue;}
        if !s.virtual_media.background_room() || !s.virtual_media.can_preindex() || playback_active(s)? {return Ok(());}
        // Recorded when the build reports it has finished.
        if let Ok(mut warmed)=WARMED.lock() {warmed.insert(format!("{id}:{revision}"));}
        match s.virtual_media.prepare_analysis(id,input) {
            Ok(_)=>tracing::info!(asset=id,"Rebuilding a missing playback index"),
            Err(error)=>tracing::warn!(asset=id,%error,"Playback index warm-up failed"),
        }
    }
    Ok(())
}
async fn process_file(s:&Service,mut asset:Value)->Result<()> {
    let id=asset["id"].as_str().context("Missing asset ID")?.to_owned();
    let path=asset["path"].as_str().context("Missing asset path")?.to_owned();
    let revision=match crate::subtitle_jobs::revision(Path::new(&path)) {
        Ok(revision)=>revision,
        Err(error) if !Path::new(&path).exists()=>{
            // Gone before it started (moved by its package or filing, or deleted): wait for the
            // scan to give its new path. Returning at once had its freed slot pick it again,
            // 706 times in 2 s on 2026-11-19.
            let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
            db.connection.execute("UPDATE media_processing SET state='queued',retry_after=?,updated=? WHERE asset_id=?",rusqlite::params![now()+15.0,now(),id])?;
            tracing::info!(asset=id,%error,"Processing paused: its file is not at its path");
            return Ok(());
        }
        Err(error)=>return Err(error),
    };
    let previous={let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        let row=db.rows("SELECT * FROM media_processing WHERE asset_id=?",[&id])?.pop().context("Missing processing record")?;
        db.connection.execute("UPDATE media_processing SET state='running',error=NULL,updated=? WHERE asset_id=?",rusqlite::params![now(),id])?;row};
    // Retry failed subtitle jobs once per new attempt. The loop checks a running file every
    // few seconds; retrying on each check restarted a failing OCR job forever.
    asset["retrying"]=json!(previous["attempts"].as_i64().unwrap_or(0)>0 && previous["state"]!="running");
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
        } else if let Some(until)=crate::opensubtitles::waits_until(&message) {
            // The day's OpenSubtitles downloads are used up: wait for the reset, not an hour's backoff.
            db.connection.execute("UPDATE media_processing SET state='queued',retry_after=?,error=?,updated=? WHERE asset_id=? AND revision=?",rusqlite::params![until+60.0,message,now(),id,revision])?;
            tracing::info!(asset=id,until,"Processing waits for OpenSubtitles downloads");
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
/// A file finished one of the queues the Processing page counts.
fn finished_queue(db:&Database,queue:&str)->Result<()> {
    db.connection.execute("INSERT INTO processing_events(stage,at) VALUES(?,?)",rusqlite::params![queue,now()])?;
    db.connection.execute("DELETE FROM processing_events WHERE at<?",[now()-86400.0])?;
    Ok(())
}
/// The subtitle work still to do is a translation (queued, waiting or translating).
fn translation_only(progress:&Value)->bool {
    progress["target"].as_str().is_some() && matches!(progress["state"].as_str(),Some("queued"|"running"|"interrupted"))
}
/// A file released for translation whose translation has not been admitted yet.
fn awaiting_lane(detail:&str)->bool {
    serde_json::from_str::<Value>(detail).is_ok_and(|d|translation_only(&d) && (d["id"].is_null() || d["state"]=="interrupted"))
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
    // Subtitles waiting only for translation: credits run meanwhile, then the slot is released.
    let mut translating=None;
    if !finished(prior["subtitles_state"].as_str().unwrap_or("")) {
        let progress=match crate::media_inventory::process(s,asset).await {
            Ok(progress)=>progress,
            // Shutdown, a file mid-move and a model swap are retried, not skipped.
            Err(error) if SHUTTING_DOWN.load(std::sync::atomic::Ordering::SeqCst) || !path.exists() || transient(&error.to_string()) || settling(path)
                || crate::opensubtitles::retry_later(&error.to_string()) => return Err(error),
            Err(error)=>json!({"state":"failed","error":error.to_string()}),
        };
        let mut state=progress["state"].as_str().unwrap_or("queued");
        // Subtitles are not essential to playing the file: log the failure and go on.
        if state=="failed" && (transient(progress["error"].as_str().unwrap_or("")) || settling(path)) {
            anyhow::bail!("{}",progress["error"].as_str().unwrap_or(""));
        }
        if state=="failed" {
            let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
            log_failure(&db,id,revision,"subtitles_skipped",progress["error"].as_str().unwrap_or("Subtitle preparation failed"))?;
            state="skipped";
        }
        let done=finished(state);
        // Released to the translation lane on an earlier pass: this pass ends its translation.
        let was_translating=prior["stage"]=="translation";
        {let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
            db.connection.execute("UPDATE media_processing SET inventory_state='ready',subtitles_state=?,stage=?,detail=?,updated=? WHERE asset_id=? AND revision=?",rusqlite::params![if done{state}else{"pending"},if done{"credits"}else{progress["stage"].as_str().unwrap_or("subtitles")},progress.to_string(),now(),id,revision])?;
            if done && was_translating {finished_queue(&db,"translation")?;}
            else if !was_translating && (done || translation_only(&progress)) {finished_queue(&db,"subtitles")?;}
        }
        // While the external lane translates, use this slot for credits analysis.
        external=state=="external";
        if !done && !external && translation_only(&progress) {translating=Some(progress.clone());}
        if !done && !external && translating.is_none() {return Ok(());}
    }
    if !finished(prior["credits_state"].as_str().unwrap_or("")) {
        // Specials list as extras (except a watch-first pilot), so no Next Episode offer needs their credits.
        let special=asset["season"]==0 && asset["watch_first"]!=true;
        let not_applicable=asset["role"]=="extra" || special || path.extension().is_some_and(|e|e.eq_ignore_ascii_case("webm"));
        {let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
            db.connection.execute("UPDATE media_processing SET stage='credits',detail='{}' WHERE asset_id=? AND revision=?",rusqlite::params![id,revision])?;}
        let mut scan_error=None;
        if !not_applicable {
            if let Err(error)=crate::credits::process(s,asset).await {
                if SHUTTING_DOWN.load(std::sync::atomic::Ordering::SeqCst) {return Err(error);}
                scan_error=Some(error.to_string());
            }
        }
        let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        let credits=if let Some(error)=scan_error {json!({"state":"failed","error":error})} else if not_applicable {json!({"state":"not_applicable","reason":"Credits analysis does not apply to extras, specials or WebM"})}else{crate::credits::cached(&db,id)?};
        let mut state=credits["state"].as_str().unwrap_or("pending");
        // Credits only place the Next Episode offer; a scan that fails is dropped, not a failure.
        if matches!(state,"failed"|"incomplete") {
            log_failure(&db,id,revision,"credits_skipped",credits["error"].as_str().unwrap_or("Credits analysis incomplete"))?;
            state="skipped";
        }
        db.connection.execute("UPDATE media_processing SET stage='credits',credits_state=?,detail=?,updated=? WHERE asset_id=? AND revision=?",rusqlite::params![state,credits.to_string(),now(),id,revision])?;
        if finished(state) && !not_applicable {finished_queue(&db,"credits")?;}
        if state=="deferred" {
            db.connection.execute("UPDATE media_processing SET state='queued',retry_after=?,error=?,updated=? WHERE asset_id=?",rusqlite::params![now()+60.0,credits["error"].as_str(),now(),id])?;
        }
        if !finished(state){return Ok(());}
    }
    if let Some(progress)=translating {
        // Only translation is left: give the slot to other work. The file comes back when
        // its translation ends, or (not yet started) when the translation lane has room.
        let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        db.connection.execute("UPDATE media_processing SET state='queued',stage=?,detail=?,retry_after=?,updated=? WHERE asset_id=? AND revision=?",
            rusqlite::params![progress["stage"].as_str().unwrap_or("translation"),progress.to_string(),now()+1800.0,now(),id,revision])?;
        return Ok(());
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
/// What a file is, as the library names it: the show (or movie), and for an episode its
/// season, number and name ("Part 2" for a part of a variety show's episode; a special's or
/// bonus video's own name). Files not yet matched keep their file name as the show.
fn name(asset:&Value,entity:Option<&Value>)->Value {
    let stem=asset["path"].as_str().and_then(|p|Path::new(p).file_stem()).and_then(|p|p.to_str()).unwrap_or("").to_owned();
    let Some(entity)=entity else { return json!({"show":stem,"season":null,"episode":null,"episode_name":null}) };
    let show=entity["title"].as_str().filter(|t|!t.is_empty()).map(str::to_owned).unwrap_or(stem);
    if entity["kind"]!="tv" { return json!({"show":show,"season":null,"episode":null,"episode_name":null}) }
    let season=asset["season"].as_u64();
    if season==Some(0)||asset["role"]=="extra" { return json!({"show":show,"season":null,"episode":null,"episode_name":crate::catalog::extra_name(asset)}) }
    let part=asset["path"].as_str().and_then(crate::scanner::part);
    let episode_name=if part.is_some() { asset["path"].as_str().and_then(crate::scanner::part_title) } else {
        entity["episodes"].as_array().and_then(|eps|eps.iter().find(|e|e["season"]==asset["season"]&&e["episode"]==asset["episode"]))
            .and_then(|e|e["title"].as_str()).map(str::to_owned)
    };
    json!({"show":show,"season":asset["season"],"episode":asset["episode"],"episode_name":episode_name})
}
pub fn summary(db:&Database,jobs:&crate::subtitle_jobs::Manager)->Result<Value> {
    let rows=db.rows("SELECT * FROM media_processing",[])?;
    let translating=jobs.translating_now();
    // Files waiting (running included) and running, per queue, in the order a file meets them.
    let mut queues:std::collections::BTreeMap<&str,(u64,u64)>=[("subtitles",(0,0)),("translation",(0,0)),("credits",(0,0))].into_iter().collect();
    let mut queue=Vec::new();let mut running=Vec::new();let mut scanned=0;let mut completed=0;let mut failed=0;
    let entities:std::collections::HashMap<String,Value>=db.documents("entities")?.into_iter()
        .filter_map(|e|e["id"].as_str().map(|id|(id.to_owned(),e.clone()))).collect();
    let by_asset:std::collections::HashMap<&str,&Value>=rows.iter().filter_map(|r|Some((r["asset_id"].as_str()?,r))).collect();
    let listed=self::queue(db)?;
    for asset in listed.iter() {
        let id=asset["id"].as_str().unwrap_or("");
        let row=by_asset.get(id).copied();
        let mut detail:Value=row.and_then(|r|r["detail"].as_str()).and_then(|v|serde_json::from_str(v).ok()).unwrap_or(json!({}));
        let mut state=row.and_then(|r|r["state"].as_str()).unwrap_or("queued");
        let mut stage=row.and_then(|r|r["stage"].as_str()).unwrap_or("inventory");
        // Translating in its own lane after giving its slot back: shown as running.
        if let Some(job)=translating.get(id).filter(|_|state=="queued") {
            state="running";stage="translation";
            detail=json!({"target":job["target"],"extraction_method":job["method"],"completed_cues":job["completed_cues"],"total_cues":job["total_cues"]});
        }
        let title=asset["path"].as_str().and_then(|p|Path::new(p).file_stem()).and_then(|p|p.to_str()).unwrap_or(id);
        let cues=detail["completed_cues"].as_u64().unwrap_or(0);let total=detail["total_cues"].as_u64().unwrap_or(0);
        let named=name(&asset,asset["entity_id"].as_str().and_then(|e|entities.get(e)));
        if row.is_some_and(|r|r["inventory_state"]=="ready"){scanned+=1;}
        if state=="ready"{completed+=1;}
        if state=="failed"{failed+=1;}
        let item=json!({"id":id,"asset_id":id,"title":title,"source":detail["source"],"show":named["show"],"season":named["season"],"episode":named["episode"],"episode_name":named["episode_name"],"state":state,"stage":stage,"target":detail["target"],"error":row.map(|r|r["error"].clone()).unwrap_or(Value::Null),"completed_cues":cues,"total_cues":total,"method":detail["extraction_method"],
            "inventory_state":row.map(|r|r["inventory_state"].clone()),"subtitles_state":row.map(|r|r["subtitles_state"].clone()),"credits_state":row.map(|r|r["credits_state"].clone()),"completed_at":row.map(|r|r["completed_at"].clone())});
        if state!="ready" {
            let settled=|k:&str|row.and_then(|r|r[k].as_str()).is_some_and(finished);
            let translating=stage=="translation";
            let queue=if translating {"translation"} else if !settled("subtitles_state") {"subtitles"} else {"credits"};
            if let Some(q)=queues.get_mut(queue) {q.0+=1;if state=="running"{q.1+=1;}}
            // Credits run while a translation waits; still owed, they are counted there too.
            if translating && !settled("credits_state") {if let Some(q)=queues.get_mut("credits") {q.0+=1;}}
        }
        if state=="running"{running.push(json!({"asset_id":id,"title":title,"source":detail["source"],"show":named["show"],"season":named["season"],"episode":named["episode"],"episode_name":named["episode_name"],"language":detail["target"].as_str().unwrap_or(""),"stage":stage,"method":detail["extraction_method"],"completed_cues":cues,"total_cues":total}));}
        queue.push(item);
    }
    // Files per minute: those that finished each queue in the last hour.
    let hour=now()-3600.0;
    let queues:Vec<Value>=["subtitles","translation","credits"].iter().map(|name|{
        let (waiting,running)=queues[name];
        let finished:i64=db.connection.query_row("SELECT COUNT(*) FROM processing_events WHERE stage=? AND at>=?",rusqlite::params![name,hour],|r|r.get(0)).unwrap_or(0);
        json!({"id":name,"waiting":waiting,"running":running,"finished_last_hour":finished,"per_minute":finished as f64/60.0})
    }).collect();
    Ok(json!({"scanned":scanned,"translated":completed,"completed":completed,"failed":failed,"waiting_for_idle":false,"current":running.first().cloned().unwrap_or(Value::Null),"running":running,"queue":queue,"queues":queues}))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn database()->Result<Database>{let db=Database{connection:rusqlite::Connection::open_in_memory()?};db.connection.execute_batch(include_str!("schema.sql"))?;crate::storage::add_column(&db.connection,"processing_failures","resolution","TEXT")?;let mut db=db;db.migrate_profiles(9)?;db.initialize_catalog_revision()?;Ok(db)}
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
    fn subtitles_skipped_for_a_full_store_are_prepared_again()->Result<()> {
        let db=database()?;
        for (id,error) in [("full",crate::subtitle_jobs::STORAGE_FULL),("broken","Subtitle validation failed")] {
            enroll(&db,id,"r1")?;
            db.connection.execute("UPDATE media_processing SET state='ready',stage='complete',inventory_state='ready',subtitles_state='skipped',credits_state='ready',detail=? WHERE asset_id=?",
                rusqlite::params![json!({"state":"failed","error":error}).to_string(),id])?;
        }
        // Recorded only in the failure log, not in the file's own detail.
        enroll(&db,"logged","r1")?;
        db.connection.execute("UPDATE media_processing SET state='ready',stage='complete',inventory_state='ready',subtitles_state='skipped',credits_state='ready',detail='{}' WHERE asset_id='logged'",[])?;
        log_failure(&db,"logged","r1","subtitles_skipped",crate::subtitle_jobs::STORAGE_FULL)?;
        assert_eq!(reopen_storage_full(&db)?,2);
        let rows=db.rows("SELECT asset_id,state,subtitles_state FROM media_processing ORDER BY asset_id",[])?;
        assert_eq!(rows.iter().map(|r|r["subtitles_state"].as_str().unwrap_or("")).collect::<Vec<_>>(),["skipped","pending","pending"]);
        Ok(())
    }
    #[test]
    fn a_file_skipped_because_it_was_just_replaced_is_done_again()->Result<()> {
        let db=database()?;
        assert!(transient("Media is unavailable or changed"));
        assert!(transient("Download is not complete"));
        for id in ["replaced","unreadable"] {
            enroll(&db,id,"r2")?;
            let error=if id=="replaced" {MEDIA_CHANGED} else {"integers can be at most 8 bytes"};
            db.connection.execute("UPDATE media_processing SET state='ready',stage='complete',inventory_state='ready',subtitles_state='skipped',credits_state='skipped',detail=? WHERE asset_id=?",
                rusqlite::params![json!({"state":"failed","error":error}).to_string(),id])?;
        }
        assert_eq!(reopen_replaced(&db)?,1);
        assert_eq!(db.rows("SELECT state,subtitles_state FROM media_processing WHERE asset_id='replaced'",[])?[0]["subtitles_state"],"pending");
        Ok(())
    }

    #[test]
    fn skipped_credits_and_subtitles_still_let_the_file_finish()->Result<()> {
        let db=database()?;enroll(&db,"episode","r1")?;
        db.connection.execute("UPDATE media_processing SET inventory_state='ready',subtitles_state='ready',credits_state='skipped' WHERE asset_id='episode'",[])?;
        assert!(finished("skipped"));
        log_failure(&db,"episode","r1","credits_skipped","Video timestamps are irregular")?;
        db.connection.execute("UPDATE media_processing SET subtitles_state='skipped' WHERE asset_id='episode'",[])?;
        assert!(sign_off(&db,"episode","r1")?);
        // The skip stays on record, unresolved, for mining.
        let logged=db.rows("SELECT stage,resolved_at FROM processing_failures",[])?;
        assert_eq!((logged[0]["stage"].as_str(),logged[0]["resolved_at"].is_null()),(Some("credits_skipped"),true));
        // When a later attempt succeeds at that stage, the skip is resolved.
        db.connection.execute("UPDATE media_processing SET credits_state='ready',state='queued' WHERE asset_id='episode'",[])?;
        assert!(sign_off(&db,"episode","r1")?);
        assert!(!db.rows("SELECT resolved_at FROM processing_failures",[])?[0]["resolved_at"].is_null());
        assert_eq!(db.rows("SELECT state FROM media_processing WHERE asset_id='episode'",[])?[0]["state"],"ready");
        Ok(())
    }
    #[test]
    fn obsolete_failures_are_retired_with_a_reason()->Result<()> {
        let db=database()?;
        db.connection.execute("INSERT INTO library_assets VALUES('a',?)",[json!({"id":"a","available":true,"path":"/M/a.mkv"}).to_string()])?;
        enroll(&db,"a","r2")?;
        for (asset,revision,error) in [("a","r1","old"),("a","r2","current"),("gone","r1","missing")] {
            db.connection.execute("INSERT INTO processing_failures(asset_id,revision,stage,error,first_seen,last_seen) VALUES(?,?,'inventory',?,1,1)",[asset,revision,error])?;
        }
        assert_eq!(retire_obsolete_failures(&db)?,2);
        let rows=db.rows("SELECT error,resolution FROM processing_failures ORDER BY error",[])?;
        let reason=|e:&str|rows.iter().find(|r|r["error"]==e).unwrap()["resolution"].clone();
        assert_eq!((reason("old"),reason("missing"),reason("current")),(json!("superseded"),json!("removed"),Value::Null));
        Ok(())
    }
    #[test]
    fn attention_lists_errors_and_skips_until_dismissed()->Result<()> {
        let db=database()?;
        for id in ["a","b"] {
            db.connection.execute("INSERT INTO library_assets VALUES(?,?)",rusqlite::params![id,json!({"id":id,"available":true,"path":format!("/M/{id} - S01E01.mkv")}).to_string()])?;
            enroll(&db,id,"r1")?;
        }
        // A problem with an earlier version of the file is not listed.
        db.connection.execute("INSERT INTO processing_failures(asset_id,revision,stage,error,first_seen,last_seen) VALUES('a','old','inventory','No such file',1,1)",[])?;
        fail(&db,"a","r1","decode failed")?;
        log_failure(&db,"b","r1","credits_skipped","Video timestamps are irregular")?;
        let items=attention(&db)?["items"].as_array().unwrap().clone();
        assert_eq!(items.len(),2);
        let skip=items.iter().find(|i|i["asset_id"]=="b").unwrap();
        assert_eq!((skip["severity"].as_str(),skip["title"].as_str()),(Some("warning"),Some("b - S01E01")));
        dismiss_attention(&db,&json!({"ids":[skip["id"]]}))?;
        assert_eq!(attention(&db)?["items"].as_array().unwrap().len(),1);
        // Seen stays seen when the same problem recurs.
        log_failure(&db,"b","r1","credits_skipped","Video timestamps are irregular")?;
        assert_eq!(attention(&db)?["items"].as_array().unwrap().len(),1);
        dismiss_attention(&db,&json!({"all":true}))?;
        assert!(attention(&db)?["items"].as_array().unwrap().is_empty());
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
        let root=std::env::temp_dir().join(format!("torrbaby-summary-{}",std::process::id()));
        let jobs=crate::subtitle_jobs::Manager::new(&root.join("subtitles"))?;
        finished_queue(&db,"credits")?;finished_queue(&db,"credits")?;
        let queue=summary(&db,&jobs)?;
        let _=std::fs::remove_dir_all(&root);
        let queues=queue["queues"].as_array().unwrap();
        assert_eq!(queues.iter().map(|q|q["id"].as_str().unwrap()).collect::<Vec<_>>(),["subtitles","translation","credits"]);
        assert_eq!(queues[2]["finished_last_hour"],2);
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
    fn titles_with_one_language_are_done_again()->Result<()> {
        let db=database()?;
        for id in ["english","both","neither"] {enroll(&db,id,"r1")?;}
        db.connection.execute("UPDATE media_processing SET state='ready',stage='complete',inventory_state='ready',subtitles_state='ready',credits_state='ready',completed_at=1",[])?;
        let prepared=|languages:&[&str]|json!({"prepared":languages.iter().map(|l|json!({"state":"ready","language":l})).collect::<Vec<_>>()}).to_string();
        for (id,languages) in [("english",&["en"][..]),("both",&["en","zh-Hans"]),("neither",&[])] {
            db.connection.execute("INSERT INTO media_inventory VALUES(?,'r1',?,0)",rusqlite::params![id,prepared(languages)])?;
        }
        assert_eq!(reopen_one_language(&db)?,1);
        let row=db.rows("SELECT state,subtitles_state FROM media_processing WHERE asset_id='english'",[])?.remove(0);
        assert_eq!((row["state"].as_str(),row["subtitles_state"].as_str()),(Some("queued"),Some("pending")));
        // Once it has both it stays settled.
        db.connection.execute("UPDATE media_inventory SET document=? WHERE asset_id='english'",[prepared(&["en","zh-Hans"])])?;
        db.connection.execute("UPDATE media_processing SET state='ready',subtitles_state='ready' WHERE asset_id='english'",[])?;
        assert_eq!(reopen_one_language(&db)?,0);
        Ok(())
    }
    #[test]
    fn files_without_subtitles_reopen_once_for_transcription()->Result<()> {
        let db=database()?;
        for id in ["silent","advert","subtitled"] {enroll(&db,id,"r1")?;}
        db.connection.execute("UPDATE media_processing SET state='ready',stage='complete',inventory_state='ready',subtitles_state='not_applicable',credits_state='ready',completed_at=1",[])?;
        db.connection.execute("UPDATE media_processing SET inventory_state='not_applicable' WHERE asset_id='advert'",[])?;
        db.connection.execute("UPDATE media_processing SET subtitles_state='ready' WHERE asset_id='subtitled'",[])?;
        assert_eq!(reopen_untranscribed(&db)?,1);
        let row=db.rows("SELECT state,subtitles_state,credits_state FROM media_processing WHERE asset_id='silent'",[])?.remove(0);
        assert_eq!((row["state"].as_str(),row["subtitles_state"].as_str(),row["credits_state"].as_str()),(Some("queued"),Some("pending"),Some("ready")));
        // Once transcription reaches a verdict the file stays settled.
        db.connection.execute("UPDATE media_processing SET state='ready',subtitles_state='not_applicable' WHERE asset_id='silent'",[])?;
        crate::speech_subtitles::store(&db,&crate::speech_subtitles::record(&db,"silent","r1"),&serde_json::from_str(r#"{"state":"rejected","tried":["en","zh-Hans","picture","opensubtitles"]}"#)?)?;
        assert_eq!(reopen_untranscribed(&db)?,0);
        // Rejected before OpenSubtitles was a source: reopened for it once.
        crate::speech_subtitles::store(&db,&crate::speech_subtitles::record(&db,"silent","r1"),&serde_json::from_str(r#"{"state":"rejected","tried":["en","zh-Hans","picture"]}"#)?)?;
        assert_eq!(reopen_untranscribed(&db)?,1);
        db.connection.execute("UPDATE media_processing SET state='ready',subtitles_state='not_applicable' WHERE asset_id='silent'",[])?;
        crate::speech_subtitles::store(&db,&crate::speech_subtitles::record(&db,"silent","r1"),&serde_json::from_str(r#"{"state":"rejected","tried":["en","zh-Hans","picture","opensubtitles"]}"#)?)?;
        // OpenSubtitles' records beside transcripts are not transcripts: no reopening for them.
        crate::speech_subtitles::store(&db,&format!("{}.en-opensubtitles","b".repeat(64)),&json!({"state":"ready","asset_id":"subtitled"}))?;
        assert_eq!(reopen_untranscribed(&db)?,0);
        // A finished title transcribed by an older version is redone once.
        crate::speech_subtitles::store(&db,&"a".repeat(64),&json!({"state":"ready","asset_id":"subtitled"}))?;
        assert_eq!(reopen_untranscribed(&db)?,1);
        crate::speech_subtitles::store(&db,&crate::speech_subtitles::record(&db,"subtitled","r1"),&serde_json::from_str(r#"{"state":"ready","asset_id":"subtitled","tried":["picture","en","opensubtitles"]}"#)?)?;
        db.connection.execute("UPDATE media_processing SET state='ready',subtitles_state='ready' WHERE asset_id='subtitled'",[])?;
        assert_eq!(reopen_untranscribed(&db)?,0);
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
