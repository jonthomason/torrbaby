use crate::{
    api::{Service, checked_path},
    config::Config,
    scanner::video,
    storage::now,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .with_context(|| format!("Missing {key}"))
}
fn files(t: &Value) -> Result<&[Value]> {
    t["files"]
        .as_array()
        .map(Vec::as_slice)
        .context("Missing torrent files")
}
pub fn complete(t: &Value) -> bool {
    t["metadata_percent_complete"].as_f64() == Some(1.0)
        && t["percent_done"].as_f64() == Some(1.0)
        && t["left_until_done"].as_u64() == Some(0)
        && t["error"].as_u64() == Some(0)
        && matches!(t["status"].as_u64(), Some(0 | 5 | 6))
        && files(t)
            .map(|f| !f.is_empty() && f.iter().all(|f| f["bytes_completed"] == f["length"]))
            .unwrap_or(false)
}
pub async fn torrents(s: &Service) -> Result<Vec<Value>> {
    Ok(s.rpc
        .call("torrent_get", json!({"fields":crate::transmission::FIELDS}))
        .await?["torrents"]
        .as_array()
        .context("Invalid torrent response")?
        .clone())
}
pub async fn torrent(s: &Service, hash: &str) -> Result<Value> {
    if hash.len() != 40 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("Use a full 40-character torrent hash");
    }
    let mut result = s
        .rpc
        .call(
            "torrent_get",
            json!({"ids":[hash],"fields":crate::transmission::FIELDS}),
        )
        .await?;
    let values = result["torrents"]
        .as_array_mut()
        .context("Invalid torrent response")?;
    if values.len() != 1 {
        bail!("Torrent not found");
    }
    Ok(values.remove(0))
}
fn managed(config: &Config, root: &Path) -> bool {
    [&config.source, &config.movies, &config.tv]
        .iter()
        .any(|p| root == Path::new(p))
}
fn owned(t: &Value) -> Result<HashSet<PathBuf>> {
    files(t)?
        .iter()
        .map(|f| checked_path(Path::new(text(t, "download_dir")?), text(f, "name")?))
        .collect()
}
fn job(s: &Service, hash: &str) -> Result<Value> {
    Ok(s.db
        .lock()
        .map_err(|_| anyhow::anyhow!("Database lock failed"))?
        .state()?["jobs"][hash]
        .clone())
}
fn save_job(s: &Service, hash: &str, value: Value) -> Result<()> {
    s.change_state(|state| {
        if !state["jobs"].is_object() {
            state["jobs"] = json!({});
        }
        state["jobs"][hash] = value;
    })
}

pub async fn action(s: &Service, hash: &str, action: &str, body: &Value) -> Result<Value> {
    let _guard = s.mutation_lock.lock().await;
    let normalized = hash.to_ascii_lowercase();
    let hash = normalized.as_str();
    if s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?.hidden_package(hash)? {
        bail!("This package has a library removal operation; its progress is shown in Seeding");
    }
    let t = torrent(s, hash).await?;
    let method = match action {
        "start" => Some("torrent_start"),
        "stop" => Some("torrent_stop"),
        "verify" => Some("torrent_verify"),
        "reannounce" => Some("torrent_reannounce"),
        _ => None,
    };
    if let Some(method) = method {
        let result = s.rpc.call(method, json!({"ids":[hash]})).await?;
        s.event(&format!("torrent_{action}"), json!({"hash":hash}))?;
        s.invalidate_torrents();
        return Ok(result);
    }
    if action == "rename" {
        return rename(s, hash, text(body, "path")?, text(body, "name")?).await;
    }
    if action == "remove" {
        let delete = match body.get("delete_data") {
            None => false,
            Some(v) => v.as_bool().context("delete_data must be boolean")?,
        };
        if delete
            && body["confirm_hash"]
                .as_str()
                .map(str::to_ascii_lowercase)
                .as_deref()
                != Some(hash)
        {
            bail!("Deleting data requires matching confirm_hash");
        }
        if delete {
            let config = s.effective_config()?;
            if !managed(&config, Path::new(text(&t, "download_dir")?)) {
                bail!("Data deletion is restricted to managed roots");
            }
            let own = owned(&t)?;
            for other in torrents(s).await? {
                if other["hash_string"] != hash && !own.is_disjoint(&owned(&other)?) {
                    bail!("Cannot delete shared torrent files");
                }
            }
        }
        s.event(
            "torrent_remove_requested",
            json!({"hash":hash,"delete_data":delete}),
        )?;
        let response = s.rpc.call("torrent_remove", json!({"ids":[hash],"delete_local_data":delete})).await;
        // Transmission may finish removal after the HTTP response times out.
        // Reconcile by identity without issuing another destructive request.
        s.invalidate_torrents();
        let result = match response {
            Ok(result) => result,
            Err(error) => {
                let state = s.rpc.call("torrent_get", json!({"ids":[hash],"fields":["hash_string"]})).await;
                match state {
                    Ok(state) if state["torrents"].as_array().is_some_and(|items|items.is_empty()) => json!({"reconciled":true}),
                    _ => return Err(error.context("Could not confirm torrent removal; refresh the torrent list before retrying")),
                }
            }
        };
        save_job(
            s,
            hash,
            json!({"hash":hash,"name":t["name"],"phase":"ignored"}),
        )?;
        s.event("torrent_removed", json!({"hash":hash,"delete_data":delete}))?;
        s.invalidate_torrents();
        return Ok(result);
    }
    if action == "retry" || action == "ignore" {
        let mut previous = job(s, hash)?;
        if action == "ignore" {
            if previous["phase"] == "moving" {
                bail!("Cannot cancel an in-flight Transmission move");
            }
            previous = json!({"hash":hash,"name":t["name"],"phase":"ignored"});
        } else if previous["phase"] == "moving" {
            previous["attempted"] = json!(0);
            previous["error"] = Value::Null;
        } else {
            previous = Value::Null;
        }
        save_job(s, hash, previous)?;
        s.event(&format!("job_{action}"), json!({"hash":hash}))?;
        return Ok(json!({"accepted":true}));
    }
    bail!("Unknown torrent action")
}

pub async fn add(s: &Service, body: &Value) -> Result<Value> {
    add_at(s, body, None).await
}
pub(crate) async fn add_at(s: &Service, body: &Value, directory:Option<&Path>) -> Result<Value> {
    use base64::Engine;
    if body
        .as_object()
        .context("Expected object")?
        .keys()
        .any(|k| !["magnet", "metainfo", "torrent_url", "paused", "listing_key"].contains(&k.as_str()))
    {
        bail!("Allowed fields: magnet, metainfo, torrent_url, paused");
    }
    let magnet = body["magnet"].as_str().filter(|v| !v.is_empty());
    let metainfo = body["metainfo"].as_str().filter(|v| !v.is_empty());
    let torrent_url = body["torrent_url"].as_str().filter(|v| !v.is_empty());
    if [magnet.is_some(), metainfo.is_some(), torrent_url.is_some()].into_iter().filter(|v| *v).count() != 1 {
        bail!("Provide one torrent source");
    }
    let config = s.effective_config()?;
    let paused = match body.get("paused") {
        None => false,
        Some(v) => v.as_bool().context("paused must be boolean")?,
    };
    let mut params = json!({"download_dir":directory.map(|p|p.to_string_lossy().into_owned()).unwrap_or(config.source),"paused":paused});
    if let Some(magnet) = magnet {
        if !magnet.starts_with("magnet:?") {
            bail!("Expected a magnet link");
        }
        params["filename"] = json!(magnet);
    } else if let Some(raw) = torrent_url {
        let url=reqwest::Url::parse(raw)?;
        let attachment=url.path().strip_prefix("/attach-download-").and_then(|s|s.strip_suffix(".htm"));
        anyhow::ensure!(url.scheme()=="http" && url.host_str()==Some("www.1lou.me") && url.port().is_none()
            && url.username().is_empty() && url.password().is_none() && url.query().is_none() && url.fragment().is_none()
            && attachment.is_some_and(|id| !id.is_empty() && id.len()<=12 && id.bytes().all(|b|b.is_ascii_digit())), "Invalid torrent attachment URL");
        params["filename"] = json!(raw);
    } else if let Some(metainfo) = metainfo {
        base64::engine::general_purpose::STANDARD.decode(metainfo)?;
        params["metainfo"] = json!(metainfo);
    }
    let _guard = s.mutation_lock.lock().await;
    let added = s.rpc.add(params).await;
    s.invalidate_torrents(); // An unconfirmed add may still finish in Transmission.
    let result = added?;
    if directory.is_none() { if let Some(hash) = result["torrent_added"]["hash_string"].as_str()
        .or_else(||result["torrent_duplicate"]["hash_string"].as_str()) {
        s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?
            .restore_completed_package(&hash.to_ascii_lowercase())?;
    }
    }
    if let Some(key)=body["listing_key"].as_str().filter(|s|!s.is_empty() && s.len()<2048) {
        if let Some(hash)=result["torrent_added"]["hash_string"].as_str().or(result["torrent_duplicate"]["hash_string"].as_str()) {
            s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?.connection.execute("INSERT OR REPLACE INTO search_torrent_hashes VALUES(?,?)",rusqlite::params![key,hash.to_ascii_lowercase()])?;
        }
    }
    s.event("torrent_added", json!({"result":result}))?;
    s.invalidate_torrents();
    Ok(result)
}

/// Caller holds mutation_lock, including for automated renames.
pub async fn rename(s: &Service, hash: &str, path: &str, name: &str) -> Result<Value> {
    let t = torrent(s, hash).await?;
    if !complete(&t) {
        bail!("Rename requires a complete torrent");
    }
    let config = s.effective_config()?;
    let root = Path::new(text(&t, "download_dir")?);
    if !managed(&config, root) {
        bail!("Torrent is outside managed roots");
    }
    if job(s, hash)?["phase"] == "moving" {
        bail!("Wait for the current move to finish");
    }
    if name.is_empty()
        || [".", ".."].contains(&name)
        || name.contains(['/', '\\', '\0'])
        || name.len() > 255
    {
        bail!("New name must be a single filesystem component");
    }
    let old = checked_path(root, path)?;
    if !files(&t)?.iter().any(|f| {
        f["name"]
            .as_str()
            .map(|p| p == path || p.starts_with(&format!("{path}/")))
            .unwrap_or(false)
    }) {
        bail!("Path is not owned by this torrent");
    }
    if !old.exists() {
        bail!("Path does not exist");
    }
    if old.is_file()
        && old.extension().map(|x| x.to_ascii_lowercase())
            != Path::new(name).extension().map(|x| x.to_ascii_lowercase())
    {
        bail!("Keep the file extension");
    }
    let relative = Path::new(path).parent().unwrap_or(Path::new("")).join(name);
    let relative = relative.to_str().context("Invalid rename path")?;
    let new = checked_path(root, relative)?;
    if new.symlink_metadata().is_ok() {
        bail!("Rename destination already exists");
    }
    for other in torrents(s).await? {
        if other["hash_string"] != hash
            && owned(&other)?
                .iter()
                .any(|p| p == &old || p.starts_with(&old))
        {
            bail!("Path is shared by another torrent");
        }
    }
    s.event(
        "rename_requested",
        json!({"hash":hash,"path":path,"name":name}),
    )?;
    s.rpc
        .call(
            "torrent_rename_path",
            json!({"ids":[hash],"path":path,"name":name}),
        )
        .await?;
    s.invalidate_torrents();
    let refreshed = torrent(s, hash).await?;
    if !new.exists()
        || old.exists()
        || !files(&refreshed)?.iter().any(|f| {
            f["name"]
                .as_str()
                .map(|p| p == relative || p.starts_with(&format!("{relative}/")))
                .unwrap_or(false)
        })
    {
        bail!("Rename requested but verification failed; inspect before retrying");
    }
    s.event(
        "rename_verified",
        json!({"hash":hash,"path":path,"name":name}),
    )?;
    Ok(refreshed)
}

fn plan(config: &Config, t: &Value) -> Result<Option<Value>> {
    if t["download_dir"] != config.source || !complete(t) {
        return Ok(None);
    }
    if !files(t)?.iter().any(|f| {
        f["name"]
            .as_str()
            .map(|p| video(Path::new(p)))
            .unwrap_or(false)
    }) {
        return Ok(None);
    }
    let is_tv = crate::metadata::parse_name(t["name"].as_str().unwrap_or("")).2 == "tv"
        || files(t)?
            .iter()
            .any(|f| crate::metadata::parse_name(f["name"].as_str().unwrap_or("")).2 == "tv");
    let destination = if is_tv { &config.tv } else { &config.movies };
    let source = Path::new(&config.source);
    let dest = Path::new(destination);
    if !source.is_dir() || !dest.is_dir() {
        bail!("Library volume/directory unavailable");
    }
    for file in files(t)? {
        let path = text(file, "name")?;
        let original = checked_path(source, path)?;
        checked_path(dest, path)?;
        if !original.is_file() || Some(original.metadata()?.len()) != file["length"].as_u64() {
            bail!("Source file missing or size differs: {path}");
        }
        let top = Path::new(path)
            .components()
            .next()
            .context("Empty path")?
            .as_os_str()
            .to_str()
            .context("Invalid path")?;
        if checked_path(dest, top)?.symlink_metadata().is_ok() {
            bail!("Destination already exists: {top}");
        }
    }
    Ok(Some(
        json!({"hash":t["hash_string"],"name":t["name"],"category":if is_tv{"tv"}else{"movies"},"source":config.source,"destination":destination,"files":t["files"],"phase":"moving","created":now(),"attempted":0}),
    ))
}

pub async fn tick(s: &Service) -> Result<()> {
    let _guard = s.mutation_lock.lock().await;
    let config = s.effective_config()?;
    if config.paused {
        return Ok(());
    }
    let all = torrents(s).await?;
    s.change_state(|state| {
        state["last_poll"] = json!(now());
        state["last_error"] = Value::Null;
    })?;
    for t in &all {
        if crate::trials::trial_path(t["download_dir"].as_str().unwrap_or("")) {continue;}
        let hash = text(t, "hash_string")?;
        if s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?.hidden_package(hash)? { continue; }
        let previous = job(s, hash)?;
        if previous["phase"] == "ignored" {
            continue;
        }
        let result = if previous["phase"] == "done" {
            Ok(())
        } else {
            advance(s, &config, t, &all, previous.clone()).await
        };
        let result = match result {
            Ok(()) => auto_name(s, &config, hash).await,
            Err(error) => Err(error),
        };
        if result.is_ok() {
            let mut current = job(s, hash)?;
            if current.is_object() && !current["error"].is_null() {
                current["error"] = Value::Null;
                save_job(s, hash, current)?;
            }
        }
        if let Err(error) = result {
            let mut current = job(s, hash)?;
            if current.is_null() {
                current = json!({"hash":hash,"name":t["name"],"phase":"blocked"});
            }
            if current["error"] != error.to_string() {
                current["error"] = json!(error.to_string());
                save_job(s, hash, current)?;
                s.event("job_error", json!({"hash":hash,"error":error.to_string()}))?;
            }
        }
    }
    Ok(())
}

async fn advance(
    s: &Service,
    config: &Config,
    t: &Value,
    all: &[Value],
    mut job: Value,
) -> Result<()> {
    let hash = text(t, "hash_string")?;
    if job.is_null() || job["phase"] == "blocked" {
        if t["download_dir"] == config.source && complete(t) {
            let ready={
            let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
            let assets=db.documents("assets")?;
            files(t)?.iter().enumerate().filter(|(_,f)|f["name"].as_str().is_some_and(|n|video(Path::new(n))))
                .all(|(index,_)|assets.iter().any(|a|a["torrent_hash"]==hash && a["file_index"]==index
                    && (a["role"]=="extra" || (a["status"]=="matched" && a["entity_id"].is_string()))))
            };
            if !ready { bail!("Waiting for media identification before moving to the library"); }
            auto_name(s,config,hash).await?;
        }
        // Renames use Transmission and can change paths; plan from its fresh file list.
        let current=torrent(s,hash).await?;
        let Some(new) = plan(config, &current)? else {
            return Ok(());
        };
        let own = owned(t)?;
        for other in all {
            if other["hash_string"] != hash && !own.is_disjoint(&owned(other)?) {
                bail!("Source files are shared by another torrent");
            }
        }
        job = new;
        save_job(s, hash, job.clone())?;
    }
    let source = PathBuf::from(text(&job, "source")?);
    let destination = PathBuf::from(text(&job, "destination")?);
    let payload = job["files"].as_array().context("Missing job files")?;
    let mut moved = t["download_dir"] == job["destination"] && complete(t);
    if moved {
        for file in payload {
            let name = text(file, "name")?;
            let target = checked_path(&destination, name)?;
            if !target.is_file()
                || Some(target.metadata()?.len()) != file["length"].as_u64()
                || checked_path(&source, name)?.exists()
            {
                moved = false;
                break;
            }
        }
    }
    if moved {
        job["phase"] = json!("done");
        job["finished"] = json!(now());
        job["error"] = Value::Null;
        save_job(s, hash, job)?;
        s.event(
            "move_verified",
            json!({"hash":hash,"destination":destination}),
        )?;
        return Ok(());
    }
    if now() - job["attempted"].as_f64().unwrap_or(0.0) < 300.0 {
        return Ok(());
    }
    if t["download_dir"] != job["source"] {
        bail!("Move awaiting verification; torrent location changed");
    }
    if !complete(t) {
        bail!("Torrent no longer complete; move deferred");
    }
    for file in payload {
        let name = text(file, "name")?;
        let original = checked_path(&source, name)?;
        if !original.is_file() || Some(original.metadata()?.len()) != file["length"].as_u64() {
            bail!("Move awaiting verification; source changed");
        }
        let top = Path::new(name)
            .components()
            .next()
            .context("Empty file path")?
            .as_os_str()
            .to_str()
            .context("Invalid file path")?;
        if checked_path(&destination, top)?.symlink_metadata().is_ok() {
            bail!("Move awaiting verification; destination exists");
        }
    }
    job["attempted"] = json!(now());
    save_job(s, hash, job)?;
    s.event(
        "move_requested",
        json!({"hash":hash,"destination":destination}),
    )?;
    s.rpc
        .call(
            "torrent_set_location",
            json!({"ids":[hash],"location":destination,"move":true}),
        )
        .await?;
    s.invalidate_torrents();
    Ok(())
}

fn clean_name(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches([' ', '.'])
        .to_owned()
}

/// Plan against stable Transmission file indices. Persist before any rename so a
/// restart can finish subtitle renames even after the video has its new name.
async fn auto_name(s: &Service, config: &Config, hash: &str) -> Result<()> {
    let t = torrent(s, hash).await?;
    if !complete(&t)
        || !managed(config, Path::new(text(&t, "download_dir")?))
    {
        return Ok(());
    }
    let (assets, entities, mut pending) = {
        let db =
            s.db.lock()
                .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
        (
            db.documents("assets")?,
            db.documents("entities")?,
            db.state()?["renames"][hash].clone(),
        )
    };
    if !pending.is_object() {
        let mut operations = Vec::new();
        for asset in assets.iter().filter(|a| {
            a["torrent_hash"] == hash
                && a["available"] == true
                && a["role"] != "extra"
                && a["status"] == "matched"
        }) {
            let Some(entity) = entities.iter().find(|e| e["id"] == asset["entity_id"]) else {
                continue;
            };
            let confidence = asset["match_confidence"]
                .as_str()
                .or_else(|| entity["match_confidence"].as_str())
                .unwrap_or("");
            if asset["confirmed"] != true
                && !["exact_title_year", "exact_unique_title"].contains(&confidence)
            {
                continue;
            }
            let Some(index) = asset["file_index"].as_u64() else {
                continue;
            };
            let Some(file) = files(&t)?.get(index as usize) else {
                continue;
            };
            let old = text(file, "name")?;
            let path = Path::new(old);
            if !video(path) {
                continue;
            }
            let title = clean_name(entity["title"].as_str().unwrap_or(""));
            if title.is_empty() {
                continue;
            }
            let year = entity["year"].as_str().unwrap_or("");
            let title = if year.len() == 4 && year.bytes().all(|c| c.is_ascii_digit()) {
                format!("{title} ({year})")
            } else {
                title
            };
            let stem = if entity["kind"] == "tv" {
                let (Some(season), Some(episode)) =
                    (asset["season"].as_u64(), asset["episode"].as_u64())
                else {
                    continue;
                };
                // Multi-episode files need an explicit mapping, not a guessed first episode.
                let filename = path.file_stem().unwrap_or_default().to_string_lossy();
                let markers = regex::Regex::new(
                    r"(?i)(?:s\d{1,3}[. _-]*e\d{1,3}|\d{1,3}x\d{1,3})(?:[. _-]*(?:e\d|[-+]\d))",
                )?;
                if markers.is_match(&filename)
                    || crate::scanner::EPISODE.find_iter(&filename).count() != 1
                {
                    continue;
                }
                let Some(ep) = entity["episodes"].as_array().and_then(|v| {
                    v.iter()
                        .find(|e| e["season"] == season && e["episode"] == episode)
                }) else {
                    continue;
                };
                let episode_title = clean_name(ep["title"].as_str().unwrap_or(""));
                if episode_title.is_empty() {
                    format!("{title} - S{season:02}E{episode:02}")
                } else {
                    format!("{title} - S{season:02}E{episode:02} - {episode_title}")
                }
            } else if entity["kind"] == "movie" {
                title
            } else {
                continue;
            };
            let extension = path
                .extension()
                .and_then(|e| e.to_str())
                .context("Missing file extension")?;
            let name = format!("{stem}.{extension}");
            if name.len() > 240 || path.file_name().and_then(|p| p.to_str()) == Some(&name) {
                continue;
            }
            operations.push(json!({"index":index,"path":old,"name":name}));
            let old_stem = path
                .file_stem()
                .and_then(|p| p.to_str())
                .context("Invalid filename")?;
            for (side_index, sidecar) in files(&t)?.iter().enumerate() {
                let side = Path::new(text(sidecar, "name")?);
                if side.parent() != path.parent() {
                    continue;
                }
                let ext = side
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if !["srt", "ass", "ssa", "vtt", "sub", "idx"].contains(&ext.as_str()) {
                    continue;
                }
                let Some(suffix) = side
                    .file_name()
                    .and_then(|p| p.to_str())
                    .and_then(|p| p.strip_prefix(old_stem))
                    .filter(|p| p.starts_with('.'))
                else {
                    continue;
                };
                let name = format!("{stem}{suffix}");
                if name.len() <= 240 {
                    operations
                        .push(json!({"index":side_index,"path":text(sidecar,"name")?,"name":name}));
                }
            }
        }
        if operations.is_empty() {
            return Ok(());
        }
        // Reject conflicting targets or multiple interpretations before any mutation.
        let mut targets = HashSet::new();
        let mut indices = HashSet::new();
        for op in &operations {
            let target = Path::new(text(op, "path")?)
                .parent()
                .unwrap_or(Path::new(""))
                .join(text(op, "name")?);
            if !targets.insert(target.clone()) || !indices.insert(op["index"].as_u64()) {
                bail!("Ambiguous canonical names; review required");
            }
            let full = checked_path(
                Path::new(text(&t, "download_dir")?),
                target.to_str().context("Invalid name")?,
            )?;
            if full.symlink_metadata().is_ok() {
                bail!("Canonical name already exists; review required");
            }
        }
        pending = json!({"operations":operations,"created":now()});
        s.change_state(|state| {
            if !state["renames"].is_object() {
                state["renames"] = json!({});
            }
            state["renames"][hash] = pending.clone();
        })?;
    }
    for op in pending["operations"]
        .as_array()
        .context("Invalid rename plan")?
    {
        let current = torrent(s, hash).await?;
        let index = op["index"].as_u64().context("Invalid planned file index")?;
        let file = files(&current)?
            .get(index as usize)
            .context("Planned file no longer exists")?;
        let old = text(op, "path")?;
        let name = text(op, "name")?;
        let target = Path::new(old).parent().unwrap_or(Path::new("")).join(name);
        if file["name"].as_str() == target.to_str() {
            let full = checked_path(
                Path::new(text(&current, "download_dir")?),
                target.to_str().context("Invalid path")?,
            )?;
            if !full.is_file() || Some(full.metadata()?.len()) != file["length"].as_u64() {
                bail!("Renamed file verification failed");
            }
            continue;
        }
        if file["name"] != old {
            bail!("Torrent path changed since rename planning; review required");
        }
        rename(s, hash, old, name).await?;
    }
    s.change_state(|state| {
        state["renames"].as_object_mut().map(|r| r.remove(hash));
    })?;
    s.event("automatic_rename_complete", json!({"hash":hash}))?;
    Ok(())
}
