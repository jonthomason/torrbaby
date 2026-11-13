//! "Move to drive…": moving a title's files to another drive, only when asked
//! (docs/disk-aggregation.md). Nothing is deleted until its copy has been read back and
//! matches; a file being played or processed waits; asset ids are kept, so history,
//! subtitles and credits follow, and finished processing is carried over, not redone.
//!
//! A move is a list of units, each one torrent's files or one loose video with its
//! sidecars. Each unit goes copying → switched (the library and Transmission point at the
//! copy) → done (the originals are gone). The step is saved after each, so a restart
//! resumes where it was. Copies are written beside their destination as `….torrbaby-moving`
//! and renamed into place only once every file of the unit is verified, together with the
//! switch, under the lock library scans take, so a scan never sees a copy the library
//! does not yet use, nor an original it no longer does.
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
    /// Emptying a drive (`from`): the torrents Transmission moves itself, unfinished
    /// downloads and Junk, one at a time before the units. Kept here so a restart goes on.
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub carry: Vec<String>,
    #[serde(default)]
    pub carry_wait: Option<String>,
}

static MOVES: Mutex<Option<Vec<Move>>> = Mutex::new(None);
/// Bytes copied of the unit being copied, for progress between saves.
static COPIED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn file(s: &Service) -> PathBuf { s.data_dir.join("drive-moves.json") }
fn load(s: &Service) -> Vec<Move> {
    let mut moves = MOVES.lock().unwrap_or_else(|e| e.into_inner());
    moves.get_or_insert_with(|| {
        let all: Vec<Move> = std::fs::read(file(s)).ok().and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or_default();
        // After a restart: originals of switched units are still on their way out, and
        // copies of units not yet switched are not the library's yet.
        if let Ok(mut leaving) = LEAVING.lock() {
            for unit in all.iter().filter(|m| active(m)).flat_map(|m| &m.units) {
                match unit.step.as_str() {
                    "switched" => leaving.extend(unit.files.iter().map(|(n, _)| unit.from.join(n))),
                    "copying" => leaving.extend(unit.files.iter().map(|(n, _)| unit.to.join(n))),
                    _ => {}
                }
            }
        }
        all
    }).clone()
}
fn store(s: &Service, all: &[Move]) -> Result<()> {
    let staged = file(s).with_extension("json.new");
    {
        use std::io::Write;
        let mut out = std::fs::File::create(&staged)?;
        out.write_all(&serde_json::to_vec(all)?)?;
        out.sync_all()?;
    }
    std::fs::rename(staged, file(s))?;
    sync_dir(&s.data_dir);
    *MOVES.lock().unwrap_or_else(|e| e.into_inner()) = Some(all.to_vec());
    Ok(())
}
/// Held across a read, change and save of the list, so two writers do not lose either's change.
static SAVING: Mutex<()> = Mutex::new(());
fn update(s: &Service, id: &str, change: impl FnOnce(&mut Move)) -> Result<()> {
    let _saving = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load(s);
    if let Some(m) = all.iter_mut().find(|m| m.id == id) { change(m); }
    store(s, &all)
}
fn active(m: &Move) -> bool { matches!(m.state.as_str(), "queued" | "running") }
/// Whether a torrent's files are being moved: the organizer and cleanup leave it alone.
pub fn moving_torrent(s: &Service, hash: &str) -> bool {
    load(s).iter().filter(|m| active(m)).flat_map(|m| &m.units)
        .any(|u| u.step != "done" && u.torrent.as_deref().is_some_and(|h| h.eq_ignore_ascii_case(hash)))
}
/// Whether any of these library files is in an unfinished move.
pub fn moving_assets(s: &Service, ids: &[String]) -> bool {
    load(s).iter().any(|m| active(m) && m.asset_ids.iter().any(|id| ids.contains(id)))
}
/// Whether a move onto `drive` has not finished.
pub fn moving_to(s: &Service, drive: &str) -> bool { load(s).iter().any(|m| active(m) && m.drive == drive) }
/// Whether an unfinished move still has files to take from, or put under, this folder.
pub fn moving_under(s: &Service, media: &Path) -> bool {
    load(s).iter().any(|m| active(m) && m.units.iter().any(|u| u.step != "done" && (u.from.starts_with(media) || u.to.starts_with(media))))
}
/// Bytes still to be copied onto a drive by moves already queued.
fn incoming(s: &Service, drive: &str) -> u64 {
    load(s).iter().filter(|m| active(m) && m.drive == drive).flat_map(|m| &m.units).filter(|u| u.step == "copying")
        .flat_map(|u| u.files.iter().map(|(_, size)| size)).sum()
}
/// Room for `bytes` more on `drive`, keeping its reserve and what other moves will bring.
fn fits(s: &Service, drive: &Drive, bytes: u64) -> bool {
    crate::drives::space(drive).1 > bytes.saturating_add(incoming(s, &drive.id)).saturating_add(drive.reserve_bytes)
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
        "carrying":m.carry.len(),
        "waiting":m.carry_wait.clone().filter(|_| !m.carry.is_empty()).or_else(|| m.units.iter().find_map(|u| u.error.clone().filter(|_| u.step != "done")))})).collect::<Vec<_>>()})
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
/// `whole_drive`: a drive is being emptied, so a download still under way is passed over
/// here (Transmission carries it across itself, see `carry_one`) instead of refusing.
async fn plan(s: &Service, drives: &Drives, assets: &[Value], target: Option<&Drive>, whole_drive: bool) -> Result<(Vec<Unit>, Vec<String>)> {
    // Without Transmission's list a torrent's files would be taken for loose ones and moved
    // from under it, so a move is not planned then.
    let torrents = match crate::organizer::torrents(s).await {
        Ok(torrents) => torrents,
        Err(error) if target.is_some() => return Err(error),
        Err(_) => Vec::new(),
    };
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
            if whole_drive && !crate::organizer::complete(t) { continue; }
            ensure!(crate::organizer::complete(t), "Wait until the download has finished");
            let dir = PathBuf::from(t["download_dir"].as_str().unwrap_or_default());
            let (from, role, root) = drives.root_of(&dir).filter(|(_, _, r)| *r == dir).context("A download is outside the library folders")?;
            // Transmission already uses the target (an earlier move stopped part-way): the
            // next scan puts the records there; copying onto itself would delete the files.
            if from.id == target.id { continue; }
            // A listed file that is no longer on disk (a sample or note removed since) has
            // nothing to copy; the files beside each video that belong to it (subtitles
            // added later) are not the torrent's, and go with it all the same.
            let mut files: Vec<(String, u64)> = Vec::new();
            for f in t["files"].as_array().into_iter().flatten() {
                let name = f["name"].as_str().context("Invalid torrent file")?;
                let Ok(meta) = std::fs::metadata(crate::api::checked_path(root, name)?) else { continue };
                if meta.is_file() { files.push((crate::drives::spelling(name), meta.len())); }
            }
            ensure!(!files.is_empty(), "This download's files are not on the drive");
            for (name, _) in files.clone() {
                let video = root.join(&name);
                if !crate::scanner::video(&video) { continue; }
                for extra in beside(&video, root)? { if !files.iter().any(|(have, _)| *have == extra.0) { files.push(extra); } }
            }
            units.push(Unit { torrent: Some(hash), from: root.to_path_buf(), to: target.root(role).to_path_buf(), files, step: "copying".into(), error: None });
        } else {
            let relative = path.strip_prefix(root)?.to_path_buf();
            if units.iter().any(|u| u.torrent.is_none() && u.files.iter().any(|(f, _)| Path::new(f) == relative)) { continue; }
            // The video and the files beside it that belong to it (its subtitles).
            let mut files = vec![(relative.to_string_lossy().into_owned(), std::fs::metadata(&path)?.len())];
            files.extend(beside(&path, root)?);
            units.push(Unit { torrent: None, from: root.to_path_buf(), to, files, step: "copying".into(), error: None });
        }
    }
    // A file is in one unit only. A torrent's file can also have a record of its own (a copy
    // found before Transmission pointed there); the torrent moves it, and a second unit would
    // find it gone.
    let carried: std::collections::HashSet<PathBuf> = units.iter().filter(|u| u.torrent.is_some())
        .flat_map(|u| u.files.iter().map(|(name, _)| u.from.join(name))).collect();
    for unit in units.iter_mut().filter(|u| u.torrent.is_none()) { unit.files.retain(|(name, _)| !carried.contains(&unit.from.join(name))); }
    units.retain(|u| !u.files.is_empty());
    Ok((units, on))
}

/// The files beside a video that belong to it (its subtitles), relative to `root`.
fn beside(path: &Path, root: &Path) -> Result<Vec<(String, u64)>> {
    let mut files = Vec::new();
    let Some(parent) = path.parent() else { return Ok(files) };
    let entries: Vec<_> = std::fs::read_dir(parent)?.flatten().collect();
    // Names are compared in one spelling: a Mac OS Extended drive lists an accented name
    // decomposed, whatever spelling the video is known by.
    let spell = |name: &std::ffi::OsStr| crate::drives::spelling(&name.to_string_lossy());
    let me = spell(path.file_name().context("Invalid file name")?);
    let stem = spell(path.file_stem().context("Invalid file name")?);
    // "Film.2160p.en.srt" belongs to "Film.2160p.mkv", not to "Film.mkv".
    let longer: Vec<String> = entries.iter().map(|e| e.path()).filter(|p| crate::scanner::video(p))
        .filter_map(|p| p.file_stem().map(spell))
        .filter(|other| other.len() > stem.len() && other.starts_with(&format!("{stem}."))).collect();
    for entry in entries {
        let other = entry.path();
        let name = spell(&entry.file_name());
        if name == me || !entry.file_type().is_ok_and(|t| t.is_file()) || !name.starts_with(&format!("{stem}.")) { continue; }
        // Not another video, another video's files, a download in progress or a copy in transit.
        if crate::scanner::video(&other) || longer.iter().any(|l| name.starts_with(&format!("{l}."))) { continue; }
        if name.ends_with(".part") || name.ends_with(".partial") || name.ends_with(".progress") || name.ends_with(".torrbaby-moving") { continue; }
        files.push((crate::drives::spelling(&other.strip_prefix(root)?.to_string_lossy()), entry.metadata()?.len()));
    }
    Ok(files)
}

async fn preview(s: &Service, body: &Value) -> Result<Value> {
    let drives = Drives::of(&s.effective_config()?);
    let assets = assets(s, &ids(body)?)?;
    let (_, on) = plan(s, &drives, &assets, None, false).await?;
    let mut choices = Vec::new();
    for drive in drives.all() {
        if on.len() == 1 && on[0] == drive.id { continue; }
        let online = drive.online();
        let (units, _) = if online { plan(s, &drives, &assets, Some(drive), false).await? } else { (Vec::new(), Vec::new()) };
        let bytes: u64 = units.iter().flat_map(|u| u.files.iter().map(|(_, size)| size)).sum();
        let free = if online { crate::drives::space(drive).1 } else { 0 };
        choices.push(json!({"id":drive.id,"name":drive.name,"online":online,"free_bytes":free,"bytes":bytes,"fits":online && fits(s, drive, bytes)}));
    }
    Ok(json!({"on":on,"choices":choices,"moving":load(s).iter().any(|m| matches!(m.state.as_str(), "queued" | "running") && m.asset_ids.iter().any(|id| assets.iter().any(|a| a["id"] == id.as_str())))}))
}

async fn start(s: &Service, body: &Value) -> Result<Value> {
    let drives = Drives::of(&s.effective_config()?);
    let target = drives.all().iter().find(|d| Some(d.id.as_str()) == body["drive"].as_str()).context("Choose a drive")?;
    ensure!(target.online(), "That drive is not connected");
    // {"from": drive} empties that drive: every finished title on it, and the rest of what
    // Transmission keeps there.
    let from = match body["from"].as_str() {
        Some(id) => Some(drives.all().iter().find(|d| d.id == id).filter(|d| d.id != target.id).context("Choose a drive")?),
        None => None,
    };
    if let Some(from) = from { ensure!(from.online(), "That drive is not connected"); }
    let ids = match from {
        Some(from) => {
            let all = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?.documents("assets")?;
            let busy: Vec<String> = load(s).iter().filter(|m| active(m)).flat_map(|m| m.asset_ids.clone()).collect();
            all.iter().filter(|a| a["available"] == true && a["complete"] == true && a["moving"] != true)
                .filter(|a| a["path"].as_str().is_some_and(|p| drives.drive_of(Path::new(p)).is_some_and(|d| d.id == from.id)))
                .filter_map(|a| a["id"].as_str().map(str::to_owned)).filter(|id| !busy.contains(id)).collect()
        }
        None => ids(body)?,
    };
    ensure!(!load(s).iter().any(|m| matches!(m.state.as_str(), "queued" | "running") && m.asset_ids.iter().any(|id| ids.contains(id))),
        "These files are already moving");
    let assets = assets(s, &ids)?;
    let (mut units, _) = plan(s, &drives, &assets, Some(target), from.is_some()).await?;
    let carry = match from { Some(from) => leftovers(s, &drives, from, target, &units).await?, None => Vec::new() };
    if let Some(from) = from { units.extend(strays(s, from, target, &units).await?); }
    let carried = carry.len();
    if units.is_empty() && carry.is_empty() && from.is_some() { return Ok(json!({"state":"done","bytes":0,"carried":0})); }
    ensure!(!units.is_empty() || !carry.is_empty(), "Already on that drive");
    let bytes: u64 = units.iter().flat_map(|u| u.files.iter().map(|(_, size)| size)).sum();
    ensure!(fits(s, target, bytes), "Not enough free space on that drive");
    let m = Move { id: uuid::Uuid::new_v4().simple().to_string(), drive: target.id.clone(), asset_ids: ids, units, state: "queued".into(),
        created: now(), bytes, done_bytes: 0, error: None, from: from.map(|d| d.id.clone()), carry, carry_wait: None };
    let _saving = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load(s);
    all.retain(|m| now() - m.created < 30.0 * 86400.0 || active(m));
    all.push(m.clone());
    store(s, &all)?;
    s.event("drive_move_queued", json!({"id":m.id,"drive":m.drive,"bytes":bytes}))?;
    Ok(json!({"id":m.id,"state":m.state,"bytes":bytes,"carried":carried}))
}

/// Emptying a drive: what Transmission keeps there that is not a finished title of the
/// library — a download still under way, a discarded package seeding out its time in Junk.
/// Each goes to the same folder on the target, moved by Transmission itself, which is how a
/// download in progress stays one (`carry_one`).
async fn leftovers(s: &Service, drives: &Drives, from: &Drive, target: &Drive, units: &[Unit]) -> Result<Vec<String>> {
    let mut carry = Vec::new();
    for t in crate::organizer::torrents(s).await? {
        let (Some(hash), Some(dir)) = (t["hash_string"].as_str(), t["download_dir"].as_str()) else { continue };
        if units.iter().any(|u| u.torrent.as_deref() == Some(hash)) { continue; }
        if crate::trials::trial_path(dir) { continue; }
        if destination(drives, from, target, Path::new(dir)).is_some() { carry.push(hash.to_owned()); }
    }
    Ok(carry)
}

/// Emptying a drive: the files under its folders that no title or torrent takes (a subtitle
/// that no longer shares its video's name, a note), to the same place on the target, so
/// nothing is left behind. Files the library has a record of, files of a torrent, and
/// hidden ones stay.
async fn strays(s: &Service, from: &Drive, target: &Drive, units: &[Unit]) -> Result<Vec<Unit>> {
    use crate::drives::{spelled, Role};
    let torrents = crate::organizer::torrents(s).await?;
    let held: Vec<PathBuf> = torrents.iter().filter_map(|t| {
        let (dir, name) = (t["download_dir"].as_str()?, t["name"].as_str().filter(|n| !n.is_empty())?);
        Some(spelled(&Path::new(dir).join(name)))
    }).collect();
    let recorded: std::collections::HashSet<PathBuf> = {
        let db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
        db.documents("assets")?.iter().filter_map(|a| a["path"].as_str().map(|p| spelled(Path::new(p)))).collect()
    };
    let planned: std::collections::HashSet<PathBuf> = units.iter().flat_map(|u| u.files.iter().map(|(name, _)| spelled(&u.from.join(name)))).collect();
    let mut strays = Vec::new();
    for role in [Role::Movies, Role::Tv, Role::Downloads] {
        let root = from.root(role);
        let files = stray_files(root, &|path| held.iter().any(|h| path.starts_with(h)) || recorded.contains(path) || planned.contains(path))?;
        if !files.is_empty() { strays.push(Unit { torrent: None, from: root.to_path_buf(), to: target.root(role).to_path_buf(), files, step: "copying".into(), error: None }); }
    }
    Ok(strays)
}

/// The files under `root`, relative and with sizes, but for hidden ones, another move's
/// partial copies, and those `taken` (given composed paths).
fn stray_files(root: &Path, taken: &dyn Fn(&Path) -> bool) -> Result<Vec<(String, u64)>> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') || name.to_string_lossy().ends_with(".torrbaby-moving") { continue; }
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() { pending.push(path); continue; }
            if !kind.is_file() { continue; }
            let path = crate::drives::spelled(&path);
            if taken(&path) { continue; }
            let (Ok(relative), Ok(meta)) = (path.strip_prefix(crate::drives::spelled(root)), entry.metadata()) else { continue };
            files.push((relative.to_string_lossy().into_owned(), meta.len()));
        }
    }
    Ok(files)
}

/// Where a torrent kept in `dir` goes when `from` is emptied onto `target`: the same place
/// there, its Downloads, its library folder or its Junk. None when it is not on `from`.
fn destination(drives: &Drives, from: &Drive, target: &Drive, dir: &Path) -> Option<PathBuf> {
    if !drives.drive_of(dir).is_some_and(|d| d.id == from.id) && !dir.starts_with(&from.media) { return None; }
    let under = |base: &Path, rest: &Path| if rest.as_os_str().is_empty() { base.to_path_buf() } else { base.join(rest) };
    Some(match drives.root_of(dir).filter(|(d, _, _)| d.id == from.id) {
        Some((_, role, root)) => under(target.root(role), dir.strip_prefix(root).ok()?),
        None => under(&target.media, dir.strip_prefix(&from.media).ok()?),
    })
}

/// How long Transmission may take to move one torrent's files to another drive. It answers
/// nothing else meanwhile, so carries go one at a time.
const CARRY_SECONDS: u64 = 3600;

/// Hand Transmission the next torrent to carry. False when it could only wait.
async fn carry_one(s: &Service, m: &Move) -> Result<bool> {
    let hash = m.carry[0].clone();
    let wait = |reason: &str| { let reason = reason.to_owned(); update(s, &m.id, |m| m.carry_wait = Some(reason)).map(|_| false) };
    let drives = Drives::of(&s.effective_config()?);
    let from = m.from.as_deref().and_then(|id| drives.all().iter().find(|d| d.id == id));
    let Some((from, target)) = from.zip(drives.all().iter().find(|d| d.id == m.drive)) else {
        // A drive taken off the list since: there is nothing left to carry between.
        update(s, &m.id, |m| { m.carry.clear(); m.carry_wait = None; })?;
        return Ok(true);
    };
    if !from.online() || !target.online() { return wait("Waiting for the drive to be connected"); }
    s.invalidate_torrents();
    let Ok(torrents) = crate::organizer::torrents(s).await else { return wait("Waiting for Transmission") };
    // Read again each time: the organizer may have filed a finished download since, and a
    // carry sent before a restart may have been finished by Transmission meanwhile.
    let torrent = torrents.iter().find(|t| t["hash_string"] == hash.as_str());
    let to = torrent.and_then(|t| t["download_dir"].as_str()).and_then(|dir| destination(&drives, from, target, Path::new(dir)));
    let Some(to) = to else {
        update(s, &m.id, |m| { m.carry.retain(|h| *h != hash); m.carry_wait = None; })?;
        return Ok(true);
    };
    crate::drives::create_media_folder(&to)?;
    tracing::info!(hash, to = %to.display(), "Emptying a drive: Transmission is moving a torrent it still needs");
    // Its files turn up on the target while Transmission still lists them where they were:
    // until it is done they are in transit, not new files for the scanner to record.
    let arriving = torrent.and_then(|t| t["name"].as_str()).filter(|name| !name.is_empty())
        .map(|name| crate::drives::spelled(&to.join(name)));
    if let (Some(path), Ok(mut carrying)) = (&arriving, CARRYING.lock()) { carrying.insert(path.clone()); }
    let sent = s.rpc.call_within("torrent_set_location", json!({"ids":[hash],"location":to,"move":true}),
        Some(std::time::Duration::from_secs(CARRY_SECONDS))).await;
    s.invalidate_torrents();
    if let (Some(path), Ok(mut carrying)) = (&arriving, CARRYING.lock()) { carrying.remove(path); }
    match sent {
        // Taken off the list once Transmission is seen keeping it on the target, next time.
        Ok(_) => { update(s, &m.id, |m| m.carry_wait = None)?; Ok(true) }
        Err(error) => { tracing::warn!(hash, %error, "Emptying a drive: Transmission did not finish moving a torrent"); wait("Waiting for Transmission") }
    }
}

/// Advance the oldest unfinished move by one unit. Called every few seconds.
pub async fn tick(s: &Service) -> Result<()> {
    release_stale_flags(s);
    let mut waiting: Vec<Move> = load(s).into_iter().filter(active).collect();
    waiting.sort_by(|a, b| a.created.total_cmp(&b.created));
    // Oldest first; one that is waiting (for playback, for a drive) lets the next go on.
    for m in waiting { if step(s, m).await? { break; } }
    Ok(())
}

/// Once after a start: a record still marked as moving that no unfinished move owns (the
/// list of moves was lost) would be skipped by processing for ever.
fn release_stale_flags(s: &Service) {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.swap(true, std::sync::atomic::Ordering::Relaxed) { return; }
    let mut owned: std::collections::HashSet<String> = Default::default();
    for unit in load(s).iter().filter(|m| active(m)).flat_map(|m| &m.units).filter(|u| u.step != "done") {
        let Ok(assets) = unit_assets(s, unit) else { return };
        owned.extend(assets.iter().filter_map(|a| a["id"].as_str().map(str::to_owned)));
    }
    let Ok(db) = s.db.lock() else { return };
    let Ok(assets) = db.documents("assets") else { return };
    for id in assets.iter().filter(|a| a["moving"] == true).filter_map(|a| a["id"].as_str()).filter(|id| !owned.contains(*id)) {
        tracing::warn!(id, "Cleared a moving mark that no move owns");
        let _ = db.connection.execute("UPDATE library_assets SET document=json_remove(document,'$.moving') WHERE id=?", [id]);
    }
}

/// Advance one move by one unit. False when it could only wait.
async fn step(s: &Service, mut m: Move) -> Result<bool> {
    if !m.carry.is_empty() {
        if m.state == "queued" { update(s, &m.id, |m| m.state = "running".into())?; }
        return carry_one(s, &m).await;
    }
    let Some(index) = m.units.iter().position(|u| u.step != "done") else {
        update(s, &m.id, |m| { m.state = "done".into(); m.done_bytes = m.bytes; })?;
        s.event("drive_move_done", json!({"id":m.id}))?;
        crate::api::scan_library(s).await?;
        return Ok(true);
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
                if step != "switched" { return Ok(true); }
                unit.step = step;
            }
            Err(stop) => { let waits = matches!(stop, Wait(_)); stopped(s, &m, index, &unit, stop)?; return Ok(!waits); }
        }
    }
}

fn stopped(s: &Service, m: &Move, index: usize, unit: &Unit, stop: Stop) -> Result<()> {
    match stop {
        Wait(reason) => update(s, &m.id, |m| m.units[index].error = Some(reason)),
        Fail(error) => {
            // The originals are untouched until a unit is switched: a unit that fails before
            // that leaves only its partial files and unused copies, removed here.
            if unit.step == "copying" { discard_copies(unit); }
            if let Ok(mut leaving) = LEAVING.lock() {
                for (name, _) in &unit.files { leaving.remove(&unit.to.join(name)); if unit.step == "copying" { leaving.remove(&unit.from.join(name)); } }
            }
            set_moving(s, unit, false).ok();
            tracing::warn!(id = %m.id, from = %unit.from.display(), file = unit.files.first().map(|(name, _)| name.as_str()).unwrap_or_default(), %error, "Moving files between drives failed");
            s.event("drive_move_failed", json!({"id":m.id,"error":error.to_string()}))?;
            update(s, &m.id, |m| { m.state = "failed".into(); m.error = Some(error.to_string()); m.units[index].error = Some(error.to_string()); })
        }
    }
}

/// Files in transit: originals whose copies the library already uses, until they are
/// removed, and copies it does not use yet. The scanner must not take one for a new file.
static LEAVING: std::sync::LazyLock<Mutex<std::collections::HashSet<PathBuf>>> = std::sync::LazyLock::new(Default::default);
pub fn leaving(path: &Path) -> bool {
    LEAVING.lock().is_ok_and(|l| l.contains(path)) || CARRYING.lock().is_ok_and(|c| c.iter().any(|at| path.starts_with(at)))
}
/// Where Transmission is putting a torrent it is carrying to another drive: its file, or
/// its folder.
static CARRYING: std::sync::LazyLock<Mutex<std::collections::HashSet<PathBuf>>> = std::sync::LazyLock::new(Default::default);

enum Stop { Wait(String), Fail(anyhow::Error) }
use Stop::{Fail, Wait};
impl<E: Into<anyhow::Error>> From<E> for Stop { fn from(error: E) -> Self { Fail(error.into()) } }

/// Make a rename or removal in this folder last through a power cut.
fn sync_dir(dir: &Path) { if let Ok(dir) = std::fs::File::open(dir) { let _ = dir.sync_all(); } }
fn staged(path: &Path) -> PathBuf { PathBuf::from(format!("{}.torrbaby-moving", path.display())) }
/// Remove a unit's partial files. Only the temporary names are this move's own: a file
/// under the final name may have been on that drive before, and is never removed here.
fn discard_copies(unit: &Unit) {
    for (name, _) in &unit.files { let _ = std::fs::remove_file(staged(&unit.to.join(name))); }
}
/// Put a unit's verified copies under their final names, refusing to replace a file that
/// appeared there during the copy. Returns the ones placed, so a failure can take them back.
fn place_copies(unit: &Unit) -> (Vec<PathBuf>, Result<()>) {
    let mut placed = Vec::new();
    for (name, _) in &unit.files {
        let copy = unit.to.join(name);
        if !staged(&copy).exists() { continue; }
        if copy.exists() { return (placed, Err(anyhow::anyhow!("A file with the same name is already on that drive"))); }
        if let Err(error) = std::fs::rename(staged(&copy), &copy) { return (placed, Err(error.into())); }
        if let Some(parent) = copy.parent() { sync_dir(parent); }
        placed.push(copy);
    }
    (placed, Ok(()))
}
/// Whether two paths are one file; None when either is not there.
fn same_file(a: &Path, b: &Path) -> Option<bool> {
    use std::os::unix::fs::MetadataExt;
    let (a, b) = (std::fs::metadata(a).ok()?, std::fs::metadata(b).ok()?);
    Some(a.dev() == b.dev() && a.ino() == b.ino())
}
/// Both drives of a unit connected, or a reason to wait.
fn drives_present(s: &Service, unit: &Unit) -> std::result::Result<(), Stop> {
    let drives = Drives::of(&s.effective_config()?);
    let connected = |path: &Path| drives.drive_of(path).is_some_and(Drive::online);
    if connected(&unit.from) && connected(&unit.to) { Ok(()) } else { Err(Wait("Waiting for the drive to be connected".into())) }
}

/// The library records whose files are in this unit.
fn unit_assets(s: &Service, unit: &Unit) -> Result<Vec<Value>> {
    // Where they were, or, once switched, where they are now.
    let files: Vec<PathBuf> = unit.files.iter().flat_map(|(name, _)| [unit.from.join(name), unit.to.join(name)]).collect();
    let db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
    Ok(db.documents("assets")?.into_iter().filter(|a| a["path"].as_str().is_some_and(|p| files.iter().any(|f| *f == crate::drives::spelled(Path::new(p))))
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
            if unit.from == unit.to { return Err(Fail(anyhow::anyhow!("Already on that drive"))); }
            {
                let _scans = s.mutation_lock.lock().await;
                drives_present(s, unit)?;
                busy(s, &unit_assets(s, unit)?)?;
                set_moving(s, unit, true)?;
            }
            // Its copies are not the library's until the switch.
            if let Ok(mut leaving) = LEAVING.lock() { for (name, _) in &unit.files { leaving.insert(unit.to.join(name)); } }
            let copy = unit.clone();
            COPIED.store(0, std::sync::atomic::Ordering::Relaxed);
            let copied = tokio::task::spawn_blocking(move || copy_unit(&copy)).await.map_err(|e| Fail(e.into()))?;
            COPIED.store(0, std::sync::atomic::Ordering::Relaxed);
            if let Err(Fail(error)) = copied {
                // A drive pulled out during the copy shows as a read or write error: that
                // waits for the drive, and the copy starts again over its partial files.
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                drives_present(s, unit)?;
                return Err(Fail(error));
            }
            copied?;
            // Every copy is verified: put them in place and switch, with no scan in between.
            let _scans = s.mutation_lock.lock().await;
            drives_present(s, unit)?;
            let (placed, result) = place_copies(unit);
            let result = match result { Ok(()) => unit_assets(s, unit), Err(error) => Err(error) };
            let result = match result { Ok(assets) => switch(s, unit, &assets).await, Err(error) => Err(error) };
            if let Err(error) = result {
                if error.is::<NotSwitched>() {
                    // Transmission may still take the change it did not confirm, so the
                    // verified copies stay where it would look, kept from scans as copies the
                    // library does not use yet; the next try finds them and switches again.
                    if let Ok(mut leaving) = LEAVING.lock() {
                        for (name, _) in &unit.files { leaving.remove(&unit.from.join(name)); leaving.insert(unit.to.join(name)); }
                    }
                    return Err(Wait(error.to_string()));
                }
                // Back to the originals, which are untouched; the copies placed just now go.
                for copy in &placed { let _ = std::fs::remove_file(copy); }
                if let Some(hash) = &unit.torrent {
                    let _ = s.rpc.call("torrent_set_location", json!({"ids":[hash],"location":unit.from,"move":false})).await;
                    s.invalidate_torrents();
                }
                return Err(Fail(error));
            }
            Ok("switched".into())
        }
        "switched" => {
            // The library and Transmission use the copies now; the originals can go, once
            // nothing started playing them during the copy and both drives are here (a
            // missing original is only taken for removed while its drive is connected).
            let _scans = s.mutation_lock.lock().await;
            drives_present(s, unit)?;
            let assets = unit_assets(s, unit)?;
            busy(s, &assets)?;
            for (name, size) in &unit.files {
                let original = unit.from.join(name);
                let copy = unit.to.join(name);
                if !original.exists() { continue; }
                // The library already uses the copies. An original that cannot go yet stays
                // this move's to remove: given up on, it would turn up as a second title.
                if same_file(&original, &copy) != Some(false) || std::fs::metadata(&copy).map(|c| c.len()).ok() != Some(*size) {
                    return Err(Wait("A copy on the new drive changed; the original is kept".into()));
                }
                if let Err(error) = std::fs::remove_file(&original) { return Err(Wait(format!("Could not remove an original: {error}"))); }
                remove_empty_parents(&original, &unit.from);
            }
            set_moving(s, unit, false)?;
            if let Ok(mut leaving) = LEAVING.lock() { for (name, _) in &unit.files { leaving.remove(&unit.from.join(name)); } }
            // With the originals gone, unfinished subtitle work follows its file.
            for asset in &assets {
                if let (Some(id), Some(path)) = (asset["id"].as_str(), asset["path"].as_str()) { s.subtitle_jobs.relocate(id, Path::new(path)); }
            }
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

/// Copy every file beside its destination and read each copy back to compare. They are put
/// in place by the caller, all together, at the switch.
fn copy_unit(unit: &Unit) -> std::result::Result<(), Stop> {
    use sha2::Digest;
    use std::io::{Read, Write};
    for (name, size) in &unit.files {
        let (original, copy) = (crate::api::checked_path(&unit.from, name)?, crate::api::checked_path(&unit.to, name)?);
        let meta = match std::fs::metadata(&original) {
            // Already moved: put in place by a move that stopped after its switch, or by
            // Transmission with a torrent it carried.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
                && std::fs::metadata(&copy).is_ok_and(|c| c.is_file() && c.len() == *size) => continue,
            other => other?,
        };
        if meta.len() != *size { return Err(Fail(anyhow::anyhow!("A file changed since the move was planned"))); }
        if let Ok(existing) = std::fs::metadata(&copy) {
            // Put in place just before a restart, or a different file in the way.
            if same_file(&copy, &original) == Some(false) && existing.len() == *size && digest(&copy)? == digest(&original)? { continue; }
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
    if let Ok(mut leaving) = LEAVING.lock() {
        for (name, _) in &unit.files { leaving.insert(unit.from.join(name)); leaving.remove(&unit.to.join(name)); }
    }
    if let Some(hash) = &unit.torrent {
        // Transmission reads the verified copies from now on; it moves nothing itself.
        let sent = s.rpc.call("torrent_set_location", json!({"ids":[hash],"location":unit.to,"move":false})).await;
        s.invalidate_torrents();
        if let Err(error) = sent {
            // A busy Transmission may take the change without answering in time: what it
            // uses decides. Not seen using the copies, the unit waits instead of failing.
            tracing::warn!(hash, %error, "Moving files between drives: Transmission did not confirm the switch");
            if !uses(s, hash, &unit.to).await { return Err(NotSwitched.into()); }
        }
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
    }
    Ok(())
}

/// A switch Transmission did not take; the unit waits and goes again.
#[derive(Debug)]
struct NotSwitched;
impl std::fmt::Display for NotSwitched {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("Waiting for Transmission") }
}
impl std::error::Error for NotSwitched {}

/// Whether Transmission keeps this torrent in `dir`, asking a few times while it is busy.
async fn uses(s: &Service, hash: &str, dir: &Path) -> bool {
    for attempt in 0..3 {
        if attempt > 0 { tokio::time::sleep(std::time::Duration::from_secs(10)).await; }
        let Ok(found) = s.rpc.call("torrent_get", json!({"ids":[hash],"fields":["hash_string","download_dir"]})).await else { continue };
        return found["torrents"].as_array().into_iter().flatten()
            .any(|t| t["download_dir"].as_str().is_some_and(|d| Path::new(d) == dir));
    }
    false
}

fn remove_empty_parents(file: &Path, root: &Path) {
    for dir in file.ancestors().skip(1) {
        if dir == root || !dir.starts_with(root) { break; }
        if std::fs::remove_dir(dir).is_err() { break; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emptying_a_drive_carries_each_torrent_to_the_same_place_on_the_target() {
        let config: crate::config::Config = serde_json::from_value(json!({"listen_host":"127.0.0.1","listen_port":1,"token_file":"t","state_file":"s",
            "transmission_url":"http://127.0.0.1:9091","transmission_auth_file":null,"poll_seconds":30,
            "drives":[{"id":"alpha","media":"/Volumes/Alpha/Media"},{"id":"beta","media":"/Volumes/Beta/Media"}]})).unwrap();
        let drives = Drives::of(&config);
        let [alpha, beta] = drives.all() else { panic!() };
        let to = |dir: &str| destination(&drives, alpha, beta, Path::new(dir));
        // A root itself, with no trailing separator added.
        assert_eq!(to("/Volumes/Alpha/Media/Downloads").unwrap().as_os_str(), "/Volumes/Beta/Media/Downloads");
        assert_eq!(to("/Volumes/Alpha/Media/TV/Show S01").unwrap(), Path::new("/Volumes/Beta/Media/TV/Show S01"));
        // Outside the three roots but on the drive: Junk.
        assert_eq!(to("/Volumes/Alpha/Media/Junk").unwrap().as_os_str(), "/Volumes/Beta/Media/Junk");
        // Already on the target, or on no drive: nothing to carry.
        assert!(to("/Volumes/Beta/Media/Downloads/").is_none());
        assert!(to("/Users/me/Downloads").is_none());
    }

    #[test]
    fn emptying_a_drive_takes_the_files_no_title_or_torrent_takes() {
        let root = std::env::temp_dir().join(format!("torrbaby-strays-{}", uuid::Uuid::new_v4()));
        let tv = root.join("TV");
        std::fs::create_dir_all(tv.join("Homeland/Season 5")).unwrap();
        std::fs::create_dir_all(tv.join("Show")).unwrap();
        std::fs::write(tv.join("Homeland/Season 5/Homeland S05E01.en.srt"), b"1").unwrap();
        std::fs::write(tv.join("Homeland/Season 5/Homeland (2011) - S05E01 - Separation Anxiety.mp4"), b"22").unwrap();
        std::fs::write(tv.join("Homeland/.DS_Store"), b"x").unwrap();
        std::fs::write(tv.join("Homeland/Season 5/E02.mp4.torrbaby-moving"), b"x").unwrap();
        std::fs::write(tv.join("Show/E01.mkv"), b"333").unwrap();
        let recorded = tv.join("Homeland/Season 5/Homeland (2011) - S05E01 - Separation Anxiety.mp4");
        let torrent = tv.join("Show");
        let mut files = stray_files(&tv, &|p| p == recorded || p.starts_with(&torrent)).unwrap();
        files.sort();
        assert_eq!(files, [("Homeland/Season 5/Homeland S05E01.en.srt".to_owned(), 1)]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_file_already_moved_to_the_target_is_not_copied_again() {
        let root = std::env::temp_dir().join(format!("torrbaby-moved-{}", uuid::Uuid::new_v4()));
        let (from, to) = (root.join("A/TV"), root.join("B/TV"));
        std::fs::create_dir_all(from.join("Show")).unwrap();
        std::fs::create_dir_all(to.join("Show")).unwrap();
        std::fs::write(from.join("Show/E02.mkv"), vec![2; 30]).unwrap();
        // Taken there by Transmission with its torrent: gone here, in place there.
        std::fs::write(to.join("Show/E01.mkv"), vec![1; 20]).unwrap();
        let unit = Unit { torrent: None, from: from.clone(), to: to.clone(), files: vec![("Show/E01.mkv".into(), 20), ("Show/E02.mkv".into(), 30)], step: "copying".into(), error: None };
        assert!(copy_unit(&unit).is_ok());
        assert!(staged(&to.join("Show/E02.mkv")).exists());
        assert!(!staged(&to.join("Show/E01.mkv")).exists());
        // Gone with nothing of its size in place: the move cannot go on.
        std::fs::write(to.join("Show/E01.mkv"), vec![1; 5]).unwrap();
        assert!(matches!(copy_unit(&unit), Err(Fail(_))));
        std::fs::remove_dir_all(root).unwrap();
    }
}
