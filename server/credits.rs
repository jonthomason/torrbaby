//! Bounded, revision-keyed credits stage (rolling crawls and static credit cards)
//! of the durable processing queue.
use crate::{api::Service, storage::{Database, now}};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use rusqlite::OptionalExtension;
use std::{path::Path, process::Stdio, sync::Arc, time::Duration};

/// 2: static credit cards (cards-v1) alongside rolling crawls, resolved per series.
pub(crate) const VERSION:i64=2;
pub fn cached(db:&Database,id:&str)->Result<Value> {
    let asset=db.asset(id)?;
    let row:Option<(String,String)>=db.connection.query_row("SELECT revision,document FROM credits_analysis WHERE asset_id=? AND version=?",rusqlite::params![id,VERSION],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((revision,text))=row {
        if crate::scanner::work_path(&asset).and_then(|p|crate::subtitle_jobs::revision(&p).ok()).as_deref()==Some(&revision) {return Ok(explain(serde_json::from_str(&text)?));}
    }
    Ok(json!({"state":"pending","segments":[]}))
}
/// An unfinished or failed analysis must always say why; the processing queue
/// surfaces this text instead of a bare "incomplete".
fn explain(mut document:Value)->Value {
    if matches!(document["state"].as_str(),Some("incomplete"|"failed")) && document["error"].as_str().is_none_or(|e|e.trim().is_empty()) {
        let reason=match document["sample_errors"].as_u64() {
            Some(n) if n>0 => format!("Credits scanner could not read {n} of {} sampled positions",document["probes"].as_u64().unwrap_or(0)),
            _ => "Credits scanner stopped without a result".to_owned(),
        };
        document["error"]=json!(reason);
    }
    document
}
fn save(db:&Database,id:&str,revision:&str,document:&Value)->Result<()> {
    db.connection.execute("INSERT INTO credits_analysis VALUES(?,?,?,?,?) ON CONFLICT(asset_id) DO UPDATE SET revision=excluded.revision,version=excluded.version,document=excluded.document,updated=excluded.updated",
        rusqlite::params![id,revision,VERSION,document.to_string(),now()])?; Ok(())
}
pub fn summary(db:&Database)->Result<Value> {
    let rows=db.rows("SELECT a.id,a.document AS asset,c.document AS analysis FROM library_assets a LEFT JOIN credits_analysis c ON a.id=c.asset_id ORDER BY c.updated DESC",[])?;
    let mut current=Value::Null;let mut scanned=0;let mut detected=0;let mut failed=0;
    for row in rows {
        let asset:Value=serde_json::from_str(row["asset"].as_str().unwrap_or("{}"))?;
        if asset["available"]!=true || asset["complete"]!=true {continue;}
        let info:Value=serde_json::from_str(row["analysis"].as_str().unwrap_or("{}"))?;
        if info["state"]=="running" {current=json!({"asset_id":row["id"],"title":asset["name"].as_str().or_else(||asset["path"].as_str().and_then(|p|Path::new(p).file_name()?.to_str())).unwrap_or("" )});}
        if info["state"]=="ready" {scanned+=1;if info["segments"].as_array().is_some_and(|s|!s.is_empty()){detected+=1;}}
        if info["state"]=="failed" || info["state"]=="incomplete" {failed+=1;}
    }
    Ok(json!({"scanned":scanned,"detected":detected,"failed":failed,"current":current}))
}
pub async fn process(s:&Service, asset:&Value)->Result<()> {
    // Only playback preparation pauses analysis. Other scans' readers and playback
    // are handled by the processing pool's limit (1 while playing), not here.
    if s.virtual_media.preparing() {return Ok(());}
    for asset in std::iter::once(asset) {
        if asset["available"]!=true || asset["complete"]!=true || asset["role"]=="extra" {continue;}
        let Some(id)=asset["id"].as_str() else {continue};
        let Some(path)=asset["path"].as_str() else {continue};
        let path=Path::new(path);
        if !crate::container::virtual_mp4_source(path) {continue;}
        let Ok(revision)=crate::subtitle_jobs::revision(path) else {continue};
        {let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
            let prior:Option<(String,i64,String,f64)>=db.connection.query_row("SELECT revision,version,document,updated FROM credits_analysis WHERE asset_id=?",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
            if let Some((rev,version,text,_updated))=prior {
                let info:Value=serde_json::from_str(&text)?;
                if rev==revision && version==VERSION && info["state"]=="ready"{continue;}
            }
        }
        // Resolve through the same complete-file/access checks as playback.
        let input=crate::api::processing_file(s,id).await?;
        let mut reader=None;
        // By content: Matroska named .mp4 must be indexed, not handed to AVFoundation.
        if crate::container::is_matroska(&input) {
            let job=s.virtual_media.prepare_analysis(id,input.clone())?;
            if job["state"]!="ready" {
                // While the index builds, replace any earlier result: otherwise the
                // queue re-reports a stale failure (e.g. from an older build that
                // rejected the audio) instead of waiting for the rebuild.
                let document=if job["state"]=="failed" {json!({"state":"failed","error":job["error"]})}else{json!({"state":"deferred"})};
                let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
                save(&db,id,&revision,&document)?;
                return Ok(());
            }
            reader=Some(s.virtual_media.analysis_reader(job["id"].as_str().context("Missing index")?,id,&input).await?);
        }
        {let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;save(&db,id,&revision,&json!({"state":"running"}))?;}
        let outcome=scan(s,&input,reader,if asset["kind"]=="tv" || asset["season"].is_number(){300}else{900}).await;
        let document=explain(match outcome {Ok(result)=>crate::credit_series::prepare(result),Err(error)=>json!({"state":if error.to_string().contains("deferred") {"deferred"}else{"failed"},"error":error.to_string()})});
        if crate::subtitle_jobs::revision(&input).ok().as_deref()==Some(&revision) {
            let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;save(&db,id,&revision,&document)?;
            // Resolve this file against its series, and its series against it.
            let entity=asset["entity_id"].as_str().map(str::to_owned).unwrap_or_default();
            if entity.is_empty() {save(&db,id,&revision,&crate::credit_series::resolve(&document,&[]))?;}
            else {crate::credit_series::resolve_title(&db,&entity,VERSION)?;}
        }
        tracing::info!(asset=id,state=?document["state"],elapsed=?document["elapsed_seconds"],"Credits analysis finished");
        return Ok(());
    }
    Ok(())
}
/// Serve an indexed Matroska file as a virtual MP4 to a local analysis helper
/// (AVFoundation cannot read Matroska). Ephemeral loopback-only capability: no
/// media credentials in argv or files. Abort the task when the helper is done.
pub(crate) async fn serve(reader:Arc<crate::virtual_captions::Reader>)->Result<(String,tokio::task::JoinHandle<()>)> {
    use axum::{Router,routing::any,http::{Method,HeaderMap},response::IntoResponse};
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let key=uuid::Uuid::new_v4().to_string();
    let url=format!("http://127.0.0.1:{}/{key}.mp4",listener.local_addr()?.port());
    let route=format!("/{key}.mp4");
    let router=Router::new().route(&route,any(move |method:Method,headers:HeaderMap| {let reader=reader.clone();async move {
        match crate::api::virtual_response(reader,"credits",&method,&headers).await {Ok(r)=>r,Err(_)=>axum::http::StatusCode::BAD_REQUEST.into_response()}
    }}));
    Ok((url,tokio::spawn(async move {let _=axum::serve(listener,router).await;})))
}
async fn scan(s:&Service,path:&Path,reader:Option<Arc<crate::virtual_captions::Reader>>,window:u32)->Result<Value> {
    let mut task=None;
    let input=if let Some(reader)=reader {
        let (url,server)=serve(reader).await?;task=Some(server);url
    } else {path.to_str().context("Invalid media path")?.to_owned()};
    let progress=std::env::temp_dir().join(format!("torrbaby-credits-{}.json",uuid::Uuid::new_v4()));
    let result=async {
        let helper=std::env::current_exe()?.with_file_name("torrbaby-credits-scanner");
        let mut child=tokio::process::Command::new("/usr/bin/nice").args(["-n","15"]).arg(helper).arg(input).arg(window.to_string()).arg(&progress)
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).spawn()?;
        let stdout=child.stdout.take().context("Missing scanner output")?;
        let stderr=child.stderr.take().context("Missing scanner errors")?;
        use tokio::io::AsyncReadExt;
        let out=tokio::spawn(async move {let mut v=Vec::new();stdout.take(65537).read_to_end(&mut v).await.map(|_|v)});
        let err=tokio::spawn(async move {let mut v=Vec::new();stderr.take(8192).read_to_end(&mut v).await.map(|_|v)});
        let started=std::time::Instant::now();
        // Finish as soon as the scanner exits; check playback preparation once a second.
        let status=loop {
            tokio::select! {
                status=child.wait()=>break status?,
                _=tokio::time::sleep(Duration::from_secs(1))=>{}
            }
            if started.elapsed()>Duration::from_secs(300) || s.virtual_media.preparing() {
                child.kill().await?;let _=child.wait().await;out.abort();err.abort();
                anyhow::bail!("Credits scan deferred for playback preparation or time budget");
            }
        };
        let output=out.await??;let error=err.await??;
        ensure!(status.success(),"Credits scanner: {}",String::from_utf8_lossy(&error));
        ensure!(output.len()<=65536,"Oversized scan result");
        let result:Value=serde_json::from_slice(&output)?;
        ensure!(result["segments"].is_array() && result["duration"].as_f64().is_some_and(|d|d.is_finite()&&d>0.0),"Invalid credits result");
        Ok(result)
    }.await;
    if let Some(task)=task{task.abort();}
    let _=std::fs::remove_file(progress);result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unfinished_analysis_always_explains_itself() {
        let legacy=json!({"state":"incomplete","probes":16,"sample_errors":1,"segments":[]});
        assert_eq!(explain(legacy)["error"],"Credits scanner could not read 1 of 16 sampled positions");
        assert_eq!(explain(json!({"state":"failed","error":""}))["error"],"Credits scanner stopped without a result");
        let reported=json!({"state":"incomplete","sample_errors":2,"error":"Cannot Decode [AVFoundationErrorDomain -11821]"});
        assert_eq!(explain(reported.clone()),reported);
        let ready=json!({"state":"ready","segments":[],"sample_errors":0});
        assert_eq!(explain(ready.clone()),ready);
    }
    #[test]
    fn signed_off_files_with_older_analysis_are_requeued_for_credits_only() -> Result<()> {
        let db=Database{connection:rusqlite::Connection::open_in_memory()?};
        db.connection.execute_batch(include_str!("schema.sql"))?;
        for (id,version) in [("asset-old",1),("asset-new",VERSION)] {
            db.connection.execute("INSERT INTO media_processing(asset_id,revision,version,state,stage,inventory_state,subtitles_state,credits_state,updated) VALUES(?,'r',1,'ready','complete','ready','ready','ready',0)",[id])?;
            db.connection.execute("INSERT INTO credits_analysis VALUES(?,'r',?,'{}',0)",rusqlite::params![id,version])?;
        }
        assert_eq!(crate::processing::reopen_outdated(&db)?,1);
        let row:(String,String,String,String)=db.connection.query_row("SELECT state,stage,subtitles_state,credits_state FROM media_processing WHERE asset_id='asset-old'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        assert_eq!(row,("queued".into(),"credits".into(),"ready".into(),"pending".into()));
        assert_eq!(crate::processing::reopen_outdated(&db)?,0);
        Ok(())
    }
}
