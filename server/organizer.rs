use crate::{
    api::{Service, checked_path},
    config::Config,
    drives::Drives,
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
    Drives::of(config).is_root(root)
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
        if action == "stop" {
            // Stopped by hand: no longer one Torrbaby stopped for its drive and will start again.
            let _ = s.change_state(|state| for key in ["held_for_drive", "running_torrents"] {
                if let Some(list) = state[key].as_array_mut() { list.retain(|h| !h.as_str().is_some_and(|h| h.eq_ignore_ascii_case(hash))); }
            });
        }
        s.event(&format!("torrent_{action}"), json!({"hash":hash}))?;
        s.invalidate_torrents();
        return Ok(result);
    }
    if action == "unprioritize" {
        // Back to an ordinary download; its place in the queue is left alone.
        let result = s.rpc.call("torrent_set", json!({"ids":[hash],"bandwidth_priority":0})).await?;
        s.event("torrent_unprioritize", json!({"hash":hash}))?;
        s.invalidate_torrents();
        return Ok(result);
    }
    if action == "prioritize" {
        // Download this one first: to the head of the queue, started at once whatever
        // the queue limit says, and favoured when bandwidth is shared.
        s.rpc.call("torrent_set", json!({"ids":[hash],"bandwidth_priority":1})).await?;
        s.rpc.call("queue_move_top", json!({"ids":[hash]})).await?;
        let result = s.rpc.call("torrent_start_now", json!({"ids":[hash]})).await?;
        s.event("torrent_prioritize", json!({"hash":hash}))?;
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
    // A cloud-drive result is not a torrent: its videos are queued as a cloud download.
    if let Some((url, title, copy)) = magnet.and_then(crate::torrent_search::cloud_listing) {
        let queued = s.cloud.add(s, &json!({"url": url, "title": title, "copy": copy})).await?;
        return Ok(json!({"result": "success", "cloud_job": queued["job_id"]}));
    }
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
    let mut params = json!({"paused":paused});
    // The release's name and size, when known before Transmission has its metadata.
    let mut release: (Option<String>, u64) = (None, 0);
    if let Some(magnet) = magnet {
        if !magnet.starts_with("magnet:?") {
            bail!("Expected a magnet link");
        }
        if let Some(listing) = crate::torrent_search::private_listing(magnet) {
            // A private site's result: its .torrent is fetched here, with the member's passkey.
            let bytes = s.torrent_search.private_metainfo(&listing).await?;
            if let Some((name, size)) = metainfo_summary(&bytes) { release = (Some(name), size); }
            params["metainfo"] = json!(base64::engine::general_purpose::STANDARD.encode(bytes));
            // Marked so it is seeded by the site's rules, not the general target.
            params["labels"] = json!([crate::torrent_search::PRIVATE_LABEL]);
        } else {
            if let Ok(url) = reqwest::Url::parse(magnet) {
                for (key, value) in url.query_pairs() {
                    if key == "dn" { release.0 = Some(value.to_string()); }
                    if key == "xl" { release.1 = value.parse().unwrap_or(0); }
                }
            }
            params["filename"] = json!(magnet);
        }
    } else if let Some(raw) = torrent_url {
        let url=reqwest::Url::parse(raw)?;
        let attachment=url.path().strip_prefix("/attach-download-").and_then(|s|s.strip_suffix(".htm"));
        anyhow::ensure!(url.scheme()=="http" && url.host_str()==Some("www.1lou.me") && url.port().is_none()
            && url.username().is_empty() && url.password().is_none() && url.query().is_none() && url.fragment().is_none()
            && attachment.is_some_and(|id| !id.is_empty() && id.len()<=12 && id.bytes().all(|b|b.is_ascii_digit())), "Invalid torrent attachment URL");
        params["filename"] = json!(raw);
    } else if let Some(metainfo) = metainfo {
        let bytes = base64::engine::general_purpose::STANDARD.decode(metainfo)?;
        if let Some((name, size)) = metainfo_summary(&bytes) { release = (Some(name), size); }
        params["metainfo"] = json!(metainfo);
    }
    let download_dir = match directory {
        Some(directory) => directory.to_path_buf(),
        None => {
            let (entities, assets) = {
                let db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
                (db.documents("entities")?, db.documents("assets")?)
            };
            let drives = Drives::of(&config);
            let drive = drives.place_named(&entities, &assets, release.0.as_deref(), release.1)?;
            crate::drives::commit_more("torrents", &drive.id, release.1);
            drive.downloads.clone()
        }
    };
    params["download_dir"] = json!(download_dir.to_string_lossy());
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

/// The name and total size in a .torrent file.
fn metainfo_summary(bytes: &[u8]) -> Option<(String, u64)> {
    // A minimal bencode reader: (value end, and for the info dictionary its name and lengths).
    fn skip(b: &[u8], at: usize, depth: usize) -> Option<usize> {
        if depth > 64 { return None; }
        match *b.get(at)? {
            b'i' => Some(at + b[at..].iter().position(|c| *c == b'e')? + 1),
            b'l' | b'd' => { let mut at = at + 1; while *b.get(at)? != b'e' { at = skip(b, at, depth + 1)?; } Some(at + 1) }
            b'0'..=b'9' => { let colon = at + b[at..].iter().position(|c| *c == b':')?; let len: usize = std::str::from_utf8(&b[at..colon]).ok()?.parse().ok()?; colon.checked_add(1 + len).filter(|end| *end <= b.len()) }
            _ => None,
        }
    }
    fn text(b: &[u8], at: usize) -> Option<&[u8]> {
        let colon = at + b[at..].iter().position(|c| *c == b':')?;
        let len: usize = std::str::from_utf8(&b[at..colon]).ok()?.parse().ok()?;
        b.get(colon + 1..colon + 1 + len)
    }
    fn int(b: &[u8], at: usize) -> Option<u64> {
        if b.get(at) != Some(&b'i') { return None; }
        let end = at + b[at..].iter().position(|c| *c == b'e')?;
        std::str::from_utf8(&b[at + 1..end]).ok()?.parse().ok()
    }
    /// Each key and the offset of its value in the dictionary at `at`.
    fn entries(b: &[u8], at: usize) -> Option<Vec<(&[u8], usize)>> {
        if b.get(at) != Some(&b'd') { return None; }
        let (mut at, mut out) = (at + 1, Vec::new());
        while *b.get(at)? != b'e' { let key = text(b, at)?; let value = skip(b, at, 0)?; out.push((key, value)); at = skip(b, value, 0)?; }
        Some(out)
    }
    let info = entries(bytes, 0)?.into_iter().find(|(k, _)| *k == b"info")?.1;
    let fields = entries(bytes, info)?;
    let field = |name: &[u8]| fields.iter().find(|(k, _)| *k == name).map(|(_, at)| *at);
    let name = String::from_utf8_lossy(text(bytes, field(b"name.utf-8").or(field(b"name"))?)?).into_owned();
    let size = match field(b"files") {
        Some(mut at) => {
            at += 1;
            let mut total = 0u64;
            while *bytes.get(at)? != b'e' {
                total = total.saturating_add(entries(bytes, at)?.into_iter().find(|(k, _)| *k == b"length").and_then(|(_, v)| int(bytes, v))?);
                at = skip(bytes, at, 0)?;
            }
            total
        }
        None => int(bytes, field(b"length")?)?,
    };
    Some((name, size))
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

pub(crate) fn plan(config: &Config, t: &Value) -> Result<Option<Value>> {
    let drives = Drives::of(config);
    // A torrent is filed on the drive it downloaded to, so the move is a rename.
    let Some(drive) = t["download_dir"].as_str().and_then(|dir| drives.downloading_into(Path::new(dir))) else {
        return Ok(None);
    };
    if !complete(t) {
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
    let source = drive.downloads.as_path();
    let dest = drive.library(is_tv);
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
        json!({"hash":t["hash_string"],"name":t["name"],"category":if is_tv{"tv"}else{"movies"},"source":source,"destination":dest,"files":t["files"],"phase":"moving","created":now(),"attempted":0}),
    ))
}

/// A torrent with no activity for this long stops holding a download slot, so the next
/// queued torrent starts. Transmission keeps the idle one connected and resumes it,
/// without a slot, as soon as a peer turns up.
const STALLED_MINUTES: u64 = 2;
/// Torrents trickling in from a single slow peer are not stalled, and with a small
/// queue they would still hold every slot ahead of a well-seeded newcomer. The limit
/// is therefore generous: it only stops a very large batch from starting all at once.
const DOWNLOAD_SLOTS: u64 = 50;
static QUEUE_POLICY_SET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Tell Transmission, once per run, not to let torrents that are waiting for peers clog the queue.
pub async fn apply_queue_policy(s: &Service) {
    use std::sync::atomic::Ordering;
    if QUEUE_POLICY_SET.load(Ordering::Relaxed) { return; }
    if s.rpc.call("session_set", json!({"queue_stalled_enabled":true,"queue_stalled_minutes":STALLED_MINUTES,
        "download_queue_enabled":true,"download_queue_size":DOWNLOAD_SLOTS})).await.is_ok() {
        QUEUE_POLICY_SET.store(true, Ordering::Relaxed);
    }
}

/// What Transmission should be told about a private-site torrent that has seeded for
/// `seeded` seconds: no limit until the time target, then stop at the ratio target.
/// Both must be met, so a torrent nobody wants keeps seeding; that is what such sites reward.
fn private_seeding(seeded: f64, seconds: f64, ratio: f64) -> Value {
    if seeded < seconds { json!({"seed_ratio_mode":2,"seed_idle_mode":2}) }
    else { json!({"seed_ratio_mode":1,"seed_ratio_limit":ratio,"seed_idle_mode":2}) }
}
/// Keep torrents from the member's private site seeding to that site's minimum time and
/// ratio, each with a margin. Transmission has no minimum seeding time of its own.
pub async fn apply_private_seeding(s: &Service) {
    let Some(site) = s.torrent_search.private_site() else { return };
    let (seconds, ratio) = site.seeding_targets();
    let Ok(found) = s.rpc.call("torrent_get", json!({"fields":["hash_string","labels","seconds_seeding","seed_ratio_mode","seed_ratio_limit","seed_idle_mode"]})).await else { return };
    for t in found["torrents"].as_array().into_iter().flatten() {
        if !t["labels"].as_array().is_some_and(|labels| labels.iter().any(|l| l == crate::torrent_search::PRIVATE_LABEL)) { continue; }
        let want = private_seeding(t["seconds_seeding"].as_f64().unwrap_or(0.0), seconds, ratio);
        let same = t["seed_ratio_mode"] == want["seed_ratio_mode"] && t["seed_idle_mode"] == want["seed_idle_mode"]
            && want["seed_ratio_limit"].as_f64().is_none_or(|limit| (t["seed_ratio_limit"].as_f64().unwrap_or(-1.0) - limit).abs() < 0.001);
        if same { continue; }
        let mut change = want.clone();
        change["ids"] = json!([t["hash_string"]]);
        if s.rpc.call("torrent_set", change).await.is_ok() { s.invalidate_torrents(); }
    }
}

pub async fn tick(s: &Service) -> Result<()> {
    apply_queue_policy(s).await;
    // Before the pause check: seeding obligations do not pause with automation.
    apply_private_seeding(s).await;
    let _guard = s.mutation_lock.lock().await;
    let config = s.effective_config()?;
    let all = torrents(s).await?;
    s.change_state(|state| {
        state["last_poll"] = json!(now());
        state["last_error"] = Value::Null;
    })?;
    let drives = Drives::of(&config);
    let online = drives.online();
    tend_session(s, &drives).await;
    let running = remember_running(s, &all, &drives, &online);
    // What unfinished torrents will still write, drive by drive, for placing new ones.
    let mut unfinished: std::collections::HashMap<String, u64> = Default::default();
    for t in &all {
        let Some(drive) = t["download_dir"].as_str().and_then(|dir| drives.drive_of(Path::new(dir))) else { continue };
        *unfinished.entry(drive.id.clone()).or_default() += t["left_until_done"].as_u64().unwrap_or(0);
    }
    crate::drives::commit("torrents", unfinished);
    for t in &all {
        if crate::trials::trial_path(t["download_dir"].as_str().unwrap_or("")) {continue;}
        let hash = text(t, "hash_string")?;
        // A torrent on a drive that is not connected waits for it, stopped: left running, it
        // would finish into a folder Transmission creates on the startup disk.
        if drives.offline(Path::new(t["download_dir"].as_str().unwrap_or("")), &online) { hold_for_drive(s, t, running.contains(hash)).await; continue; }
        // Its files are being copied to another drive: filing it now would leave a copy behind.
        if crate::drive_moves::moving_torrent(s, hash) { continue; }
        release_for_drive(s, t).await;
        reconnect(s, t, running.contains(hash)).await;
        // Pausing automation pauses filing and naming, not the care of a drive that is away.
        if config.paused { continue; }
        settle_placement(s, &drives, t).await;
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

/// Every video in the torrent is identified through its own record, keyed by torrent
/// hash and file index. Untracked records for the same files (such as copies seen in
/// Transmission's incomplete directory while downloading) neither satisfy nor block this.
/// Release adverts named .mkv/.mp4 are not videos (the scanner tags them "not_media")
/// and never will be identified, so they do not hold the pack back.
fn identified(hash: &str, files: &[Value], assets: &[Value]) -> bool {
    // A real but tiny video beside far larger ones (a promo clip) is incidental too.
    let largest = files.iter().filter(|f|f["name"].as_str().is_some_and(|n|video(Path::new(n)))).filter_map(|f|f["length"].as_u64()).max().unwrap_or(0);
    files.iter().enumerate().filter(|(_,f)|f["name"].as_str().is_some_and(|n|video(Path::new(n))))
        .filter(|(_,f)|f["length"].as_u64().is_none_or(|length|length.saturating_mul(20)>=largest))
        .all(|(index,_)|assets.iter().any(|a|a["torrent_hash"]==hash && a["file_index"]==index
            && (a["role"]=="extra" || a["container"]=="not_media" || (a["status"]=="matched" && a["entity_id"].is_string()))))
}

#[cfg(test)]
mod identification_tests {
    use super::*;
    #[test]
    fn release_adverts_do_not_block_an_identified_pack() {
        let files = vec![json!({"name":"Show/Show.S01E01.mkv"}), json!({"name":"Show/visit www.example.invalid.MKV"}), json!({"name":"Show/notes.DOC"})];
        let episode = json!({"torrent_hash":"h","file_index":0,"status":"matched","entity_id":"tv"});
        let advert = json!({"torrent_hash":"h","file_index":1,"status":"no_match","container":"not_media"});
        let unknown = json!({"torrent_hash":"h","file_index":1,"status":"no_match","container":"matroska"});
        assert!(identified("h", &files, &[episode.clone(), advert]));
        // A real video nobody could identify still waits for review.
        assert!(!identified("h", &files, &[episode.clone(), unknown]));
        assert!(!identified("h", &files, &[episode.clone()]));
        // A genuine video advert is far smaller than the episodes and does not block either.
        let sized = vec![json!({"name":"Show/Show.S01E01.mkv","length":900_000_000u64}), json!({"name":"Show/promo.mp4","length":8_000_000u64})];
        assert!(identified("h", &sized, &[episode]));
    }
}

/// Transmission has one folder for new downloads and one for downloads in progress, for
/// every torrent. With one drive that is where they belong. With several, each torrent is
/// given its drive when it is added, and a shared in-progress folder would defeat that:
/// every download would be written to that folder's drive first, fill it, and be copied
/// across when done. So with more than one drive, or when either folder is not on a
/// connected drive (its drive was retired), downloads in progress stay in their own
/// drive's Downloads, under a temporary name, and the default folder is a connected
/// drive's Downloads. Looked at once a minute.
async fn tend_session(s: &Service, drives: &Drives) {
    static LOOKED: std::sync::Mutex<f64> = std::sync::Mutex::new(0.0);
    {
        let Ok(mut looked) = LOOKED.lock() else { return };
        if now() - *looked < 60.0 { return; }
        *looked = now();
    }
    let Ok(session) = s.rpc.call("session_get", json!({"fields":["download_dir","incomplete_dir","incomplete_dir_enabled","rename_partial_files"]})).await else { return };
    let online = drives.online();
    let on_a_drive = |dir: &str| drives.drive_of(Path::new(dir)).is_some_and(|d| online.contains(&d.id));
    let mut change = serde_json::Map::new();
    let shared = session["incomplete_dir_enabled"] == true;
    if shared && (drives.all().len() > 1 || !session["incomplete_dir"].as_str().is_some_and(on_a_drive)) {
        change.insert("incomplete_dir_enabled".into(), json!(false));
        if session["rename_partial_files"] != true { change.insert("rename_partial_files".into(), json!(true)); }
    }
    if !session["download_dir"].as_str().is_some_and(on_a_drive) {
        if let Ok(drive) = drives.place(None, 0) { change.insert("download_dir".into(), json!(drive.downloads)); }
    }
    if change.is_empty() { return; }
    let change = Value::Object(change);
    match s.rpc.call("session_set", change.clone()).await {
        Ok(_) => tracing::info!(settings = %change, "Set Transmission's folders for the drives in use"),
        Err(error) => tracing::warn!(%error, "Could not set Transmission's folders"),
    }
}

fn no_data(t: &Value) -> bool { t["error"].as_u64() == Some(3) && t["error_string"].as_str().is_some_and(|e| e.contains("No data found")) }

/// Which torrents were running when last seen in good order, kept across restarts.
/// Transmission stops every torrent whose data is away with "No data found", the paused
/// and the finished as well as the running, and cannot say afterwards which was which;
/// only those that were running are started again when their drive returns. A torrent
/// stopped that way, or on a drive that is away, keeps what was last known of it.
fn remember_running(s: &Service, all: &[Value], drives: &Drives, online: &HashSet<String>) -> HashSet<String> {
    let before: HashSet<String> = s.db.lock().ok().and_then(|db| db.state().ok())
        .and_then(|state| state["running_torrents"].as_array().map(|list| list.iter().filter_map(|h| h.as_str().map(str::to_owned)).collect()))
        .unwrap_or_default();
    let mut running = HashSet::new();
    for t in all {
        let Some(hash) = t["hash_string"].as_str() else { continue };
        let away = drives.offline(Path::new(t["download_dir"].as_str().unwrap_or("")), online);
        let on = if t["status"].as_u64() != Some(0) { true } else if no_data(t) || away { before.contains(hash) } else { false };
        if on { running.insert(hash.to_owned()); }
    }
    if running != before {
        let mut list: Vec<&String> = running.iter().collect();
        list.sort();
        let _ = s.change_state(|state| state["running_torrents"] = json!(list));
    }
    running
}

/// Stop a torrent whose drive is away, remembering that Torrbaby stopped it. One that
/// Transmission stopped first for the same reason is remembered too, if it was running.
async fn hold_for_drive(s: &Service, t: &Value, was_running: bool) {
    let Some(hash) = t["hash_string"].as_str() else { return };
    if t["status"].as_u64() == Some(0) {
        if was_running && no_data(t) {
            let _ = s.change_state(|state| {
                let held = &mut state["held_for_drive"];
                if !held.is_array() { *held = json!([]); }
                if let Some(list) = held.as_array_mut() { if !list.iter().any(|h| h == hash) { list.push(json!(hash)); } }
            });
        }
        return;
    }
    match s.rpc.call("torrent_stop", json!({"ids":[hash]})).await {
        Ok(_) => {
            tracing::info!(hash, "Stopped a torrent until its drive is connected");
            let _ = s.change_state(|state| {
                let held = &mut state["held_for_drive"];
                if !held.is_array() { *held = json!([]); }
                if let Some(list) = held.as_array_mut() { if !list.iter().any(|h| h == hash) { list.push(json!(hash)); } }
            });
        }
        Err(error) => tracing::warn!(hash, %error, "Could not stop a torrent whose drive is away"),
    }
}

/// Start a torrent Torrbaby stopped for its drive, now that the drive is back. Torrents
/// the user stopped are left as they are.
async fn release_for_drive(s: &Service, t: &Value) {
    let Some(hash) = t["hash_string"].as_str() else { return };
    let held = s.db.lock().ok().and_then(|db| db.state().ok())
        .is_some_and(|state| state["held_for_drive"].as_array().is_some_and(|list| list.iter().any(|h| h == hash)));
    if !held { return; }
    // Files that did not come back show as "No data found", which reconnect() handles.
    match s.rpc.call("torrent_start", json!({"ids":[hash]})).await {
        Ok(_) => {
            tracing::info!(hash, "Drive is back: resumed a torrent stopped for it");
            let _ = s.change_state(|state| {
                if let Some(list) = state["held_for_drive"].as_array_mut() { list.retain(|h| h != hash); }
            });
        }
        Err(error) => tracing::warn!(hash, %error, "Could not resume a torrent after its drive returned"),
    }
}

/// Transmission stops a torrent whose data vanished with its drive ("No data found") and
/// does not try again when the drive returns. Once the data is back, a torrent that was
/// running is started, which clears the error; one the user had stopped, or that had
/// finished seeding, is left stopped. At most every ten minutes per torrent.
async fn reconnect(s: &Service, t: &Value, was_running: bool) {
    static TRIED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, f64>>> = std::sync::LazyLock::new(Default::default);
    if !was_running || !no_data(t) { return; }
    let Some(hash) = t["hash_string"].as_str() else { return };
    // Only when its data is back: checking a torrent whose files were deleted would download it all again.
    let present = t["download_dir"].as_str().is_some_and(|dir| files(t).is_ok_and(|files| files.iter().any(|f|
        f["bytes_completed"].as_u64().unwrap_or(0) > 0 && f["name"].as_str().and_then(|n| checked_path(Path::new(dir), n).ok()).is_some_and(|p| p.is_file()))));
    if !present { return; }
    {
        let Ok(mut tried) = TRIED.lock() else { return };
        if tried.get(hash).is_some_and(|at| now() - at < 600.0) { return; }
        tried.insert(hash.to_owned(), now());
    }
    let ids = json!({"ids":[hash]});
    // Starting is enough: Transmission looks for the files again and clears its error. A
    // full check would read every byte of every torrent on the drive.
    match s.rpc.call("torrent_start", ids).await.and(Ok(())) {
        Ok(()) => tracing::info!(hash, "Drive is back: resumed a torrent"),
        Err(error) => tracing::warn!(hash, %error, "Could not resume a torrent after its drive returned"),
    }
}

/// A magnet's size and name arrive with its metadata, after it was placed, and a .torrent
/// link's name is not known when it is added. Before much data is written, move it once
/// if its drive turns out to lack room, or if its show is on another drive with room.
const SETTLE_BYTES: u64 = 2_000_000_000;
async fn settle_placement(s: &Service, drives: &Drives, t: &Value) {
    static SETTLED: std::sync::LazyLock<std::sync::Mutex<HashSet<String>>> = std::sync::LazyLock::new(Default::default);
    let (Some(hash), Some(dir)) = (t["hash_string"].as_str(), t["download_dir"].as_str()) else { return };
    // A .torrent link (BT之家) starts at once, before the first look: a little data is
    // cheap to carry, and with an incomplete folder it has not reached the drive at all.
    if t["metadata_percent_complete"].as_f64() != Some(1.0) || t["downloaded_ever"].as_u64().unwrap_or(u64::MAX) > SETTLE_BYTES
        || t["percent_done"].as_f64().unwrap_or(1.0) >= 1.0 { return; }
    let Some(current) = drives.downloading_into(Path::new(dir)) else { return };
    if !SETTLED.lock().is_ok_and(|mut settled| settled.insert(hash.to_owned())) { return; }
    if drives.all().len() < 2 { return; }
    let size: u64 = files(t).map(|f| f.iter().filter_map(|f| f["length"].as_u64()).sum()).unwrap_or(0);
    let Some((entities, assets)) = s.db.lock().ok().and_then(|db| Some((db.documents("entities").ok()?, db.documents("assets").ok()?))) else { return };
    let name = t["name"].as_str();
    let prefer = name.and_then(|n| crate::drives::show_for_name(&entities, &assets, n)).and_then(|show| drives.show_drive(&assets, &show));
    let Ok(best) = drives.place(prefer.as_deref(), size) else { return };
    // Data taken over by a check is on disk without having been downloaded this time.
    let left = t["left_until_done"].as_u64().unwrap_or(size);
    if size.saturating_sub(left) > SETTLE_BYTES { return; }
    let moves = best.id != current.id && (!Drives::has_room(current, left) || prefer.as_deref() == Some(best.id.as_str()));
    if !moves { return; }
    match s.rpc.call("torrent_set_location", json!({"ids":[hash],"location":best.downloads,"move":true})).await {
        Ok(_) => tracing::info!(hash, from = current.id, to = best.id, "Placed a new torrent once its size was known"),
        Err(error) => tracing::warn!(hash, %error, "Could not place a new torrent on another drive"),
    }
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
        if t["download_dir"].as_str().is_some_and(|dir| Drives::of(config).downloading_into(Path::new(dir)).is_some()) && complete(t) {
            let assets=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?.documents("assets")?;
            if !identified(hash, files(t)?, &assets) { bail!("Waiting for media identification before moving to the library"); }
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
                // Release packs number specials differently from TMDB, so a special's
                // TMDB title would usually name another special. Leave its name alone.
                if season == 0 {
                    continue;
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_torrent_file_gives_its_name_and_size() {
        let single = b"d8:announce3:url4:infod6:lengthi1500e4:name8:Film.mkv12:piece lengthi16e6:pieces0:ee";
        assert_eq!(metainfo_summary(single), Some(("Film.mkv".into(), 1500)));
        let many = b"d4:infod5:filesld6:lengthi10e4:pathl5:a.mkveed6:lengthi20e4:pathl5:b.mkveee4:name4:Show6:pieces0:ee";
        assert_eq!(metainfo_summary(many), Some(("Show".into(), 30)));
        assert_eq!(metainfo_summary(b"not bencode"), None);
        assert_eq!(metainfo_summary(b"d4:infod4:name"), None);
    }
    #[test]
    fn a_finished_torrent_is_filed_on_the_drive_it_downloaded_to() {
        let root = std::env::temp_dir().join(format!("torrbaby-plan-{}", uuid::Uuid::new_v4()));
        let media = |drive: &str| root.join(drive).join("Media");
        for drive in ["Alpha", "Beta"] { for folder in ["Downloads", "Movies", "TV"] { std::fs::create_dir_all(media(drive).join(folder)).unwrap(); } }
        let finished = |dir: &Path, name: &str| {
            std::fs::create_dir_all(dir.join(name)).unwrap();
            std::fs::write(dir.join(name).join("Show.S01E01.mkv"), vec![0u8; 10]).unwrap();
            json!({"hash_string":"h","name":name,"download_dir":dir,"metadata_percent_complete":1.0,"percent_done":1.0,"left_until_done":0,"error":0,"status":6,
                "files":[{"name":format!("{name}/Show.S01E01.mkv"),"length":10,"bytes_completed":10}]})
        };
        let base = |extra: Value| -> Config {
            let mut v = json!({"listen_host":"127.0.0.1","listen_port":1,"token_file":"t","state_file":"s","transmission_url":"x","transmission_auth_file":null,"poll_seconds":30});
            v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            serde_json::from_value(v).unwrap()
        };
        // Drive 1 from the folders configured before drives existed.
        let legacy = base(json!({"source":media("Alpha").join("Downloads"),"movies":media("Alpha").join("Movies"),"tv":media("Alpha").join("TV")}));
        let plan_one = plan(&legacy, &finished(&media("Alpha").join("Downloads"), "Show.S01")).unwrap().unwrap();
        assert_eq!(plan_one["source"], json!(media("Alpha").join("Downloads")));
        assert_eq!(plan_one["destination"], json!(media("Alpha").join("TV")));
        assert_eq!(plan_one["category"], "tv");
        // A torrent on the second drive is filed on that drive, never moved across.
        let two = base(json!({"drives":[{"id":"alpha","media":media("Alpha")},{"id":"beta","media":media("Beta")}]}));
        let plan_two = plan(&two, &finished(&media("Beta").join("Downloads"), "Other.S01")).unwrap().unwrap();
        assert_eq!(plan_two["source"], json!(media("Beta").join("Downloads")));
        assert_eq!(plan_two["destination"], json!(media("Beta").join("TV")));
        // Already filed, or outside any drive's Downloads: nothing to do.
        assert!(plan(&two, &finished(&media("Beta").join("TV"), "Filed.S01")).unwrap().is_none());
        assert!(plan(&two, &finished(&root.join("Elsewhere"), "Loose.S01")).unwrap().is_none());
        std::fs::remove_dir_all(root).ok();
    }
    #[test]
    fn private_site_torrents_seed_past_the_time_then_to_the_ratio() {
        let (seconds, ratio) = (90.0 * 3600.0, 1.0);
        assert_eq!(private_seeding(0.0, seconds, ratio), json!({"seed_ratio_mode":2,"seed_idle_mode":2}), "no limit before the time is served");
        assert_eq!(private_seeding(seconds - 1.0, seconds, ratio)["seed_ratio_mode"], 2);
        assert_eq!(private_seeding(seconds, seconds, ratio), json!({"seed_ratio_mode":1,"seed_ratio_limit":1.0,"seed_idle_mode":2}));
    }
    #[test]
    fn identification_uses_tracked_records_and_ignores_stale_duplicates() {
        let files = json!([{"name":"Set/1.Dr.No.1962/DR.No.1962.mp4"},{"name":"Set/1.Dr.No.1962/DR.No.1962.srt"},
            {"name":"Set/11.Moonraker.1979/007.James.Bond.Moonraker.1979.mp4"}]);
        let files = files.as_array().unwrap();
        let tracked = |index: u64, status: &str| json!({"torrent_hash":"bond","file_index":index,"status":status,"role":"main",
            "available":true,"complete":true,"entity_id":if status=="matched" {json!(format!("tmdb:movie:{index}"))} else {Value::Null}});
        // Untracked records from the incomplete directory, matched or not.
        let stale = [json!({"path":"/Downloads/Incomplete/Set/11.Moonraker.1979/007.James.Bond.Moonraker.1979.mp4","status":"review","available":false,"complete":false,"role":"main"}),
                     json!({"path":"/Downloads/Incomplete/Set/1.Dr.No.1962/DR.No.1962.mp4","status":"matched","entity_id":"tmdb:movie:646","available":false,"complete":false,"role":"main"})];
        let mut assets = vec![tracked(0, "matched"), tracked(2, "review")];
        assets.extend(stale.clone());
        assert!(!identified("bond", files, &assets), "an unidentified tracked film waits");
        assets[1] = tracked(2, "matched");
        assert!(identified("bond", files, &assets), "stale duplicates do not block");
        // A stale matched duplicate cannot stand in for the tracked record.
        assert!(!identified("bond", files, &[tracked(0, "matched"), stale[1].clone(), stale[0].clone()]));
    }
}
