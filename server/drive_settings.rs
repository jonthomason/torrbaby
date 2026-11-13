//! Settings → Server → Drives: the drives Torrbaby keeps media on, for administrators.
//! A drive is used only once it is added here (docs/disk-aggregation.md, decisions); a
//! volume that is merely mounted is offered, never adopted.
//!
//! GET    /v1/drives              every drive, with its state, space and how much is on it
//! GET    /v1/drives/candidates   mounted volumes that could be added
//! POST   /v1/drives              {"mount": "/Volumes/Beta"} adds that volume's Media folder
//! PATCH  /v1/drives/{id}         {"name", "accept_new", "reserve_bytes"}
//! DELETE /v1/drives/{id}         removes an empty drive from the list (its files stay)
use crate::{api::Service, config::DriveConfig, drives::{self, Drive, Drives, Status}};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub async fn route(s: &Service, method: &str, path: &str, body: &Value) -> Option<Result<Value>> {
    let rest = path.strip_prefix("/v1/drives")?;
    Some(match (method, rest) {
        ("GET", "") => list(s),
        ("GET", "/candidates") => candidates(s),
        ("POST", "") => { let _guard = s.mutation_lock.lock().await; add(s, body) }
        ("PATCH", id) if id.starts_with('/') => { let _guard = s.mutation_lock.lock().await; change(s, &id[1..], body) }
        ("DELETE", id) if id.starts_with('/') => { let _guard = s.mutation_lock.lock().await; remove(s, &id[1..]).await }
        _ => return None,
    })
}

fn status_name(status: &Status) -> &'static str {
    match status { Status::Online => "online", Status::Missing => "missing", Status::Foreign(_) => "foreign", Status::Elsewhere(_) => "elsewhere" }
}

/// How many library files, and bytes, each drive holds.
fn holdings(drives: &Drives, assets: &[Value]) -> std::collections::HashMap<String, (u64, u64)> {
    let mut held = std::collections::HashMap::new();
    for asset in assets {
        let Some(drive) = asset["path"].as_str().and_then(|p| drives.drive_of(Path::new(p))) else { continue };
        if asset["available"] != true && asset["offline"] != true { continue; }
        let entry = held.entry(drive.id.clone()).or_insert((0, 0));
        entry.0 += 1;
        entry.1 += asset["size"].as_u64().unwrap_or(0);
    }
    held
}

fn list(s: &Service) -> Result<Value> {
    let drives = Drives::of(&s.effective_config()?);
    let assets = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?.documents("assets")?;
    let held = holdings(&drives, &assets);
    let rows: Vec<Value> = drives.all().iter().map(|d| {
        let status = d.status();
        let (total, free) = if status == Status::Online { drives::space(d) } else { (0, 0) };
        let (files, bytes) = held.get(&d.id).copied().unwrap_or((0, 0));
        json!({"id":d.id,"name":d.name,"media":d.media,"status":status_name(&status),"total_bytes":total,"free_bytes":free,
            "accept_new":d.accept_new,"reserve_bytes":d.reserve_bytes,"files":files,"bytes":bytes,
            "full":status == Status::Online && free < d.reserve_bytes})
    }).collect();
    Ok(json!({"drives":rows}))
}

/// Volumes mounted under /Volumes that are not already drives, with room and writable.
fn candidates(s: &Service) -> Result<Value> {
    let drives = Drives::of(&s.effective_config()?);
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/Volumes")?.flatten() {
        let mount = entry.path();
        // "Macintosh HD" is a link to the startup disk.
        if entry.file_type().map(|t| t.is_symlink()).unwrap_or(true) || !drives::is_mount(&mount) { continue; }
        if drives.all().iter().any(|d| d.media.starts_with(&mount)) { continue; }
        let Some((total, free)) = drives::capacity_of(&mount) else { continue };
        // Recovery, Preboot and other parts of the startup disk share its space.
        if drives::capacity_of(Path::new("/")).is_some_and(|startup| startup.0 == total) { continue; }
        let writable = std::fs::metadata(&mount).is_ok_and(|m| !m.permissions().readonly());
        let name = mount.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let known = drives::read_marker(&mount.join("Media"));
        let (format, case_sensitive) = drives::format_of(&mount).unwrap_or_default();
        found.push(json!({"mount":mount,"name":name,"total_bytes":total,"free_bytes":free,"writable":writable,
            "format":format,"case_sensitive":case_sensitive,"usable":drives::usable_format(&mount),
            "has_media":mount.join("Media").is_dir(),"used_before":known.is_some()}));
    }
    Ok(json!({"candidates":found}))
}

/// The current drive list as it will be saved.
fn configs(drives: &Drives) -> Vec<DriveConfig> { drives.all().iter().map(Drive::to_config).collect() }

/// Refuse anything the server would not start with.
fn validate(s: &Service, list: &[DriveConfig]) -> Result<()> {
    let state = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?.state()?;
    let mut settings = state["settings"].clone();
    if !settings.is_object() { settings = json!({}); }
    settings["drives"] = serde_json::to_value(list)?;
    s.config.with_settings(&settings)?;
    Ok(())
}

fn save(s: &Service, list: Vec<DriveConfig>) -> Result<()> {
    validate(s, &list)?;
    let value = serde_json::to_value(&list)?;
    s.change_state(|state| {
        if !state["settings"].is_object() { state["settings"] = json!({}); }
        state["settings"]["drives"] = value.clone();
    })?;
    s.event("drives_changed", json!({"drives":value}))
}

fn add(s: &Service, body: &Value) -> Result<Value> {
    let mount = PathBuf::from(body["mount"].as_str().context("Choose a drive")?);
    let link = std::fs::symlink_metadata(&mount).map(|m| m.file_type().is_symlink()).unwrap_or(true);
    ensure!(mount.parent() == Some(Path::new("/Volumes")) && !link && drives::is_mount(&mount), "Choose a connected drive");
    // A part of the startup disk (Recovery, Preboot…) shares its space; it is not a drive.
    ensure!(!drives::capacity_of(&mount).zip(drives::capacity_of(Path::new("/"))).is_some_and(|(this, startup)| this.0 == startup.0),
        "Choose a connected drive");
    ensure!(drives::usable_format(&mount), "Erase this drive as APFS or Mac OS Extended (Journaled), not case-sensitive, in Disk Utility, then add it");
    let drives = Drives::of(&s.effective_config()?);
    ensure!(!drives.all().iter().any(|d| d.media.starts_with(&mount) || mount.starts_with(&d.media)), "This drive is already in use");
    let media = mount.join("Media");
    // A drive used before keeps its identity, so its files are recognised again.
    let id = match drives::read_marker(&media) {
        // A marker naming a drive already in the list came with files copied from that
        // drive: this is a new drive, and gets its own identity.
        Some(id) if !drives.all().iter().any(|d| d.id == id) => id,
        Some(id) => {
            ensure!(drives.all().iter().any(|d| d.id == id && d.online()), "This drive is already in use");
            new_id(mount.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default().as_str(), &drives)
        }
        None => new_id(mount.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default().as_str(), &drives),
    };
    let mut list = configs(&drives);
    list.push(DriveConfig { id: id.clone(), name: None, media: Some(media.to_string_lossy().into_owned()), downloads: None, movies: None, tv: None,
        accept_new: body["accept_new"].as_bool().unwrap_or(true), reserve_bytes: None });
    // Checked before anything is written to the drive.
    validate(s, &list)?;
    for folder in ["Downloads", "Movies", "TV"] { drives::create_media_folder(&media.join(folder)).context("Could not create folders on this drive")?; }
    drives::write_marker(&media, &id).context("Could not write to this drive")?;
    save(s, list)?;
    list_one(s, &id)
}

/// A short id from the volume's name, unique among the drives.
fn new_id(name: &str, drives: &Drives) -> String {
    let base: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).take(24).collect::<String>().to_ascii_lowercase();
    let base = if base.is_empty() { "drive".to_owned() } else { base };
    let mut id = base.clone();
    let mut n = 2;
    while drives.all().iter().any(|d| d.id == id) { id = format!("{base}-{n}"); n += 1; }
    id
}

fn list_one(s: &Service, id: &str) -> Result<Value> {
    list(s)?["drives"].as_array().into_iter().flatten().find(|d| d["id"] == id).cloned().context("Unknown drive")
}

fn change(s: &Service, id: &str, body: &Value) -> Result<Value> {
    let object = body.as_object().context("Expected drive settings")?;
    ensure!(!object.is_empty() && object.keys().all(|k| ["name", "accept_new", "reserve_bytes"].contains(&k.as_str())),
        "Allowed drive settings: name, accept_new, reserve_bytes");
    let drives = Drives::of(&s.effective_config()?);
    let mut list = configs(&drives);
    let drive = list.iter_mut().find(|d| d.id == id).context("Unknown drive")?;
    if let Some(name) = object.get("name") {
        let name = name.as_str().context("Name must be text")?.trim();
        ensure!(name.chars().count() <= 64, "Use a shorter name");
        drive.name = (!name.is_empty()).then(|| name.to_owned());
    }
    if let Some(accept) = object.get("accept_new") { drive.accept_new = accept.as_bool().context("accept_new must be true or false")?; }
    if let Some(reserve) = object.get("reserve_bytes") {
        let reserve = reserve.as_u64().context("reserve_bytes must be a number of bytes")?;
        ensure!(reserve <= 10_000_000_000_000, "Reserve is too large");
        drive.reserve_bytes = (reserve != drives::DEFAULT_RESERVE_BYTES).then_some(reserve);
    }
    save(s, list)?;
    list_one(s, id)
}

async fn remove(s: &Service, id: &str) -> Result<Value> {
    let drives = Drives::of(&s.effective_config()?);
    let drive = drives.all().iter().find(|d| d.id == id).context("Unknown drive")?;
    ensure!(drives.all().len() > 1, "The library needs at least one drive");
    let assets = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?.documents("assets")?;
    if holdings(&drives, &assets).get(id).is_some_and(|(files, _)| *files > 0) {
        bail!("Move this drive's titles to another drive first");
    }
    ensure!(!crate::drive_moves::moving_to(s, id) && !crate::drive_moves::moving_under(s, &drive.media), "Wait until moves to this drive have finished");
    let torrents = crate::organizer::torrents(s).await?;
    let on_drive = |dir: &str| drives.drive_of(Path::new(dir)).is_some_and(|d| d.id == id) || Path::new(dir).starts_with(&drive.media);
    ensure!(!torrents.iter().any(|t| t["download_dir"].as_str().is_some_and(on_drive)), "Torrents are still using this drive");
    let list = configs(&drives).into_iter().filter(|d| d.id != id).collect();
    save(s, list)?;
    Ok(json!({"removed":id}))
}
