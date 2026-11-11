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
            let mut info=serde_json::from_str(&text)?;
            normalize_prepared(&mut info);
            return Ok(info);
        }
    }
    Ok(json!({"state":"pending","audio_tracks":[],"subtitle_tracks":[],"prepared":[]}))
}
// The tb badge identifies Torrbaby-created captions: OCR or translation.
fn normalize_prepared(info:&mut Value) {
    let mut identified=Vec::new();
    if let Some(prepared)=info["prepared"].as_array_mut() {
        for job in prepared {
            job["generated"]=json!(job["target"].as_str().is_some_and(|s|s!="auto") || job["extraction_method"]=="ocr");
            if job["state"]=="ready" && job["sidecar"].is_null() {
                if let (Some(track),Some(source))=(job["track"].as_u64(),job["source"].as_str()) {
                    if source!="auto" {identified.push((track,source.to_owned()));}
                }
            }
        }
    }
    if let Some(tracks)=info["subtitle_tracks"].as_array_mut() {
        for track in tracks {
            if track["sidecar"]!=true && track["language"].as_str().is_none_or(|s|s.is_empty() || s=="und") {
                if let Some((_,source))=identified.iter().find(|(id,_)|track["track"]==*id) {track["language"]=json!(source);}
            }
        }
    }
}
fn save(db: &Database, id: &str, revision: &str, info: &Value) -> Result<()> {
    db.connection.execute("INSERT INTO media_inventory VALUES(?,?,?,?) ON CONFLICT(asset_id) DO UPDATE SET revision=excluded.revision,document=excluded.document,updated=excluded.updated",
        rusqlite::params![id,revision,info.to_string(),now()])?;
    Ok(())
}
pub fn update_sidecar_languages(db: &Database, id: &str, tracks: &[crate::subtitles::Sidecar]) -> Result<()> {
    let mut info = cached(db, id)?;
    if info["state"] != "ready" { return Ok(()); }
    if let Some(existing) = info["subtitle_tracks"].as_array_mut() {
        for item in existing.iter_mut().filter(|t| t["sidecar"] == true) {
            if let Some(track) = tracks.iter().find(|t| item["name"].as_str() == Some(t.name.as_str())) {
                item["language"] = serde_json::to_value(&track.language)?;
                item["id"] = json!(track.id); item["track"] = json!(1); item["extraction_supported"] = json!(true);
            }
        }
    }
    let asset = db.asset(id)?;
    if let Some(path) = asset["path"].as_str() {
        save(db, id, &crate::subtitle_jobs::revision(Path::new(path))?, &info)?;
    }
    Ok(())
}

fn language(track: &Value) -> Option<&'static str> {
    let tag = track["language"].as_str()?.to_ascii_lowercase();
    if tag == "en" || tag == "eng" || tag.starts_with("en-") { Some("en") }
    else if tag == "zh" || tag == "zho" || tag == "chi" || tag.starts_with("zh-") { Some("zh-Hans") }
    else { None }
}

/// Prefer existing full captions over translation, and text over OCR in each language.
/// Equal source/target means extraction only; it never reaches the translator.
fn subtitle_plan(tracks: &[Value]) -> Vec<(&Value, &'static str, &'static str)> {
    let full = |t: &&Value| t["forced"] != true;
    // Older inventories have no ocr_supported field; those tracks remain eligible.
    let bitmap = |t: &&Value| (t["codec"] == "S_VOBSUB" || t["codec"] == "S_HDMV/PGS") && t["ocr_supported"] != false;
    let text = |lang| tracks.iter().filter(full).find(|t| language(t) == Some(lang) && t["text_extractable"] == true);
    let image = |lang| tracks.iter().filter(full).filter(bitmap).find(|t| language(t) == Some(lang));
    let has = |lang| text(lang).is_some() || image(lang).is_some();
    if has("en") && has("zh-Hans") {
        return ["en", "zh-Hans"].into_iter().filter_map(|lang|
            text(lang).or_else(||image(lang)).map(|t| (t, lang, lang))).collect();
    }
    let (source, target) = if has("zh-Hans") { ("zh-Hans", "en") } else { ("en", "zh-Hans") };
    if let Some(t) = text(source) {
        return if t["extraction_supported"] == true { vec![(t, source, target)] } else { vec![] };
    }
    if let Some(t) = image(source) { return vec![(t, source, target)]; }
    let unknown = |t: &&Value| t["language"].as_str().is_none_or(|s| s.is_empty() || s == "und");
    if let Some(t) = tracks.iter().filter(full).filter(unknown).find(|t| t["extraction_supported"] == true) {
        return vec![(t, "auto", "auto")];
    }
    if tracks.iter().filter(full).any(|t| t["text_extractable"] == true || t["sidecar"] == true) { return vec![]; }
    tracks.iter().filter(full).filter(unknown).find(bitmap)
        .map(|t| vec![(t, "auto", "auto")]).unwrap_or_default()
}
#[cfg(test)]
fn subtitle_source(tracks: &[Value]) -> Option<(&Value, &'static str, &'static str)> {
    subtitle_plan(tracks).into_iter().next()
}

pub(crate) fn priorities(db: &Database) -> Result<std::collections::HashMap<String, i64>> {
    let mut statement = db.connection.prepare("SELECT asset_id,priority FROM subtitle_priorities")?;
    Ok(statement.query_map([], |row| Ok((row.get(0)?,row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn prioritize(db: &Database, id: &str) -> Result<Value> {
    let asset = db.asset(id)?;
    anyhow::ensure!(asset["available"] == true && asset["complete"] == true,
        "Wait for this download to finish before preparing subtitles");
    db.connection.execute("INSERT INTO subtitle_priorities(asset_id,priority) VALUES(?,(SELECT COALESCE(MAX(priority),0)+1 FROM subtitle_priorities)) ON CONFLICT(asset_id) DO UPDATE SET priority=excluded.priority", [id])?;
    db.connection.execute("UPDATE media_processing SET state='queued',retry_after=0,error=NULL WHERE asset_id=? AND state='failed'",[id])?;
    Ok(json!({"accepted":true}))
}

fn group_key(asset: &Value) -> String {
    asset["entity_id"].as_str().or(asset["series_name"].as_str()).or(asset["id"].as_str()).unwrap_or("").to_owned()
}
pub(crate) fn queue_assets(db: &Database) -> Result<Vec<Value>> {
    let entities: std::collections::HashMap<String,Value> = db.documents("entities")?.into_iter()
        .filter_map(|e| e["id"].as_str().map(str::to_owned).map(|id|(id,e))).collect();
    let mut assets=db.visible_assets()?;
    for a in &mut assets {
        a["queue_title"]=a["entity_id"].as_str().and_then(|id|entities.get(id)).map(|e|e["title"].clone())
            .unwrap_or_else(||a["series_name"].clone());
    }
    Ok(assets)
}
pub(crate) fn order_assets(assets: &mut [Value], priorities: &std::collections::HashMap<String,i64>) {
    let mut groups=std::collections::HashMap::<String,i64>::new();
    for a in assets.iter() {
        let priority=*priorities.get(a["id"].as_str().unwrap_or("")).unwrap_or(&0);
        let entry=groups.entry(group_key(a)).or_default(); *entry=(*entry).max(priority);
    }
    let title=|a:&Value|a["queue_title"].as_str().or(a["series_name"].as_str()).or(a["id"].as_str()).unwrap_or("").to_lowercase();
    assets.sort_by(|a,b| groups[&group_key(b)].cmp(&groups[&group_key(a)])
        .then_with(||title(a).cmp(&title(b)))
        .then_with(||group_key(a).cmp(&group_key(b)))
        .then_with(|| priorities.get(b["id"].as_str().unwrap_or("")).unwrap_or(&0)
            .cmp(priorities.get(a["id"].as_str().unwrap_or("")).unwrap_or(&0)))
        .then_with(||a["season"].as_u64().unwrap_or(0).cmp(&b["season"].as_u64().unwrap_or(0)))
        .then_with(||a["episode"].as_u64().unwrap_or(0).cmp(&b["episode"].as_u64().unwrap_or(0)))
        .then_with(||a["id"].as_str().cmp(&b["id"].as_str())));
}

const INVENTORY_VERSION:u64=4;
/// Advance the subtitle stages of one file. The SQLite processing coordinator owns
/// scheduling; artifact jobs are implementation details, never queue membership.
pub async fn process(s: &Service, asset: &Value) -> Result<Value> {
    let id=asset["id"].as_str().context("Missing asset ID")?;
    let input=crate::api::media_file(s,id,0).await?;
    let revision=crate::subtitle_jobs::revision(&input)?;
    let mut info={let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;cached(&db,id)?};
    // Version 4: sidecars in legacy encodings are labelled and advert-only files dropped.
    if info["state"]!="ready" || info["language_inventory_version"].as_u64().is_none_or(|v|v<INVENTORY_VERSION) {
        let inventory=crate::native_subtitles::tracks(&input).await?;
        let sidecar_input=input.clone();
        let sidecars=tokio::task::spawn_blocking(move ||crate::subtitles::discover(&sidecar_input)).await??;
        let mut tracks=inventory["tracks"].as_array().cloned().unwrap_or_default();
        tracks.extend(sidecars.into_iter().map(|t|json!({"id":t.id,"track":1,"language":t.language,"name":t.name,"sidecar":true,"text_extractable":true,"extraction_supported":true,"forced":t.forced})));
        info=json!({"state":"ready","language_inventory_version":INVENTORY_VERSION,"audio_tracks":inventory["audio_tracks"],"subtitle_tracks":tracks,"scanned_at":now()});
    }
    let tracks=info["subtitle_tracks"].as_array().context("Invalid track inventory")?;
    let plan=subtitle_plan(tracks);
    let mut planned=Vec::new();
    for (source,language,target) in plan {
        let track=source["track"].as_u64().context("Missing subtitle track")?;
        let source_input=if source["sidecar"]==true {
            let input=input.clone();let id=source["id"].as_str().context("Missing sidecar ID")?.to_owned();
            tokio::task::spawn_blocking(move ||crate::subtitles::sidecar_path(&input,&id)).await??
        }else{input.clone()};
        planned.push((source_input.clone(),track,language,None));
        if language!=target || target=="auto" {planned.push((source_input,track,language,Some(target)));}
    }
    let mut jobs=Vec::new();let mut prepared=Vec::new();let mut complete=true;
    let mut progress=json!({"state":"ready","stage":"subtitles","completed_cues":0,"total_cues":0});
    // Bitmap tracks found to hold no visible captions (placeholders) are no source.
    let mut empty_tracks=std::collections::HashSet::new();
    // Sequential stages: extraction must finish before its translation starts.
    for (path,track,language,target) in &planned {
        if empty_tracks.contains(&(path.clone(),*track)) {continue;}
        let source=Some((*language).to_owned());let target=target.map(str::to_owned);
        let existing=s.subtitle_jobs.lookup(id,path,*track,&source,&target);
        if target.is_none() && existing.as_ref().is_some_and(|job| job["state"]=="failed"
            && job["error"]==crate::ocr_subtitles::NO_VISIBLE_CAPTIONS) {
            empty_tracks.insert((path.clone(),*track));
            continue;
        }
        let job=match existing {
            Some(job) if job["state"]=="ready" => job,
            Some(job) if job["state"]=="running" || job["state"]=="queued" => job,
            // Awaiting the temporary external lane; with it off, translate on device.
            Some(job) if job["state"]=="external" && s.subtitle_jobs.external() => job,
            Some(job) if job["state"]=="failed" => {
                // Coordinator supplies the retry boundary; preserve the error until then.
                if asset["retrying"]==true && complete && s.subtitle_jobs.can_admit() {
                    s.subtitle_jobs.prepare(id,path.clone(),*track,source,target)?
                } else {job}
            },
            _ if complete && s.subtitle_jobs.can_admit() => s.subtitle_jobs.prepare(id,path.clone(),*track,source,target)?,
            _ => json!({"state":"queued","stage":if target.is_some(){"translation"}else{"extraction"},"target":target}),
        };
        if !job["id"].is_null(){jobs.push(job["id"].clone());}
        if job["state"]=="ready" {
            let mut ready=job.clone();ready["language"]=ready["target"].as_str().or(ready["source"].as_str()).map(|v|json!(v)).unwrap_or(json!("und"));
            ready["generated"]=json!(!ready["target"].is_null() || ready["extraction_method"]=="ocr");prepared.push(ready);
        } else if complete {
            progress=job;complete=false;
        }
    }
    if planned.is_empty() {
        // No supported source is explicit, never silently signed off as translated.
        progress["state"]=json!("not_applicable");
        progress["reason"]=json!("No supported English or Chinese subtitle source");
    } else if planned.iter().all(|(path,track,..)| empty_tracks.contains(&(path.clone(),*track))) {
        progress["state"]=json!("not_applicable");
        progress["reason"]=json!("The English and Chinese subtitle tracks contain no visible captions");
    }
    info["prepared"]=json!(prepared);info["jobs"]=json!(jobs);
    info["preparation_state"]=json!(if complete {"ready"}else if progress["state"]=="failed" {"failed"}else{"preparing"});
    {let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;save(&db,id,&revision,&info)?;}
    Ok(progress)
}

/// Compatibility endpoint: one row per file, backed by the same durable queue.
pub fn summary(db: &Database, _jobs: &crate::subtitle_jobs::Manager) -> Result<Value> {
    crate::processing::summary(db)
}

#[cfg(test)]
mod priority_tests {
    use super::*;
    #[test]
    fn shows_stay_together_but_clicked_episode_moves_to_front() {
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
        assert_eq!(assets.iter().map(|a|a["id"].as_str().unwrap()).collect::<Vec<_>>(),vec!["b2","b1","a1","a2"]);
        priorities.insert("a2".into(), 2);
        order_assets(&mut assets, &priorities);
        assert_eq!(assets.iter().map(|a|a["id"].as_str().unwrap()).collect::<Vec<_>>(),vec!["a2","a1","b2","b1"]);
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

#[cfg(test)]
mod ocr_fallback_tests {
    use super::*;
    #[test]
    fn chinese_text_gets_english_only_when_missing() {
        let chinese=json!({"track":2,"text_extractable":true,"extraction_supported":true,"language":"zh-Hans"});
        let tracks=vec![chinese.clone()];
        let (_, source, target)=subtitle_source(&tracks).unwrap();
        assert_eq!((source,target),("zh-Hans","en"));
        let english=json!({"track":3,"text_extractable":true,"extraction_supported":true,"language":"eng"});
        let tracks=[chinese,english];let plan=subtitle_plan(&tracks);
        assert_eq!(plan.len(),2);assert!(plan.iter().all(|(_,source,target)|source==target));
    }
    #[test]
    fn existing_bitmap_captions_are_extracted_and_forced_tracks_do_not_count_as_full() {
        let chinese=json!({"track":2,"text_extractable":true,"extraction_supported":true,"language":"zh-Hans"});
        let english=json!({"track":3,"codec":"S_HDMV/PGS","language":"eng"});
        let tracks=vec![chinese.clone(),english.clone()];
        let plan=subtitle_plan(&tracks);
        assert_eq!(plan.len(),2);
        assert_eq!((plan[0].0["track"].as_u64(),plan[0].1,plan[0].2),(Some(3),"en","en"));
        let tracks=vec![english,json!({"track":4,"codec":"S_VOBSUB","language":"zh"})];
        let plan=subtitle_plan(&tracks);
        assert_eq!(plan.len(),2);
        assert!(plan.iter().all(|(_,source,target)|source==target));
        let forced=json!({"track":3,"text_extractable":true,"extraction_supported":true,"language":"en","forced":true});
        let tracks=vec![chinese,forced];
        let (_,source,target)=subtitle_source(&tracks).unwrap();
        assert_eq!((source,target),("zh-Hans","en"));
    }
    #[test]
    fn text_and_sidecars_always_precede_bitmap_ocr() {
        let bitmap=json!({"track":3,"codec":"S_VOBSUB","language":null});
        let text=json!({"track":4,"text_extractable":true,"extraction_supported":true,"language":"en"});
        let tracks=vec![bitmap.clone(),text.clone()];assert_eq!(subtitle_source(&tracks).unwrap().0["track"],4);
        let tracks=vec![bitmap.clone(),json!({"sidecar":true,"text_extractable":true,"extraction_supported":true,"language":"en","track":1})];assert_eq!(subtitle_source(&tracks).unwrap().0["sidecar"],true);
        let tracks=vec![bitmap.clone(),json!({"text_extractable":true,"extraction_supported":false,"language":"en"})];assert!(subtitle_source(&tracks).is_none());
        let tracks=vec![bitmap];assert_eq!(subtitle_source(&tracks).unwrap().1,"auto");
        let pgs=json!({"codec":"S_HDMV/PGS","track":5,"language":"en"});
        assert_eq!(subtitle_source(&[pgs.clone()]).unwrap().1,"en");
        assert_eq!(subtitle_source(&[pgs,text.clone()]).unwrap().0["track"],4);
        let tracks=vec![text,json!({"track":5,"text_extractable":true,"extraction_supported":true,"language":"zh-Hans"})];let plan=subtitle_plan(&tracks);assert_eq!(plan.len(),2);assert!(plan.iter().all(|(_,source,target)|source==target));
    }
    #[test]
    fn bitmap_tracks_with_unsupported_encodings_are_not_planned_for_ocr() {
        // zlib-compressed PGS (the mkvmerge default) is supported; inventories
        // written before ocr_supported existed keep their tracks eligible.
        let compressed=json!({"track":10,"codec":"S_HDMV/PGS","language":"chi","encoded":true,"ocr_supported":true});
        assert_eq!(subtitle_source(&[compressed]).unwrap().0["track"],10);
        let legacy=json!({"track":10,"codec":"S_HDMV/PGS","language":"chi","encoded":true});
        assert_eq!(subtitle_source(&[legacy]).unwrap().0["track"],10);
        let encrypted=json!({"track":10,"codec":"S_HDMV/PGS","language":"chi","encoded":true,"ocr_supported":false});
        let english=json!({"track":4,"codec":"S_HDMV/PGS","language":"eng","ocr_supported":true});
        assert!(subtitle_plan(&[encrypted.clone()]).is_empty());
        assert_eq!(subtitle_plan(&[encrypted,english]).iter().map(|(t,s,d)|(t["track"].as_u64(),*s,*d)).collect::<Vec<_>>(),[(Some(4),"en","zh-Hans")]);
    }
}

#[cfg(test)]
mod caption_provenance_tests {
    use super::*;
    #[test]
    fn ocr_and_translation_keep_badges_and_identify_source_language() {
        let mut info=json!({"subtitle_tracks":[{"track":3,"language":null,"codec":"S_VOBSUB"}],"prepared":[
            {"track":3,"state":"ready","source":"en","target":null,"generated":true,"extraction_method":"ocr"},
            {"track":3,"state":"ready","source":"en","target":"zh-Hans","generated":true,"extraction_method":"ocr"}]});
        normalize_prepared(&mut info);
        assert_eq!(info["prepared"][0]["generated"],true);
        assert_eq!(info["prepared"][1]["generated"],true);
        assert_eq!(info["subtitle_tracks"][0]["language"],"en");
    }
}
