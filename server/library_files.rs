//! Household-wide file operation previews. Execution must revalidate the fingerprint.
use crate::{api::{Service, checked_path}, organizer, storage::now};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::{Path, PathBuf}, os::unix::fs::MetadataExt};

/// Attach removal intent to live Transmission rows without changing its state.
pub fn annotate_torrents(mut torrents: Value, operations: &[Value]) -> Value {
    if let Some(rows) = torrents.as_array_mut() {
        for torrent in rows {
            if let Some(operation) = operations.iter().find(|op|
                op["package_id"] == torrent["hash_string"] && op["phase"] != "done") {
                torrent["file_operation"] = json!({"id":operation["id"],"action":operation["action"],
                    "phase":operation["phase"],"error":operation["error"]});
            }
        }
    }
    torrents
}

fn file_record(path: &Path, roots: &[PathBuf]) -> Result<Value> {
    ensure!(path.is_absolute(), "Media path must be absolute");
    let root = roots.iter().find(|root| path.starts_with(root) && path != root.as_path())
        .context("File is outside the managed library roots")?;
    ensure!(root.canonicalize()? == *root, "Managed root contains a symlink");
    let relative = path.strip_prefix(root)?.to_str().context("Invalid file name")?;
    let checked = checked_path(root, relative)?;
    let info = std::fs::symlink_metadata(&checked)?;
    ensure!(info.is_file(), "Only regular files can be managed");
    ensure!(info.nlink() == 1, "File has shared hard links; resolve ownership before removal");
    Ok(json!({"path":checked,"bytes":info.len(),"device":info.dev(),"inode":info.ino(),
        "modified":info.mtime(),"modified_ns":info.mtime_nsec()}))
}

fn shared_sidecars(paths: &BTreeSet<PathBuf>) -> Result<Vec<String>> {
    let mut reasons = Vec::new();
    for sidecar in paths.iter().filter(|p|p.extension().and_then(|e|e.to_str())
        .is_some_and(|e|["srt","vtt","ass","ssa","sub","idx"].contains(&e.to_ascii_lowercase().as_str()))) {
        if !sidecar.try_exists()? { continue; }
        let name = sidecar.file_stem().and_then(|s|s.to_str()).context("Invalid subtitle name")?;
        for entry in std::fs::read_dir(sidecar.parent().context("Missing subtitle folder")?)? {
            let other = entry?.path();
            if paths.contains(&other) || !crate::scanner::video(&other) { continue; }
            let stem = other.file_stem().and_then(|s|s.to_str()).context("Invalid neighboring media name")?;
            if name == stem || name.strip_prefix(stem).is_some_and(|tail|tail.starts_with('.')) {
                reasons.push(format!("Subtitle {} may also belong to {}; resolve shared ownership before removal",
                    sidecar.file_name().unwrap_or_default().to_string_lossy(),other.file_name().unwrap_or_default().to_string_lossy()));
            }
        }
    }
    Ok(reasons)
}

fn package_preview(asset: &Value, assets: &[Value], torrents: &[Value], roots: &[PathBuf], active_media: &BTreeSet<String>) -> Result<Value> {
    ensure!(asset["complete"] == true && asset["available"] == true, "Only complete, available media can be managed");
    let torrent = asset["torrent_hash"].as_str().and_then(|hash| torrents.iter().find(|t| t["hash_string"] == hash));
    let mut paths = BTreeSet::new();
    let mut affected = Vec::new();
    let mut reasons = Vec::<String>::new();
    if let Some(t) = torrent {
        ensure!(organizer::complete(t), "The whole download must finish before managing its files");
        let root = Path::new(t["download_dir"].as_str().context("Missing download folder")?);
        for file in t["files"].as_array().context("Missing package files")? {
            paths.insert(checked_path(root, file["name"].as_str().context("Missing package file name")?)?);
        }
    } else {
        paths.insert(PathBuf::from(asset["path"].as_str().context("Missing media path")?));
    }
    for candidate in assets {
        let path = candidate["path"].as_str().unwrap_or("");
        if paths.contains(Path::new(path)) {
            affected.push(json!({"asset_id":candidate["id"],"entity_id":candidate["entity_id"],
                "name":Path::new(path).file_name().and_then(|v|v.to_str()),"season":candidate["season"],"episode":candidate["episode"]}));
            let media = format!("{}:{}", candidate["torrent_hash"].as_str().or(candidate["id"].as_str()).unwrap_or(""), candidate["file_index"].as_u64().unwrap_or(0));
            if active_media.contains(&media) { reasons.push("Someone is playing a file in this package".into()); }
        }
    }
    // Include only subtitle names associated with an exact media stem, never a parent directory.
    for media in paths.clone().iter().filter(|p| crate::scanner::video(p)) {
        let stem = media.file_stem().and_then(|s|s.to_str()).context("Invalid media name")?;
        for entry in std::fs::read_dir(media.parent().context("Missing parent folder")?)? {
            let path = entry?.path();
            let ext = path.extension().and_then(|s|s.to_str()).unwrap_or("").to_ascii_lowercase();
            let name = path.file_stem().and_then(|s|s.to_str()).unwrap_or("");
            if ["srt","vtt","ass","ssa","sub","idx"].contains(&ext.as_str())
                && (name == stem || name.strip_prefix(stem).is_some_and(|tail|tail.starts_with('.'))) {
                paths.insert(path);
            }
        }
    }
    for other in torrents {
        if torrent.is_some_and(|t| t["hash_string"] == other["hash_string"]) { continue; }
        let root = Path::new(other["download_dir"].as_str().context("Cannot verify other torrent ownership")?);
        for file in other["files"].as_array().context("Cannot verify other torrent files")? {
            let path = checked_path(root, file["name"].as_str().context("Invalid torrent file")?)?;
            if paths.contains(&path) { reasons.push("Another torrent shares files in this package".into()); }
        }
    }
    reasons.extend(shared_sidecars(&paths)?);
    let mut files = Vec::new();
    for path in &paths {
        match file_record(path, roots) {
            Ok(record) => files.push(record),
            Err(error) => reasons.push(format!("{}: {error}", path.display())),
        }
    }
    reasons.sort(); reasons.dedup();
    // is_finished is Transmission's seed-limit result, never inferred from stopped/paused status.
    let seeding_remaining = torrent.is_some_and(|t| t["is_finished"] != true);
    let mut actions = vec![];
    if reasons.is_empty() {
        actions.push("delete");
        if seeding_remaining { actions.push("junk"); }
    }
    let mut preview = json!({"asset_id":asset["id"], "package_id":torrent.map(|t| t["hash_string"].clone()).unwrap_or(asset["id"].clone()),
        "name":torrent.map(|t|t["name"].clone()).unwrap_or(json!(Path::new(asset["path"].as_str().unwrap_or("")).file_name().and_then(|v|v.to_str()))),
        "download_dir":torrent.map(|t|t["download_dir"].clone()),"torrent_hash":torrent.map(|t|t["hash_string"].clone()),"scope":if torrent.is_some(){"entire_download"}else{"file"},
        "affects_everyone":true,"seeding_remaining":seeding_remaining,"assets":affected,
        "total_bytes":files.iter().filter_map(|f|f["bytes"].as_u64()).sum::<u64>(),"files":files,
        "allowed_actions":actions,"blocked_reasons":reasons});
    if let Some(torrent) = torrent {
        let mut policy = serde_json::Map::new();
        for field in ["seed_ratio_mode", "seed_ratio_limit", "seed_idle_mode", "seed_idle_limit"] {
            if let Some(value) = torrent.get(field).filter(|v|v.is_number()) {
                policy.insert(field.to_owned(),value.clone());
            }
        }
        preview["seed_policy"] = Value::Object(policy);
    }
    let fingerprint: String = Sha256::digest(serde_json::to_vec(&preview)?).iter().map(|b| format!("{b:02x}")).collect();
    preview["confirmation_token"] = json!(fingerprint);
    Ok(preview)
}

pub async fn preview(s: &Service, id: &str) -> Result<Value> {
    let torrents = organizer::torrents(s).await?;
    let config = s.effective_config()?;
    let roots = crate::drives::Drives::of(&config).all_roots().map(|(_, _, root)| root.to_path_buf()).collect::<Vec<_>>();
    let (asset, assets, active) = {
        let db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
        let active = db.rows("SELECT DISTINCT s.media_id FROM playback_sessions s JOIN profile_playback p ON p.active_session=s.id WHERE s.ended=0 AND p.last_played>?", [now()-30.0])?
            .into_iter().filter_map(|v|v["media_id"].as_str().map(str::to_owned)).collect();
        (db.asset(id)?, db.documents("assets")?, active)
    };
    package_preview(&asset, &assets, &torrents, &roots, &active)
}

pub async fn submit(s: &Service, asset_id: &str, action: &str, token: &str) -> Result<Value> {
    ensure!(action == "delete" || action == "junk", "Unknown file action");
    let _mutation = s.mutation_lock.lock().await;
    // A repeated confirmation can arrive after the torrent or files have gone.
    {
        let db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
        // A delayed retry must never authorize removal of a new download incarnation.
        if let Some(operation) = db.archived_confirmation(asset_id, action, token)? { return Ok(operation); }
        for operation in db.library_operations()? {
            let saved: Value = serde_json::from_str(operation["preview"].as_str().context("Invalid operation manifest")?)?;
            if saved["assets"].as_array().is_some_and(|a|a.iter().any(|a|a["asset_id"] == asset_id)) {
                ensure!(operation["action"] == action && saved["confirmation_token"] == token,"This package already has a different removal operation");
                return Ok(operation);
            }
        }
    }
    let current = preview(s,asset_id).await?;
    let config = s.effective_config()?;
    let destination = if action == "junk" {
        let source = Path::new(current["download_dir"].as_str().context("Junk requires a torrent package")?);
        let drives = crate::drives::Drives::of(&config);
        let (_, _, managed) = drives.root_of(source).context("Unmanaged torrent location")?;
        let hash = current["torrent_hash"].as_str().context("Missing torrent hash")?;
        ensure!(hash.len() == 40 && hash.bytes().all(|b|b.is_ascii_hexdigit()),"Invalid torrent identity");
        let destination = checked_path(Path::new(managed), &format!("Junk/{hash}"))?;
        ensure!(!destination.exists(),"Junk destination already exists; resolve it before moving this package");
        let device = std::fs::metadata(managed)?.dev();
        ensure!(current["files"].as_array().context("Missing file manifest")?.iter().all(|f|f["device"].as_u64() == Some(device)),
            "Junk destination must be on the same volume as the package");
        Some(destination.to_str().context("Invalid Junk path")?.to_owned())
    } else { None };
    let mut db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
    db.accept_library_operation(&current,action,token,destination.as_deref())
}

fn prepare_destination(operation: &Value, roots: &[PathBuf]) -> Result<PathBuf> {
    use std::io::Write;
    let destination = PathBuf::from(operation["destination"].as_str().context("Missing Junk destination")?);
    let root = roots.iter().find(|r|destination.starts_with(r.join("Junk"))).context("Junk is outside managed roots")?;
    let relative = destination.strip_prefix(root)?.to_str().context("Invalid Junk path")?;
    checked_path(root,relative)?;
    ensure!(root.canonicalize()? == *root,"Managed root changed");
    std::fs::create_dir_all(&destination)?;
    let marker = destination.join(".torrbaby-operation");
    let id = operation["id"].as_str().context("Invalid operation")?;
    if marker.exists() {
        ensure!(!marker.is_symlink() && std::fs::read_to_string(&marker)? == id,"Junk belongs to another operation");
    } else {
        ensure!(std::fs::read_dir(&destination)?.next().is_none(),"Junk destination is not empty");
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(marker)?;
        file.write_all(id.as_bytes())?; file.sync_all()?;
    }
    Ok(destination)
}

/// Torrent files have already moved through Transmission. Associated sidecars
/// move by same-volume rename, preserving their accepted inode across retries.
fn relocate_manifest(original: &Value, destination: &Path, roots: &[PathBuf]) -> Result<Value> {
    let source_root = Path::new(original["download_dir"].as_str().context("Missing original location")?);
    let mut moved = original.clone();
    let mut files = Vec::new();
    for expected in original["files"].as_array().context("Missing original files")? {
        let source = Path::new(expected["path"].as_str().context("Invalid original path")?);
        let relative = source.strip_prefix(source_root)?.to_str().context("Invalid relative path")?;
        let target = checked_path(destination,relative)?;
        if std::fs::symlink_metadata(source).is_ok() {
            ensure!(file_record(source,roots)? == *expected,"Source file changed during move");
            ensure!(!target.exists(),"Both original and destination exist; resolve the move before continuing");
            let parent = target.parent().context("Missing destination parent")?;
            std::fs::create_dir_all(parent)?;
            ensure!(std::fs::metadata(parent)?.dev() == expected["device"].as_u64().context("Missing file device")?,"Sidecar needs same-volume relocation");
            std::fs::rename(source,&target)?;
        }
        let actual = file_record(&target,roots)?;
        let mut expected_at_target = expected.clone(); expected_at_target["path"] = json!(target);
        ensure!(actual == expected_at_target,"Moved file does not match its confirmed identity");
        files.push(actual);
    }
    moved["files"] = json!(files);
    moved["download_dir"] = json!(destination);
    Ok(moved)
}

fn cleanup_destination(operation: &Value, manifest: &Value, roots: &[PathBuf]) -> Result<()> {
    let destination = PathBuf::from(operation["destination"].as_str().context("Missing Junk destination")?);
    if !destination.exists() { return Ok(()); }
    prepare_destination(operation,roots)?;
    let mut parents = BTreeSet::new();
    for file in manifest["files"].as_array().context("Missing files")? {
        let path = Path::new(file["path"].as_str().context("Invalid file path")?);
        for parent in path.ancestors().skip(1).take_while(|p|p.starts_with(&destination)) { parents.insert(parent.to_path_buf()); }
    }
    for parent in parents.iter().rev().filter(|p|**p != destination) {
        match std::fs::remove_dir(parent) {
            Ok(()) => {},
            Err(e) if matches!(e.kind(),std::io::ErrorKind::NotFound|std::io::ErrorKind::DirectoryNotEmpty) => {},
            Err(e) => return Err(e.into()),
        }
    }
    std::fs::remove_file(destination.join(".torrbaby-operation"))?;
    match std::fs::remove_dir(destination) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn verify_no_shared_files(preview: &Value, torrents: &[Value]) -> Result<()> {
    let owned: BTreeSet<PathBuf> = preview["files"].as_array().context("Missing file manifest")?.iter()
        .map(|f|f["path"].as_str().map(PathBuf::from).context("Missing file path")).collect::<Result<_>>()?;
    let conflicts = shared_sidecars(&owned)?;
    ensure!(conflicts.is_empty(),"{}",conflicts.join("; "));
    let paths: BTreeSet<_> = preview["files"].as_array().context("Invalid owned manifest")?.iter()
        .filter_map(|f|f["path"].as_str().map(PathBuf::from)).collect();
    for torrent in torrents {
        if torrent["hash_string"] == preview["torrent_hash"] { continue; }
        let root = Path::new(torrent["download_dir"].as_str().context("Cannot verify torrent ownership")?);
        for file in torrent["files"].as_array().context("Cannot verify torrent files")? {
            let path = checked_path(root,file["name"].as_str().context("Invalid torrent file name")?)?;
            ensure!(!paths.contains(&path),"Another torrent now shares files in this package");
        }
    }
    Ok(())
}

fn purge_caches(s: &Service, preview: &Value) -> Result<()> {
    let assets = preview["assets"].as_array().context("Missing affected assets")?.iter()
        .filter_map(|a|a["asset_id"].as_str().map(str::to_owned)).collect();
    s.hls.purge_assets(&assets)?;
    s.virtual_media.purge_assets(&assets)?;
    s.subtitle_jobs.purge_assets(&assets)?;
    Ok(())
}

async fn advance(s: &Service, operation: &Value) -> Result<()> {
    let id = operation["id"].as_str().context("Invalid operation identity")?;
    let phase = operation["phase"].as_str().context("Invalid operation phase")?;
    let original: Value = serde_json::from_str(operation["preview"].as_str().context("Missing operation manifest")?)?;
    let manifest = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?.relocated_manifest(id)?.unwrap_or(original.clone());
    let config = s.effective_config()?;
    let roots = crate::drives::Drives::of(&config).all_roots().map(|(_, _, root)| root.to_path_buf()).collect::<Vec<_>>();
    let advance = |next: &str| -> Result<()> {
        s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?.advance_library_operation(id,phase,next)
    };
    match phase {
        "accepted" => {
            let remaining = validate_owned_files(&manifest,&roots)?;
            if operation["action"] == "junk" {
                ensure!(remaining.len() == manifest["files"].as_array().context("Missing files")?.len(),"Files disappeared before relocation");
                prepare_destination(operation,&roots)?;
                advance("moving")
            } else { advance("detaching") }
        }
        "moving" => {
            let destination = prepare_destination(operation,&roots)?;
            let hash = original["torrent_hash"].as_str().context("Missing torrent hash")?;
            let torrents = organizer::torrents(s).await?;
            verify_no_shared_files(&original,&torrents)?;
            let torrent = torrents.iter().find(|t|t["hash_string"] == hash).context("Torrent disappeared during relocation; files are retained")?;
            if torrent["download_dir"].as_str() == destination.to_str() {
                return advance("verifying");
            }
            ensure!(torrent["download_dir"] == original["download_dir"],"Torrent moved to an unexpected location");
            let remaining = validate_owned_files(&original,&roots)?;
            ensure!(remaining.len() == original["files"].as_array().context("Operation has no file list")?.len(),"Waiting for the in-progress move to finish");
            ensure!(std::fs::read_dir(&destination)?.all(|e|e.is_ok_and(|e|e.file_name() == ".torrbaby-operation")),"Waiting for the in-progress move to finish");
            purge_caches(s,&original)?;
            s.rpc.call("torrent_set_location",json!({"ids":[hash],"location":destination,"move":true})).await?;
            s.invalidate_torrents();
            // The RPC may acknowledge before moving finishes. Verify on a later poll.
            Ok(())
        }
        "verifying" => {
            let destination = prepare_destination(operation,&roots)?;
            let hash = original["torrent_hash"].as_str().context("Missing torrent hash")?;
            let torrent = organizer::torrent(s,hash).await?;
            ensure!(torrent["download_dir"].as_str() == destination.to_str(),"Waiting for Transmission relocation");
            ensure!(organizer::complete(&torrent),"Waiting for a complete, readable package after relocation");
            let moved = relocate_manifest(&original,&destination,&roots)?;
            verify_no_shared_files(&moved,&organizer::torrents(s).await?)?;
            if torrent["is_finished"] != true {
                s.rpc.call("torrent_start",json!({"ids":[hash]})).await?;
                // Transmission may disable a met ratio/idle limit on manual start.
                // Replay the confirmed policy, including after a lost RPC response.
                if let Some(policy) = original["seed_policy"].as_object().filter(|p|!p.is_empty()) {
                    let mut params = policy.clone();
                    params.insert("ids".to_owned(),json!([hash]));
                    s.rpc.call("torrent_set",Value::Object(params)).await?;
                }
            }
            s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?.finish_junk_move(id,&moved)
        }
        "seeding" => {
            let hash = manifest["torrent_hash"].as_str().context("Missing torrent hash")?;
            let torrent = organizer::torrent(s,hash).await?;
            ensure!(torrent["download_dir"] == manifest["download_dir"],"Junk location changed; files are retained");
            // Paused, queued, stalled, and unavailable states are never completion.
            if torrent["is_finished"] == true && organizer::complete(&torrent) {
                advance("detaching")
            } else { Ok(()) }
        }
        "detaching" => {
            let torrents = organizer::torrents(s).await?;
            verify_no_shared_files(&manifest,&torrents)?;
            validate_owned_files(&manifest,&roots)?;
            if let Some(hash) = manifest["torrent_hash"].as_str() {
                if let Some(torrent) = torrents.iter().find(|t|t["hash_string"] == hash) {
                    let root = Path::new(torrent["download_dir"].as_str().context("Missing download folder")?);
                    let owned = manifest["files"].as_array().context("Missing owned manifest")?;
                    for file in torrent["files"].as_array().context("Missing torrent files")? {
                        let path = checked_path(root,file["name"].as_str().context("Missing torrent filename")?)?;
                        ensure!(owned.iter().any(|f|f["path"].as_str() == path.to_str()), "Torrent location changed after confirmation");
                    }
                    // Keep disk ownership here; Transmission must not recursively remove data.
                    s.rpc.call("torrent_remove",json!({"ids":[hash],"delete_local_data":false})).await?;
                    s.invalidate_torrents();
                    ensure!(!organizer::torrents(s).await?.iter().any(|t|t["hash_string"] == hash),"Waiting for Transmission to detach this package");
                }
            }
            advance("deleting")
        }
        "deleting" => {
            let torrents = organizer::torrents(s).await?;
            if let Some(hash) = manifest["torrent_hash"].as_str() {
                ensure!(!torrents.iter().any(|t|t["hash_string"] == hash),"Torrent was re-added; resolve ownership before removal");
            }
            verify_no_shared_files(&manifest,&torrents)?;
            purge_caches(s,&manifest)?;
            delete_owned_files(&manifest,&roots)?;
            advance("cleanup")
        }
        "cleanup" => {
            purge_caches(s,&manifest)?;
            if operation["action"] == "junk" { cleanup_destination(operation,&manifest,&roots)?; }
            advance("done")
        }
        "done" => Ok(()),
        _ => anyhow::bail!("Unsupported file operation phase"),
    }
}

pub async fn tick(s: &Service) -> Result<()> {
    let _mutation = s.mutation_lock.lock().await;
    let operations = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?.library_operations()?;
    for operation in operations {
        if operation["phase"] == "done" { continue; }
        if let Err(error) = advance(s,&operation).await {
            s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?
                .fail_library_operation(operation["id"].as_str().context("Invalid operation")?,&error.to_string())?;
        } else {
            s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?
                .clear_library_operation_error(&operation)?;
        }
    }
    Ok(())
}

/// Reconciliation may see already removed files after a crash. Existing files must
/// still have the exact identity accepted by the user; a replacement is never ours.
fn validate_owned_files(preview: &Value, roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut remaining = Vec::new();
    for expected in preview["files"].as_array().context("Missing owned file manifest")? {
        let path = PathBuf::from(expected["path"].as_str().context("Missing owned file path")?);
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
            Ok(_) => {}
        }
        ensure!(file_record(&path, roots)? == *expected, "File changed after confirmation: {}", path.display());
        remaining.push(path);
    }
    Ok(remaining)
}

/// Caller must hold the mutation lock, detach Transmission, and persist phase
/// `deleting` before entering. No parent directory is recursively deleted.
fn delete_owned_files(preview: &Value, roots: &[PathBuf]) -> Result<()> {
    let paths = validate_owned_files(preview, roots)?;
    for path in paths {
        let expected = preview["files"].as_array().into_iter().flatten().find(|v|v["path"].as_str() == path.to_str()).context("File is not part of the removal plan")?;
        ensure!(file_record(&path,roots)? == *expected, "File changed during removal");
        std::fs::remove_file(path)?;
    }
    Ok(())
}

impl crate::storage::Database {
    pub fn archived_confirmation(&self, asset: &str, action: &str, token: &str) -> Result<Option<Value>> {
        for row in self.rows("SELECT document FROM library_operation_archive", [])? {
            let operation: Value = serde_json::from_str(row["document"].as_str().context("Invalid archived operation")?)?;
            let preview: Value = serde_json::from_str(operation["preview"].as_str().context("Invalid archived manifest")?)?;
            if operation["action"] == action && preview["confirmation_token"] == token
                && preview["assets"].as_array().is_some_and(|a|a.iter().any(|a|a["asset_id"] == asset)) {
                return Ok(Some(operation));
            }
        }
        Ok(None)
    }

    /// Explicit re-add releases a completed removal only. Keep its confirmation
    /// receipt, but retain personal media/history rows and asset identities.
    pub fn restore_completed_package(&mut self, hash: &str) -> Result<bool> {
        let Some(operation) = self.library_operations()?.into_iter().find(|op|op["package_id"] == hash) else { return Ok(false); };
        if operation["phase"] != "done" { return Ok(false); }
        let id = operation["id"].as_str().context("Missing operation identity")?;
        let mut state = self.state()?;
        if let Some(jobs) = state["jobs"].as_object_mut() { jobs.remove(hash); }
        let tx = self.connection.transaction()?;
        tx.execute("INSERT INTO library_operation_archive VALUES(?,?,?,?)",rusqlite::params![id,hash,operation.to_string(),now()])?;
        // Old paths may still be present in the catalog snapshot. A fresh scan must
        // verify the new package before any of its assets become available again.
        tx.execute("UPDATE library_assets SET document=json_set(document,'$.available',json('false'),'$.complete',json('false'),'$.torrent_present',json('false')) WHERE json_extract(document,'$.torrent_hash')=?",[hash])?;
        tx.execute("DELETE FROM library_operation_assets WHERE operation_id=?",[id])?;
        tx.execute("DELETE FROM library_operation_runtime WHERE operation_id=?",[id])?;
        tx.execute("DELETE FROM library_operations WHERE id=? AND phase='done'",[id])?;
        tx.execute("UPDATE organizer_state SET document=? WHERE id=1",[state.to_string()])?;
        tx.commit()?;
        Ok(true)
    }
    /// The accepted preview and all affected IDs are committed before any external mutation.
    /// Retrying the same request returns its original operation, including after a restart.
    pub fn accept_library_operation(&mut self, preview: &Value, action: &str, token: &str, destination: Option<&str>) -> Result<Value> {
        use rusqlite::{params, OptionalExtension};
        let package = preview["package_id"].as_str().context("Missing package identity")?;
        let tx = self.connection.transaction()?;
        let existing: Option<(String,String,String)> = tx.query_row(
            "SELECT id,action,preview FROM library_operations WHERE package_id=?", [package],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        if let Some((id, previous, saved)) = existing {
            let saved: Value = serde_json::from_str(&saved)?;
            ensure!(previous == action && saved["confirmation_token"] == token, "This package already has a different removal operation");
            drop(tx);
            return self.library_operation(&id);
        }
        ensure!(preview["confirmation_token"].as_str() == Some(token), "Files changed; review the updated package before confirming");
        ensure!(preview["allowed_actions"].as_array().is_some_and(|a|a.iter().any(|v|v == action)), "This file operation is not currently allowed");
        ensure!(action == "delete" || action == "junk", "Unknown file action");
        ensure!(action != "junk" || destination.is_some(), "Junk needs a verified destination");
        let id = format!("operation-{}", uuid::Uuid::new_v4());
        let time = now();
        tx.execute("INSERT INTO library_operations VALUES(?,?,?,'accepted',?,?,NULL,?,?)",
            params![id,package,action,preview.to_string(),destination,time,time])?;
        for asset in preview["assets"].as_array().context("Missing affected assets")? {
            let asset_id = asset["asset_id"].as_str().context("Missing affected asset ID")?;
            tx.execute("INSERT INTO library_operation_assets VALUES(?,?)", params![asset_id,id])?;
        }
        tx.commit()?;
        self.library_operation(&id)
    }
    pub fn relocated_manifest(&self, id: &str) -> Result<Option<Value>> {
        use rusqlite::OptionalExtension;
        let saved: Option<String> = self.connection.query_row("SELECT document FROM library_operation_runtime WHERE operation_id=?",[id],|r|r.get(0)).optional()?;
        saved.map(|v|serde_json::from_str(&v).map_err(Into::into)).transpose()
    }
    pub fn finish_junk_move(&mut self, id: &str, manifest: &Value) -> Result<()> {
        let tx = self.connection.transaction()?;
        ensure!(tx.execute("UPDATE library_operations SET phase='seeding',error=NULL,updated=? WHERE id=? AND phase='verifying'",rusqlite::params![now(),id])? == 1,"Relocation phase changed");
        tx.execute("INSERT OR REPLACE INTO library_operation_runtime VALUES(?,?)",rusqlite::params![id,manifest.to_string()])?;
        tx.commit()?;
        Ok(())
    }
    pub fn library_operation(&self, id: &str) -> Result<Value> {
        self.rows("SELECT * FROM library_operations WHERE id=?",[id])?.into_iter().next().context("File operation not found")
    }
    pub fn library_operations(&self) -> Result<Vec<Value>> {
        self.rows("SELECT * FROM library_operations ORDER BY created", [])
    }
    pub fn hidden_asset(&self, asset: &Value) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM library_operation_assets WHERE asset_id=?) OR EXISTS(SELECT 1 FROM library_operations WHERE package_id=?)",
            rusqlite::params![asset["id"].as_str().unwrap_or(""),asset["torrent_hash"].as_str().unwrap_or("")], |r|r.get(0))?)
    }
    pub fn hidden_package(&self, hash: &str) -> Result<bool> {
        Ok(self.connection.query_row("SELECT EXISTS(SELECT 1 FROM library_operations WHERE package_id=?)",[hash],|r|r.get(0))?)
    }
    pub fn visible_assets(&self) -> Result<Vec<Value>> {
        // Non-video files with video extensions (release adverts) are not titles or work.
        self.documents("assets")?.into_iter().filter(|a| !crate::container::tagged_not_media(a)).filter_map(|a| match self.hidden_asset(&a) {
            Ok(false) => Some(Ok(a)), Ok(true) => None, Err(e) => Some(Err(e))
        }).collect()
    }
    /// Compare-and-swap prevents a delayed worker from overwriting a later durable phase.
    pub fn advance_library_operation(&self, id: &str, expected: &str, next: &str) -> Result<()> {
        let operation = self.library_operation(id)?;
        let permitted = match (expected,next) {
            ("accepted","moving") | ("moving","verifying") | ("verifying","seeding") => operation["action"] == "junk",
            ("accepted","detaching") => operation["action"] == "delete",
            ("seeding","detaching") | ("detaching","deleting") | ("deleting","cleanup") | ("cleanup","done") => true,
            _ => false,
        };
        ensure!(permitted,"Invalid library operation phase transition");
        ensure!(self.connection.execute("UPDATE library_operations SET phase=?,error=NULL,updated=? WHERE id=? AND phase=?",
            rusqlite::params![next,now(),id,expected])? == 1, "Library operation changed; reload before continuing");
        Ok(())
    }
    pub fn fail_library_operation(&self, id: &str, error: &str) -> Result<()> {
        self.connection.execute("UPDATE library_operations SET error=?,updated=? WHERE id=?",rusqlite::params![error,now(),id])?;
        Ok(())
    }
    /// A successful poll can recover without changing phase (for example, still
    /// seeding after an RPC outage). Do not keep displaying the previous outage.
    pub fn clear_library_operation_error(&self, observed: &Value) -> Result<()> {
        if observed["error"].is_null() { return Ok(()); }
        self.connection.execute(
            "UPDATE library_operations SET error=NULL,updated=? WHERE id=? AND phase=? AND updated=?",
            rusqlite::params![now(),observed["id"].as_str(),observed["phase"].as_str(),observed["updated"].as_f64()])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn torrent_annotation_keeps_paused_junk_and_preserves_rpc_state() {
        let rows = json!([{"hash_string":"a","status":0,"is_finished":false},
            {"hash_string":"b","status":6},{"hash_string":"c","status":0}]);
        let operations = vec![json!({"package_id":"a","id":"op-a","action":"junk","phase":"seeding","error":"Disk unavailable"}),
            json!({"package_id":"b","id":"op-b","action":"delete","phase":"done"})];
        let annotated = annotate_torrents(rows.clone(), &operations);
        assert_eq!(annotated[0]["file_operation"]["action"], "junk");
        assert_eq!(annotated[0]["file_operation"]["error"], "Disk unavailable");
        assert_eq!(annotated[0]["status"], 0);
        assert_eq!(annotated[0]["is_finished"], false);
        assert_eq!(annotated[1], rows[1]);
        assert_eq!(annotated[2], rows[2]);
    }
    #[test]
    fn delete_replay_keeps_neighbors_and_rejects_replaced_files() -> Result<()> {
        let root = std::env::temp_dir().canonicalize()?.join(format!("torrbaby-delete-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let run = || -> Result<()> {
            let first = root.join("first.mp4"); let second = root.join("second.srt"); let neighbor = root.join("neighbor.mp4");
            for path in [&first,&second,&neighbor] { std::fs::write(path,b"fixture")?; }
            let roots = vec![root.clone()];
            let manifest = json!({"files":[file_record(&first,&roots)?,file_record(&second,&roots)?]});
            // Replace a file under the same name. Nothing in the package is deleted.
            std::fs::rename(&second,root.join("original.srt"))?;
            std::fs::write(&second,b"replacement")?;
            assert!(delete_owned_files(&manifest,&roots).is_err());
            assert!(first.exists()); assert!(neighbor.exists());
            std::fs::remove_file(&second)?;
            std::fs::rename(root.join("original.srt"),&second)?;
            // Simulate a crash after deleting the first file.
            std::fs::remove_file(&first)?;
            delete_owned_files(&manifest,&roots)?;
            delete_owned_files(&manifest,&roots)?;
            assert!(!first.exists() && !second.exists());
            assert!(neighbor.exists() && root.is_dir());
            Ok(())
        };
        let result = run(); std::fs::remove_dir_all(root)?; result
    }
    #[test]
    fn removal_intent_survives_restart_and_never_changes_personal_history() -> Result<()> {
        let root = std::env::temp_dir().join(format!("torrbaby-operations-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("state.sqlite3");
        let run = || -> Result<()> {
            let mut db = crate::storage::Database::open(&path,None)?;
            let asset = json!({"id":"asset-one","torrent_hash":"hash","path":"/fixture/movie.mp4","available":true,"complete":true});
            db.connection.execute("INSERT INTO library_assets VALUES(?,?)",rusqlite::params!["asset-one",asset.to_string()])?;
            db.connection.execute("INSERT INTO media VALUES('hash:0','hash',0,'Movie',100)",[])?;
            db.connection.execute("INSERT INTO profile_playback(profile_id,media_id,position,duration,watched) VALUES('profile-jon','hash:0',950,1000,1)",[])?;
            let preview = json!({"package_id":"hash","confirmation_token":"fingerprint","allowed_actions":["delete","junk"],"assets":[{"asset_id":"asset-one"}]});
            assert!(db.accept_library_operation(&preview,"junk","stale",Some("/fixture/Junk/hash")).is_err());
            assert!(!db.hidden_asset(&asset)?);
            let op = db.accept_library_operation(&preview,"junk","fingerprint",Some("/fixture/Junk/hash"))?;
            let id = op["id"].as_str().unwrap();
            assert_eq!(db.accept_library_operation(&preview,"junk","fingerprint",Some("/fixture/Junk/hash"))?["id"],op["id"]);
            assert!(db.hidden_asset(&asset)?);
            assert!(db.visible_assets()?.is_empty());
            assert!(db.catalog(crate::profiles::LEGACY_PROFILE)?["titles"].as_array().unwrap().is_empty());
            assert!(db.accept_library_operation(&preview,"delete","fingerprint",None).is_err());
            assert!(db.advance_library_operation(id,"accepted","done").is_err());
            db.advance_library_operation(id,"accepted","moving")?;
            assert!(db.advance_library_operation(id,"accepted","moving").is_err());
            db.fail_library_operation(id,"Interrupted move")?;
            drop(db);
            let mut db = crate::storage::Database::open(&path,None)?;
            assert_eq!(db.library_operation(id)?["phase"],"moving");
            assert_eq!(db.library_operation(id)?["error"],"Interrupted move");
            let failed = db.library_operation(id)?;
            db.clear_library_operation_error(&failed)?;
            assert_eq!(db.library_operation(id)?["phase"],"moving");
            assert!(db.library_operation(id)?["error"].is_null());
            db.fail_library_operation(id,"New failure")?;
            db.clear_library_operation_error(&failed)?;
            assert_eq!(db.library_operation(id)?["error"],"New failure");
            assert!(!db.restore_completed_package("hash")?);
            assert!(db.hidden_package("hash")?);
            // A rescan must not expose the same torrent under a fresh asset ID.
            assert!(db.hidden_asset(&json!({"id":"asset-new","torrent_hash":"hash"}))?);
            db.advance_library_operation(id,"moving","verifying")?;
            assert!(db.library_operation(id)?["error"].is_null());
            for (from,to) in [("verifying","seeding"),("seeding","detaching"),("detaching","deleting"),("deleting","cleanup"),("cleanup","done")] {
                db.advance_library_operation(id,from,to)?;
            }
            assert!(db.hidden_asset(&asset)?);
            assert_eq!(db.history(crate::profiles::LEGACY_PROFILE)?[0]["watched"],true);
            assert_eq!(db.library_operations()?.len(),1);
            assert!(db.restore_completed_package("hash")?);
            assert!(!db.hidden_package("hash")?);
            assert!(!db.hidden_asset(&asset)?);
            assert_eq!(db.asset("asset-one")?["available"],false);
            assert_eq!(db.asset("asset-one")?["complete"],false);
            assert_eq!(db.history(crate::profiles::LEGACY_PROFILE)?[0]["watched"],true);
            assert!(db.catalog(crate::profiles::LEGACY_PROFILE)?["titles"].as_array().unwrap().is_empty());
            assert_eq!(db.archived_confirmation("asset-one","junk","fingerprint")?.unwrap()["id"],op["id"]);
            let mut next_preview = preview.clone();
            next_preview["confirmation_token"] = json!("new-files");
            let next = db.accept_library_operation(&next_preview,"delete","new-files",None)?;
            assert_ne!(next["id"],op["id"]);
            assert_eq!(db.archived_confirmation("asset-one","junk","fingerprint")?.unwrap()["phase"],"done");
            drop(db);
            let db = crate::storage::Database::open(&path,None)?;
            assert!(db.hidden_package("hash")?);
            assert_eq!(db.archived_confirmation("asset-one","junk","fingerprint")?.unwrap()["id"],op["id"]);
            assert_eq!(db.history(crate::profiles::LEGACY_PROFILE)?[0]["watched"],true);
            Ok(())
        };
        let result = run(); std::fs::remove_dir_all(root)?; result
    }
    #[test]
    fn preview_scopes_package_and_blocks_sharing_and_playback() -> Result<()> {
        let root = std::env::temp_dir().canonicalize()?.join(format!("torrbaby-files-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let run = || -> Result<()> {
            std::fs::write(root.join("one.mp4"), b"one")?;
            std::fs::write(root.join("two.mp4"), b"two")?;
            std::fs::write(root.join("one.en.srt"), b"subtitle")?;
            let a = json!({"id":"asset-one","path":root.join("one.mp4"),"complete":true,"available":true,"torrent_hash":"hash","file_index":0});
            let b = json!({"id":"asset-two","path":root.join("two.mp4"),"complete":true,"available":true,"torrent_hash":"hash","file_index":1});
            let t = json!({"hash_string":"hash","name":"Season","download_dir":root,"metadata_percent_complete":1.0,"percent_done":1.0,"left_until_done":0,"error":0,"status":0,"is_finished":false,
                "files":[{"name":"one.mp4","length":3,"bytes_completed":3},{"name":"two.mp4","length":3,"bytes_completed":3}]});
            let assets = vec![a.clone(),b]; let roots = vec![root.clone()]; let empty = BTreeSet::new();
            let standalone = package_preview(&a,&assets,&[],&roots,&empty)?;
            assert_eq!(standalone["scope"], "file");
            assert_eq!(standalone["assets"].as_array().unwrap().len(),1);
            assert_eq!(standalone["files"].as_array().unwrap().len(),2);
            assert_eq!(standalone["allowed_actions"],json!(["delete"]));
            let mut unfinished = t.clone(); unfinished["files"][1]["bytes_completed"] = json!(1);
            assert!(package_preview(&a,&assets,&[unfinished],&roots,&empty).is_err());
            let preview = package_preview(&a,&assets,&[t.clone()],&roots,&empty)?;
            assert_eq!(preview["assets"].as_array().unwrap().len(),2);
            assert_eq!(preview["files"].as_array().unwrap().len(),3);
            assert_eq!(preview["allowed_actions"],json!(["delete","junk"]));
            std::fs::write(root.join("one.en.mp4"),b"neighbor")?;
            assert_eq!(package_preview(&a,&assets,&[t.clone()],&roots,&empty)?["allowed_actions"],json!([]));
            assert!(verify_no_shared_files(&preview,&[t.clone()]).is_err());
            std::fs::remove_file(root.join("one.en.mp4"))?;
            verify_no_shared_files(&preview,&[t.clone()])?;
            let mut done = t.clone(); done["is_finished"] = json!(true);
            assert_eq!(package_preview(&a,&assets,&[done],&roots,&empty)?["allowed_actions"],json!(["delete"]));
            let mut active = BTreeSet::new(); active.insert("hash:1".into());
            assert_eq!(package_preview(&a,&assets,&[t.clone()],&roots,&active)?["allowed_actions"],json!([]));
            let mut other = t.clone(); other["hash_string"] = json!("other");
            assert_eq!(package_preview(&a,&assets,&[t.clone(),other],&roots,&empty)?["allowed_actions"],json!([]));
            std::fs::write(root.join("one.mp4"),b"changed")?;
            assert_ne!(preview["confirmation_token"],package_preview(&a,&assets,&[t],&roots,&empty)?["confirmation_token"]);
            std::os::unix::fs::symlink(root.join("one.mp4"),root.join("link.mp4"))?;
            assert!(file_record(&root.join("link.mp4"),&roots).is_err());
            assert!(file_record(&root,&roots).is_err());
            std::fs::hard_link(root.join("one.mp4"),root.join("shared.mp4"))?;
            assert!(file_record(&root.join("one.mp4"),&roots).is_err());
            Ok(())
        };
        let result = run(); std::fs::remove_dir_all(root)?; result
    }
}
