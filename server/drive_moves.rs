//! "Move to drive…": moving a title's files to another drive, only when asked
//! (docs/disk-aggregation.md). Nothing is deleted until its copy has been read back and
//! matches; a file being played or processed waits; asset ids are kept, so history,
//! subtitles and credits follow, and finished processing is carried over, not redone.
//!
//! A move is a list of units, each one torrent's files or one loose video with its
//! sidecars. Each unit goes copying → switched (the library and Transmission point at the
//! copy) → done (the originals are gone). The step is saved after each, so a restart
//! resumes where it was. Copies are written beside their destination as `….torrbaby-moving`
//! and renamed into place only once verified.
//!
//! POST /v1/drives/moves/preview  {"asset_ids": [...]}                where they are, what fits
//! POST /v1/drives/moves          {"asset_ids": [...], "drive": "beta"}
//! GET  /v1/drives/moves          moves, newest first, with progress
use crate::{api::Service, drives::{Drive, Drives}, storage::now};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Unit {
    /// The torrent whose files these are, or none for a loose file.
    pub torrent: Option<String>,
    /// Root the files are under on the source drive, and on the target.
    pub from: PathBuf,
    pub to: PathBuf,
    /// Paths relative to the roots, with sizes.
    pub files: Vec<(String, u64)>,
    pub step: String,
    pub error: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Move {
    pub id: String,
    pub drive: String,
    pub asset_ids: Vec<String>,
    pub units: Vec<Unit>,
    pub state: String,
    pub created: f64,
    pub bytes: u64,
    pub done_bytes: u64,
    pub error: Option<String>,
}

static MOVES: Mutex<Option<Vec<Move>>> = Mutex::new(None);
/// Bytes copied of the unit being copied, for progress between saves.
static COPIED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn file(s: &Service) -> PathBuf { s.data_dir.join("drive-moves.json") }
fn load(s: &Service) -> Vec<Move> {
    let mut moves = MOVES.lock().unwrap_or_else(|e| e.into_inner());
    moves.get_or_insert_with(|| {
        let all: Vec<Move> = std::fs::read(file(s)).ok().and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or_default();
        // After a restart, originals of switched units are still on their way out.
        if let Ok(mut leaving) = LEAVING.lock() {
            for unit in all.iter().flat_map(|m| &m.units).filter(|u| u.step == "switched") { leaving.extend(unit.files.iter().map(|(n, _)| unit.from.join(n))); }
        }
        all
    }).clone()
}
fn store(s: &Service, all: &[Move]) -> Result<()> {
    let staged = file(s).with_extension("json.new");
    std::fs::write(&staged, serde_json::to_vec(all)?)?;
    std::fs::rename(staged, file(s))?;
    *MOVES.lock().unwrap_or_else(|e| e.into_inner()) = Some(all.to_vec());
    Ok(())
}
fn update(s: &Service, id: &str, change: impl FnOnce(&mut Move)) -> Result<()> {
    let mut all = load(s);
    if let Some(m) = all.iter_mut().find(|m| m.id == id) { change(m); }
    store(s, &all)
}

pub async fn route(s: &Service, method: &str, path: &str, body: &Value) -> Option<Result<Value>> {
    Some(match (method, path) {
        ("GET", "/v1/drives/moves") => Ok(list(s)),
        ("POST", "/v1/drives/moves/preview") => preview(s, body).await,
        ("POST", "/v1/drives/moves") => { let _guard = s.mutation_lock.lock().await; start(s, body).await }
        _ => return None,
    })
}

fn list(s: &Service) -> Value {
    let copying = COPIED.load(std::sync::atomic::Ordering::Relaxed);
    let mut all = load(s);
    all.sort_by(|a, b| b.created.total_cmp(&a.created));
    json!({"moves": all.iter().take(50).map(|m| json!({"id":m.id,"drive":m.drive,"asset_ids":m.asset_ids,"state":m.state,"created":m.created,
        "bytes":m.bytes,"done_bytes":if m.state == "running" { (m.done_bytes + copying).min(m.bytes) } else { m.done_bytes },"error":m.error,
        "waiting":m.units.iter().find_map(|u| u.error.clone().filter(|_| u.step == "copying"))})).collect::<Vec<_>>()})
}

fn assets(s: &Service, ids: &[String]) -> Result<Vec<Value>> {
    let db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
    ids.iter().map(|id| db.asset(id)).collect()
}

fn ids(body: &Value) -> Result<Vec<String>> {
    let ids: Vec<String> = body["asset_ids"].as_array().context("Expected asset_ids")?.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect();
    ensure!(!ids.is_empty() && ids.len() <= 5000, "Choose the files to move");
    Ok(ids)
}

/// The units that move these assets to `target` (None: just where they are now).
async fn plan(s: &Service, drives: &Drives, assets: &[Value], target: Option<&Drive>) -> Result<(Vec<Unit>, Vec<String>)> {
    let torrents = crate::organizer::torrents(s).await.unwrap_or_default();
    let mut units: Vec<Unit> = Vec::new();
    let mut on = Vec::new();
    for asset in assets {
        let path = PathBuf::from(asset["path"].as_str().context("A file has no path")?);
        let (drive, role, root) = drives.root_of(&path).context("A file is outside the library")?;
        if !on.contains(&drive.id) { on.push(drive.id.clone()); }
        ensure!(asset["offline"] != true, "That drive is not connected");
        let Some(target) = target else { continue };
        if target.id == drive.id { continue; }
        let to = target.root(role).to_path_buf();
        let torrent = asset["torrent_hash"].as_str().and_then(|hash| torrents.iter().find(|t| t["hash_string"] == hash));
        if let Some(t) = torrent {
            let hash = t["hash_string"].as_str().unwrap_or_default().to_owned();
            if units.iter().any(|u| u.torrent.as_deref() == Some(hash.as_str())) { continue; }
            ensure!(crate::organizer::complete(t), "Wait until the download has finished");
            let dir = PathBuf::from(t["download_dir"].as_str().unwrap_or_default());
            let (_, role, root) = drives.root_of(&dir).filter(|(_, _, r)| *r == dir).context("A download is outside the library folders")?;
            let files = t["files"].as_array().into_iter().flatten()
                .map(|f| Ok((f["name"].as_str().context("Invalid torrent file")?.to_owned(), f["length"].as_u64().unwrap_or(0)))).collect::<Result<Vec<_>>>()?;
            units.push(Unit { torrent: Some(hash), from: root.to_path_buf(), to: target.root(role).to_path_buf(), files, step: "copying".into(), error: None });
        } else {
            let relative = path.strip_prefix(root)?.to_path_buf();
            if units.iter().any(|u| u.torrent.is_none() && u.files.iter().any(|(f, _)| Path::new(f) == relative)) { continue; }
            // The video and the files beside it that belong to it (its subtitles).
            let stem = path.file_stem().and_then(|s| s.to_str()).context("Invalid file name")?.to_owned();
            let mut files = vec![(relative.to_string_lossy().into_owned(), std::fs::metadata(&path)?.len())];
            if let Some(parent) = path.parent() {
                for entry in std::fs::read_dir(parent)?.flatten() {
                    let other = entry.path();
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if other == path || !entry.file_type().is_ok_and(|t| t.is_file()) || !(name.starts_with(&format!("{stem}.")) ) { continue; }
                    if crate::scanner::video(&other) { continue; }
                    files.push((other.strip_prefix(root)?.to_string_lossy().into_owned(), entry.metadata()?.len()));
                }
            }
            units.push(Unit { torrent: None, from: root.to_path_buf(), to, files, step: "copying".into(), error: None });
        }
    }
    Ok((units, on))
}

async fn preview(s: &Service, body: &Value) -> Result<Value> {
    let drives = Drives::of(&s.effective_config()?);
    let assets = assets(s, &ids(body)?)?;
    let (_, on) = plan(s, &drives, &assets, None).await?;
    let mut choices = Vec::new();
    for drive in drives.all() {
        if on.len() == 1 && on[0] == drive.id { continue; }
        let online = drive.online();
        let (units, _) = if online { plan(s, &drives, &assets, Some(drive)).await? } else { (Vec::new(), Vec::new()) };
        let bytes: u64 = units.iter().flat_map(|u| u.files.iter().map(|(_, size)| size)).sum();
        let free = if online { crate::drives::space(drive).1 } else { 0 };
        choices.push(json!({"id":drive.id,"name":drive.name,"online":online,"free_bytes":free,"bytes":bytes,"fits":online && free > bytes + 2_000_000_000}));
    }
    Ok(json!({"on":on,"choices":choices,"moving":load(s).iter().any(|m| matches!(m.state.as_str(), "queued" | "running") && m.asset_ids.iter().any(|id| assets.iter().any(|a| a["id"] == id.as_str())))}))
}

async fn start(s: &Service, body: &Value) -> Result<Value> {
    let drives = Drives::of(&s.effective_config()?);
    let target = drives.all().iter().find(|d| Some(d.id.as_str()) == body["drive"].as_str()).context("Choose a drive")?;
    ensure!(target.online(), "That drive is not connected");
    let ids = ids(body)?;
    ensure!(!load(s).iter().any(|m| matches!(m.state.as_str(), "queued" | "running") && m.asset_ids.iter().any(|id| ids.contains(id))),
        "These files are already moving");
    let assets = assets(s, &ids)?;
    let (units, _) = plan(s, &drives, &assets, Some(target)).await?;
    ensure!(!units.is_empty(), "Already on that drive");
    let bytes: u64 = units.iter().flat_map(|u| u.files.iter().map(|(_, size)| size)).sum();
    ensure!(crate::drives::space(target).1 > bytes + 2_000_000_000, "Not enough free space on that drive");
    let m = Move { id: uuid::Uuid::new_v4().simple().to_string(), drive: target.id.clone(), asset_ids: ids, units, state: "queued".into(),
        created: now(), bytes, done_bytes: 0, error: None };
    let mut all = load(s);
    all.retain(|m| now() - m.created < 30.0 * 86400.0 || matches!(m.state.as_str(), "queued" | "running"));
    all.push(m.clone());
    store(s, &all)?;
    s.event("drive_move_queued", json!({"id":m.id,"drive":m.drive,"bytes":bytes}))?;
    Ok(json!({"id":m.id,"state":m.state,"bytes":bytes}))
}

/// Advance the oldest unfinished move by one unit. Called every few seconds.
pub async fn tick(s: &Service) -> Result<()> {
    let Some(mut m) = load(s).into_iter().filter(|m| matches!(m.state.as_str(), "queued" | "running")).min_by(|a, b| a.created.total_cmp(&b.created)) else { return Ok(()) };
    let Some(index) = m.units.iter().position(|u| u.step != "done") else {
        update(s, &m.id, |m| { m.state = "done".into(); m.done_bytes = m.bytes; })?;
        s.event("drive_move_done", json!({"id":m.id}))?;
        return crate::api::scan_library(s).await;
    };
    if m.state == "queued" { update(s, &m.id, |m| m.state = "running".into())?; m.state = "running".into(); }
    let mut unit = m.units[index].clone();
    loop {
        match advance(s, &unit).await {
            Ok(step) => {
                update(s, &m.id, |m| {
                    if step == "done" { m.done_bytes += m.units[index].files.iter().map(|(_, size)| size).sum::<u64>(); }
                    m.units[index].step = step.clone(); m.units[index].error = None;
                })?;
                // Straight on from the switch to removing the originals.
                if step != "switched" { return Ok(()); }
                unit.step = step;
            }
            Err(stop) => return stopped(s, &m, index, &unit, stop),
        }
    }
}

fn stopped(s: &Service, m: &Move, index: usize, unit: &Unit, stop: Stop) -> Result<()> {
    match stop {
        Wait(reason) => update(s, &m.id, |m| m.units[index].error = Some(reason)),
        Fail(error) => {
            // The originals are untouched until a unit is switched; a failed copy only leaves its partial file, removed here.
            if unit.step == "copying" { discard_partials(&unit); }
            set_moving(s, &unit, false).ok();
            s.event("drive_move_failed", json!({"id":m.id,"error":error.to_string()}))?;
            update(s, &m.id, |m| { m.state = "failed".into(); m.error = Some(error.to_string()); })
        }
    }
}

/// Originals whose copies the library already uses, until they are removed. The scanner
/// must not take one for a new file.
static LEAVING: std::sync::LazyLock<Mutex<std::collections::HashSet<PathBuf>>> = std::sync::LazyLock::new(Default::default);
pub fn leaving(path: &Path) -> bool { LEAVING.lock().is_ok_and(|l| l.contains(path)) }

enum Stop { Wait(String), Fail(anyhow::Error) }
use Stop::{Fail, Wait};
impl<E: Into<anyhow::Error>> From<E> for Stop { fn from(error: E) -> Self { Fail(error.into()) } }

fn staged(path: &Path) -> PathBuf { PathBuf::from(format!("{}.torrbaby-moving", path.display())) }
fn discard_partials(unit: &Unit) {
    for (name, _) in &unit.files { let _ = std::fs::remove_file(staged(&unit.to.join(name))); }
}

/// The library records whose files are in this unit.
fn unit_assets(s: &Service, unit: &Unit) -> Result<Vec<Value>> {
    let files: Vec<PathBuf> = unit.files.iter().map(|(name, _)| unit.from.join(name)).collect();
    let db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
    Ok(db.documents("assets")?.into_iter().filter(|a| a["path"].as_str().is_some_and(|p| files.iter().any(|f| f == Path::new(p)))
        || unit.torrent.as_ref().is_some_and(|h| a["torrent_hash"] == h.as_str())).collect())
}

/// While moving, processing leaves the files alone and the scanner keeps their records.
fn set_moving(s: &Service, unit: &Unit, moving: bool) -> Result<()> {
    let ids: Vec<String> = unit_assets(s, unit)?.iter().filter_map(|a| a["id"].as_str().map(str::to_owned)).collect();
    let db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
    for id in ids {
        let sql = if moving { "UPDATE library_assets SET document=json_set(document,'$.moving',json('true')) WHERE id=?" }
            else { "UPDATE library_assets SET document=json_remove(document,'$.moving') WHERE id=?" };
        db.connection.execute(sql, [id])?;
    }
    Ok(())
}

async fn advance(s: &Service, unit: &Unit) -> std::result::Result<String, Stop> {
    match unit.step.as_str() {
        "copying" => {
            let assets = unit_assets(s, unit)?;
            busy(s, &assets)?;
            set_moving(s, unit, true)?;
            let copy = unit.clone();
            COPIED.store(0, std::sync::atomic::Ordering::Relaxed);
            tokio::task::spawn_blocking(move || copy_unit(&copy)).await.map_err(|e| Fail(e.into()))??;
            COPIED.store(0, std::sync::atomic::Ordering::Relaxed);
            switch(s, unit, &assets).await?;
            Ok("switched".into())
        }
        "switched" => {
            // The library and Transmission use the copies now; the originals can go,
            // once nothing started playing them during the copy.
            busy(s, &unit_assets(s, unit)?)?;
            for (name, size) in &unit.files {
                let original = unit.from.join(name);
                let copy = unit.to.join(name);
                if !original.exists() { continue; }
                if std::fs::metadata(&copy).map(|c| c.len()) .ok() != Some(*size) { return Err(Fail(anyhow::anyhow!("A copy on the new drive changed; the original is kept"))); }
                std::fs::remove_file(&original)?;
                remove_empty_parents(&original, &unit.from);
            }
            set_moving(s, unit, false)?;
            if let Ok(mut leaving) = LEAVING.lock() { for (name, _) in &unit.files { leaving.remove(&unit.from.join(name)); } }
            Ok("done".into())
        }
        other => Err(Fail(anyhow::anyhow!("Unknown move step {other}"))),
    }
}

/// Playing or processing files wait; nothing is moved underneath an open file.
fn busy(s: &Service, assets: &[Value]) -> std::result::Result<(), Stop> {
    let db = s.db.lock().map_err(|_| Fail(anyhow::anyhow!("Database unavailable")))?;
    let playing: Vec<String> = db.rows("SELECT DISTINCT s.media_id FROM playback_sessions s JOIN profile_playback p ON p.active_session=s.id WHERE s.ended=0 AND p.last_played>?", [now() - 90.0])?
        .into_iter().filter_map(|v| v["media_id"].as_str().map(str::to_owned)).collect();
    for asset in assets {
        let id = asset["id"].as_str().unwrap_or_default();
        let media = format!("{}:{}", asset["torrent_hash"].as_str().unwrap_or(id), asset["file_index"].as_u64().unwrap_or(0));
        if playing.iter().any(|p| *p == id || *p == media) { return Err(Wait("Waiting for playback to stop".into())); }
        let running: bool = db.connection.query_row("SELECT EXISTS(SELECT 1 FROM media_processing WHERE asset_id=? AND state='running')", [id], |r| r.get(0))?;
        if running { return Err(Wait("Waiting for processing to finish".into())); }
    }
    Ok(())
}

/// Copy every file, read each copy back and compare, then put them in place.
fn copy_unit(unit: &Unit) -> std::result::Result<(), Stop> {
    use sha2::Digest;
    use std::io::{Read, Write};
    for (name, size) in &unit.files {
        let (original, copy) = (crate::api::checked_path(&unit.from, name)?, crate::api::checked_path(&unit.to, name)?);
        let meta = std::fs::metadata(&original)?;
        if meta.len() != *size { return Err(Fail(anyhow::anyhow!("A file changed since the move was planned"))); }
        if let Ok(existing) = std::fs::metadata(&copy) {
            // Already copied before a restart, or a different file in the way.
            if existing.len() == *size && digest(&copy)? == digest(&original)? { continue; }
            return Err(Fail(anyhow::anyhow!("A file with the same name is already on that drive")));
        }
        crate::drives::create_media_folder(copy.parent().context("Invalid destination")?)?;
        let partial = staged(&copy);
        let mut hasher = sha2::Sha256::new();
        {
            let mut from = std::fs::File::open(&original)?;
            let mut to = std::fs::File::create(&partial)?;
            let mut buffer = vec![0u8; 8 << 20];
            loop {
                let n = from.read(&mut buffer)?;
                if n == 0 { break; }
                hasher.update(&buffer[..n]);
                to.write_all(&buffer[..n])?;
                COPIED.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
            }
            to.sync_all()?;
        }
        let read_back = digest(&partial)?;
        if read_back != hasher.finalize().to_vec() || std::fs::metadata(&partial)?.len() != *size {
            let _ = std::fs::remove_file(&partial);
            return Err(Fail(anyhow::anyhow!("A copy did not match its original; the original is kept")));
        }
        // Same modification time, so finished subtitle work recognises the file.
        let mtime = meta.modified()?;
        std::fs::File::options().write(true).open(&partial)?.set_modified(mtime)?;
        std::fs::rename(&partial, &copy)?;
    }
    Ok(())
}

fn digest(path: &Path) -> Result<Vec<u8>> {
    use sha2::Digest;
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0u8; 8 << 20];
    loop { let n = file.read(&mut buffer)?; if n == 0 { break; } hasher.update(&buffer[..n]); }
    Ok(hasher.finalize().to_vec())
}

/// Point the library, its finished processing, and Transmission at the copies.
async fn switch(s: &Service, unit: &Unit, assets: &[Value]) -> Result<()> {
    if let Ok(mut leaving) = LEAVING.lock() { for (name, _) in &unit.files { leaving.insert(unit.from.join(name)); } }
    if let Some(hash) = &unit.torrent {
        // Transmission reads the verified copies from now on; it moves nothing itself.
        s.rpc.call("torrent_set_location", json!({"ids":[hash],"location":unit.to,"move":false})).await?;
        s.invalidate_torrents();
    }
    for asset in assets {
        let Some(id) = asset["id"].as_str() else { continue };
        let moved = |key: &str| asset[key].as_str().and_then(|p| Path::new(p).strip_prefix(&unit.from).ok()).map(|r| unit.to.join(r));
        let Some(path) = moved("path") else { continue };
        let revision = crate::subtitle_jobs::revision(&path).ok();
        {
            let db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
            let mut document = asset.clone();
            document["path"] = json!(path);
            if let Some(early) = moved("early_path") { document["early_path"] = json!(early); }
            if let Ok(meta) = std::fs::metadata(&path) {
                use std::os::unix::fs::MetadataExt;
                document["inode"] = json!(format!("{}:{}", meta.dev(), meta.ino()));
            }
            db.connection.execute("UPDATE library_assets SET document=? WHERE id=?", rusqlite::params![document.to_string(), id])?;
            if let Some(revision) = &revision {
                crate::processing::follow(&db, id, revision)?;
            }
        }
        s.subtitle_jobs.relocate(id, &path);
    }
    Ok(())
}

fn remove_empty_parents(file: &Path, root: &Path) {
    for dir in file.ancestors().skip(1) {
        if dir == root || !dir.starts_with(root) { break; }
        if std::fs::remove_dir(dir).is_err() { break; }
    }
}
