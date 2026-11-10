//! Bounded, durable comparisons. Only newly added torrents in isolated trial roots are owned.
use crate::{api::Service,storage::now};
use anyhow::{Result,Context,ensure};
use serde_json::{Value,json};
use std::{path::Path,sync::LazyLock};
static LOCK:LazyLock<tokio::sync::Mutex<()>>=LazyLock::new(||tokio::sync::Mutex::new(()));
const CAP:u64=256*1024*1024;
const SECONDS:f64=300.0;
pub fn trial_path(path:&str)->bool {Path::new(path).components().any(|c|c.as_os_str()==".torrbaby-trials")}
fn save(s:&Service,v:&Value)->Result<()> {
 s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?.connection.execute("INSERT OR REPLACE INTO torrent_trials VALUES(?,?)",rusqlite::params![v["id"].as_str(),v.to_string()])?;Ok(())
}
pub fn list(s:&Service)->Result<Value> {
 let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
 let rows=db.rows("SELECT document FROM torrent_trials ORDER BY rowid DESC LIMIT 30",[])?;
 Ok(json!({"trials":rows.iter().map(|r|serde_json::from_str::<Value>(r["document"].as_str().unwrap_or("{}"))).collect::<std::result::Result<Vec<_>,_>>()?}))
}
fn owned(c:&Value,t:&Value)->bool {
 c["hash"].as_str().is_some_and(|h|h==t["hash_string"].as_str().unwrap_or("")) && c["directory"]==t["download_dir"] && trial_path(t["download_dir"].as_str().unwrap_or(""))
}
pub async fn create(s:&Service,body:&Value)->Result<Value> {
 let _lock=LOCK.lock().await;
 let sources=body["candidates"].as_array().context("Choose 3 to 5 releases")?;
 ensure!((3..=5).contains(&sources.len()),"Choose 3 to 5 releases");
 ensure!(!list(s)?["trials"].as_array().unwrap().iter().any(|v|matches!(v["state"].as_str(),Some("starting"|"running"|"finishing"))),"A trial is already running");
 for candidate in sources {
  ensure!(candidate["name"].as_str().is_some_and(|n|!n.is_empty() && n.len()<4096),"Invalid release name");
  let source=candidate["source"].as_object().context("Invalid torrent source")?;
  ensure!(source.len()==1 && source.iter().all(|(k,v)|["magnet","torrent_url","metainfo"].contains(&k.as_str()) && v.as_str().is_some_and(|s|!s.is_empty())),"Invalid torrent source");
 }
 let id=uuid::Uuid::new_v4().to_string();
 let root=std::path::PathBuf::from(s.effective_config()?.source).join(".torrbaby-trials").join(&id);
 let candidates:Vec<_>=sources.iter().enumerate().map(|(i,v)|json!({"name":v["name"],"source":v["source"],"listing_key":v["listing_key"],"directory":root.join(i.to_string()),"state":"pending","bytes":0,"speed":0,"peers":0,"sending":0})).collect();
 let v=json!({"id":id,"state":"starting","created":now(),"duration":SECONDS,"byte_limit":CAP,"candidates":candidates});
 save(s,&v)?;Ok(v)
}
pub async fn action(s:&Service,id:&str,body:&Value)->Result<Value> {
 let _lock=LOCK.lock().await;
 let mut v=list(s)?["trials"].as_array().unwrap().iter().find(|v|v["id"]==id).cloned().context("Trial not found")?;
 ensure!(matches!(v["state"].as_str(),Some("running"|"review")),"Trial cannot be changed now");
 if body["cancel"]==true {v["winner"]=Value::Null;} else {
 let hash=body["winner"].as_str().context("Choose a release")?;
 ensure!(v["candidates"].as_array().unwrap().iter().any(|c|c["hash"]==hash && c["state"]!="duplicate"),"Release is not owned by this trial");
 v["winner"]=json!(hash);
 }
 v["state"]=json!("finishing");v["error"]=Value::Null;save(s,&v)?;Ok(v)
}
fn best(candidates:&[Value])->Option<String> {
 candidates.iter().filter(|c|c["bytes"].as_u64().unwrap_or(0)>0 && c["state"]!="error" && c["state"]!="duplicate")
 .max_by(|a,b|a["speed"].as_f64().unwrap_or(0.).total_cmp(&b["speed"].as_f64().unwrap_or(0.)))
 .and_then(|c|c["hash"].as_str().map(str::to_owned))
}
pub async fn tick(s:&Service)->Result<()> {
 let _lock=LOCK.lock().await;
 let trials=list(s)?;
 for mut v in trials["trials"].as_array().unwrap().iter().cloned() {
  if !matches!(v["state"].as_str(),Some("starting"|"running"|"finishing")){continue;}
  if let Err(error)=advance(s,&mut v).await {
   let all=crate::organizer::torrents(s).await?;
   for c in v["candidates"].as_array().unwrap() {if let Some(t)=all.iter().find(|t|owned(c,t)){s.rpc.call("torrent_stop",json!({"ids":[t["hash_string"]]})).await?;}}
   v["error"]=json!(error.to_string());if v["state"]!="starting"{v["state"]=json!("review");}save(s,&v)?;
   s.event("trial_needs_attention",json!({"name":"Trial needs attention"}))?;
   tracing::warn!(trial=?v["id"],%error,"Torrent trial will retry");
  }
 }
 Ok(())
}
async fn advance(s:&Service,v:&mut Value)->Result<()> {
 if v["state"]=="starting" {
  let len=v["candidates"].as_array().unwrap().len();
  for i in 0..len {
   if v["candidates"][i]["state"]!="pending" {continue;}
   let dir=v["candidates"][i]["directory"].as_str().context("Missing trial directory")?.to_owned();
   // Reconcile an add that succeeded immediately before a crash/timeout.
   let all=crate::organizer::torrents(s).await?;
   let recovered=all.iter().find(|t|t["download_dir"]==dir);
   let result=if let Some(t)=recovered {Ok(json!({"torrent_added":{"hash_string":t["hash_string"]}}))} else {
    std::fs::create_dir_all(&dir)?;
    let mut body=v["candidates"][i]["source"].clone();body["paused"]=json!(true);
    crate::organizer::add_at(s,&body,Some(Path::new(&dir))).await
   };
   match result {
    Ok(r)=>{
     if let Some(hash)=r["torrent_added"]["hash_string"].as_str().or(r["torrent_duplicate"]["hash_string"].as_str()) {
      if let Some(key)=v["candidates"][i]["listing_key"].as_str().filter(|s|!s.is_empty()) {
       s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?.connection.execute("INSERT OR REPLACE INTO search_torrent_hashes VALUES(?,?)",rusqlite::params![key,hash.to_ascii_lowercase()])?;
      }
     }
     if r["torrent_duplicate"].is_object() {v["candidates"][i]["state"]=json!("duplicate");}
     else if let Some(h)=r["torrent_added"]["hash_string"].as_str() {v["candidates"][i]["hash"]=json!(h);v["candidates"][i]["state"]=json!("ready");}
    },
    Err(e)=>{
     // A timed-out add may have succeeded. Reconcile before relinquishing ownership.
     let current=crate::organizer::torrents(s).await?;
     if let Some(t)=current.iter().find(|t|t["download_dir"]==dir){v["candidates"][i]["hash"]=t["hash_string"].clone();v["candidates"][i]["state"]=json!("ready");}
     else {v["candidates"][i]["error"]=json!(e.to_string());v["candidates"][i]["state"]=json!("error");}
    }
   }
   save(s,v)?;
  }
  for c in v["candidates"].as_array().unwrap() {
   if c["state"]!="ready"{continue;}
   s.rpc.call("torrent_set",json!({"ids":[c["hash"]],"download_limited":true,"download_limit":8192})).await?;
   s.rpc.call("torrent_start",json!({"ids":[c["hash"]]})).await?;
  }
  v["started"]=json!(now());v["state"]=json!("running");save(s,v)?;
 }
 if v["state"]=="running" {
  let all=crate::organizer::torrents(s).await?;
  let elapsed=(now()-v["started"].as_f64().unwrap_or(now())).max(1.);
  for c in v["candidates"].as_array_mut().unwrap() {
   if c["state"]=="paused" {continue;}
   let Some(t)=all.iter().find(|t|owned(c,t)) else {continue;};
   let bytes=t["downloaded_ever"].as_u64().unwrap_or(0);
   c["bytes"]=json!(bytes);c["elapsed"]=json!(elapsed);c["speed"]=json!(bytes as f64/elapsed);
   c["peers"]=t["peers_connected"].clone();c["sending"]=t["peers_sending_to_us"].clone();
   if bytes>=CAP || elapsed>=SECONDS || t["percent_done"]==1.0 {
    s.rpc.call("torrent_stop",json!({"ids":[c["hash"]]})).await?;c["state"]=json!("paused");
   }
  }
  save(s,v)?;
  let candidates=v["candidates"].as_array().unwrap();
  if elapsed>=SECONDS || candidates.iter().all(|c|c["state"]!="ready") {
   if let Some(winner)=best(candidates){v["winner"]=json!(winner);v["state"]=json!("finishing");}
   else {v["state"]=json!("review");s.event("trial_needs_attention",json!({"name":"No trial made progress"}))?;}
   save(s,v)?;
  }
 }
 if v["state"]=="finishing" {
  let _mutation=s.mutation_lock.lock().await;
  // First promote the winner. Do not delete alternatives until its move succeeds.
  let winner=v["winner"].as_str().map(str::to_owned);
  if let Some(hash)=&winner {
   let c=v["candidates"].as_array().unwrap().iter().find(|c|c["hash"]==*hash).context("Winner missing")?;
   let t=crate::organizer::torrent(s,hash).await?;
   let destination=s.effective_config()?.source;
   if t["download_dir"]!=destination {
    ensure!(owned(c,&t),"Trial ownership changed; cleanup stopped");
    for file in t["files"].as_array().into_iter().flatten() {
     let name=file["name"].as_str().context("Invalid torrent filename")?;
     let top=Path::new(name).components().next().context("Invalid torrent filename")?;
     ensure!(matches!(top,std::path::Component::Normal(_)),"Invalid torrent filename");
     ensure!(!Path::new(&destination).join(top.as_os_str()).exists(),"Destination already exists; trial paused for review");
    }
    s.rpc.call("torrent_set_location",json!({"ids":[hash],"location":destination,"move":true})).await?;
    return Ok(());
   }
   s.rpc.call("torrent_set",json!({"ids":[hash],"download_limited":false})).await?;
   s.rpc.call("torrent_start",json!({"ids":[hash]})).await?;
  }
  let all=crate::organizer::torrents(s).await?;
  for c in v["candidates"].as_array_mut().unwrap() {
   if c["hash"].as_str()==winner.as_deref() || c["hash"].is_null() || c["state"]=="removed" {continue;}
   if let Some(t)=all.iter().find(|t|t["hash_string"]==c["hash"]) {
    ensure!(owned(c,t),"Trial ownership changed; cleanup stopped");
    s.rpc.call("torrent_remove",json!({"ids":[c["hash"]],"delete_local_data":true})).await?;
   }
   c["state"]=json!("removed");
  }
  v["state"]=json!(if winner.is_some(){"complete"}else{"cancelled"});save(s,v)?;
  s.event("trial_complete",json!({"hash":winner,"name":v["candidates"].as_array().unwrap().iter().find(|c|c["hash"]==v["winner"]).map(|c|c["name"].clone())}))?;
  s.invalidate_torrents();
 }
 Ok(())
}
#[cfg(test)] mod tests {
 use super::*;
 #[test] fn selection_requires_real_transfer(){assert!(best(&[json!({"hash":"a","bytes":0,"elapsed":300.,"speed":0.})]).is_none());assert_eq!(best(&[json!({"hash":"a","bytes":2000000,"elapsed":40.,"speed":50000.}),json!({"hash":"b","bytes":4000000,"elapsed":40.,"speed":100000.})]),Some("b".into()));}
 #[test] fn cleanup_requires_hash_and_isolated_path(){let c=json!({"hash":"a","directory":"/media/.torrbaby-trials/id/0"});assert!(owned(&c,&json!({"hash_string":"a","download_dir":"/media/.torrbaby-trials/id/0"})));assert!(!owned(&c,&json!({"hash_string":"a","download_dir":"/media/Movies"})));assert!(!owned(&c,&json!({"hash_string":"b","download_dir":"/media/.torrbaby-trials/id/0"})));}
}
