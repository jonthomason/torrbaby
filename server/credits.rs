//! Bounded, revision-keyed rolling credits stage of the durable processing queue.
use crate::{api::Service, storage::{Database, now}};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use rusqlite::OptionalExtension;
use std::{path::Path, process::Stdio, sync::Arc, time::Duration};

const VERSION:i64=1;
pub fn cached(db:&Database,id:&str)->Result<Value> {
    let asset=db.asset(id)?;
    let row:Option<(String,String)>=db.connection.query_row("SELECT revision,document FROM credits_analysis WHERE asset_id=? AND version=?",rusqlite::params![id,VERSION],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((revision,text))=row {
        if asset["path"].as_str().and_then(|p|crate::subtitle_jobs::revision(Path::new(p)).ok()).as_deref()==Some(&revision) {return Ok(serde_json::from_str(&text)?);}
    }
    Ok(json!({"state":"pending","segments":[]}))
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
    if !s.virtual_media.idle() {return Ok(());}
    for asset in std::iter::once(asset) {
        if asset["available"]!=true || asset["complete"]!=true || asset["role"]=="extra" {continue;}
        let Some(id)=asset["id"].as_str() else {continue};
        let Some(path)=asset["path"].as_str() else {continue};
        let path=Path::new(path);
        if !path.extension().and_then(|e|e.to_str()).is_some_and(|e|["mp4","m4v","mov","mkv"].contains(&e.to_lowercase().as_str())) {continue;}
        let Ok(revision)=crate::subtitle_jobs::revision(path) else {continue};
        {let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
            let prior:Option<(String,i64,String,f64)>=db.connection.query_row("SELECT revision,version,document,updated FROM credits_analysis WHERE asset_id=?",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
            if let Some((rev,version,text,_updated))=prior {
                let info:Value=serde_json::from_str(&text)?;
                if rev==revision && version==VERSION && info["state"]=="ready"{continue;}
            }
        }
        // Resolve through the same complete-file/access checks as playback.
        let input=crate::api::media_file(s,id,0).await?;
        let mut reader=None;
        if path.extension().is_some_and(|e|e.eq_ignore_ascii_case("mkv")) {
            let job=s.virtual_media.prepare(id,input.clone(),Vec::new())?;
            if job["state"]!="ready" {
                if job["state"]=="failed" {
                    let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
                    save(&db,id,&revision,&json!({"state":"failed","error":job["error"]}))?;
                }
                return Ok(());
            }
            reader=Some(s.virtual_media.reader(job["id"].as_str().context("Missing index")?,id,&input).await?);
        }
        {let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;save(&db,id,&revision,&json!({"state":"running"}))?;}
        let outcome=scan(s,&input,reader,if asset["kind"]=="tv" || asset["season"].is_number(){300}else{900}).await;
        let document=match outcome {Ok(result)=>result,Err(error)=>json!({"state":if error.to_string().contains("deferred") {"deferred"}else{"failed"},"error":error.to_string()})};
        if crate::subtitle_jobs::revision(&input).ok().as_deref()==Some(&revision) {
            let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;save(&db,id,&revision,&document)?;
        }
        tracing::info!(asset=id,state=?document["state"],elapsed=?document["elapsed_seconds"],"Credits analysis finished");
        return Ok(());
    }
    Ok(())
}
async fn scan(s:&Service,path:&Path,reader:Option<Arc<crate::virtual_captions::Reader>>,window:u32)->Result<Value> {
    use axum::{Router,routing::any,http::{Method,HeaderMap},response::IntoResponse};
    // Ephemeral loopback-only capability: no media credentials in argv or files.
    let mut task=None;
    let input=if let Some(reader)=reader {
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let key=uuid::Uuid::new_v4().to_string();
        let url=format!("http://127.0.0.1:{}/{key}.mp4",listener.local_addr()?.port());
        let route=format!("/{key}.mp4");
        let router=Router::new().route(&route,any(move |method:Method,headers:HeaderMap| {let reader=reader.clone();async move {
            match crate::api::virtual_response(reader,"credits",&method,&headers).await {Ok(r)=>r,Err(_)=>axum::http::StatusCode::BAD_REQUEST.into_response()}
        }}));
        task=Some(tokio::spawn(async move {let _=axum::serve(listener,router).await;}));url
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
        let status=loop {
            if let Some(status)=child.try_wait()?{break status;}
            if started.elapsed()>Duration::from_secs(300) || s.virtual_media.preparing() {
                child.kill().await?;let _=child.wait().await;out.abort();err.abort();
                anyhow::bail!("Credits scan deferred for playback preparation or time budget");
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
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
