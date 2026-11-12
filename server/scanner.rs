use crate::{config::Config, storage::now};
use regex::Regex;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::LazyLock,
};

pub static EPISODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])s(\d{1,2})[ ._-]*e(\d{1,3})(?:[^0-9]|$)|(?:^|[^0-9])(\d{1,2})x(\d{2})(?:[^0-9]|$)").expect("valid regex")
});
/// Nonstandard episode names require explicit series-pack context and an episode title.
/// The release's numbers may be DVD order, so resolve by title against provider metadata.
pub fn named_episode(path: &Path) -> Option<(String, String, String)> {
    let stem = path.file_stem()?.to_str()?;
    if EPISODE.is_match(stem) { return None; }
    static MARKER: LazyLock<Regex> = LazyLock::new(||
        Regex::new(r"(?i)\s+-\s+(?:episode[ ._-]+\d+|pilot[ ._-]+film)(?:\s|$)").expect("valid regex"));
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+[ ._-]+").expect("valid regex"));
    let marker = MARKER.find(stem)?;
    let prefix = NUMBER.replace(&stem[..marker.start()], "");
    for folder in path.ancestors().skip(1).take(3) {
        let Some(query) = folder.file_name().and_then(|n| n.to_str()) else { continue; };
        if !crate::metadata::SERIES_PACK.is_match(query) { continue; }
        let (series, _, _) = crate::metadata::parse_name(query);
        if series.is_empty() { continue; }
        let Some(prefix_head) = prefix.get(..series.len()) else { continue; };
        if !prefix_head.eq_ignore_ascii_case(&series) { continue; }
        let rest = &prefix[series.len()..];
        if !rest.starts_with([' ', '.', '_', '-']) { continue; }
        let title = rest.trim_matches([' ', '.', '_', '-']);
        if !title.is_empty() { return Some((series, query.to_owned(), title.to_owned())); }
    }
    None
}

/// Do not infer provider episode numbers from release-pack numbering.
pub fn resolve_episode_title(asset: &mut Value, entity: &Value) {
    if entity["kind"] != "tv" { return; }
    let Some(title) = asset["episode_title"].as_str() else { return; };
    let normalize = |s: &str| s.chars().filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase).collect::<String>();
    let matches: Vec<_> = entity["episodes"].as_array().into_iter().flatten()
        .filter(|e| e["title"].as_str().is_some_and(|s| normalize(s) == normalize(title))).collect();
    if matches.len() == 1 {
        asset["season"] = matches[0]["season"].clone();
        asset["episode"] = matches[0]["episode"].clone();
    }
}

pub fn video(path: &Path) -> bool {
    path.extension()
        .and_then(|x| x.to_str())
        .map(|x| {
            [
                "mp4", "m4v", "mkv", "mov", "avi", "webm", "ts", "m2ts", "mpg", "mpeg", "wmv",
            ]
            .contains(&x.to_ascii_lowercase().as_str())
        })
        .unwrap_or(false)
}
/// Record the sniffed container of a complete file. Packs ship adverts named
/// .mkv/.mp4, which `video()` admits by extension; a "not_media" tag hides them
/// from the library and processing. Incomplete files are never judged (their head
/// may not be written yet), and an unchanged file is not read again.
fn tag_container(asset: &mut Value, path: &Path, metadata: Option<&fs::Metadata>) {
    let key = metadata.filter(|_| asset["complete"] == true)
        .map(|m| format!("{}:{}:{}:{}", m.ino(), m.len(), m.mtime(), m.mtime_nsec()));
    if key.is_some() && asset["container_key"].as_str() == key.as_deref() && asset["container"].is_string() {
        return;
    }
    let sniffed = key.and_then(|key| crate::container::sniff(path).ok().map(|c| (c, key)));
    let Some(object) = asset.as_object_mut() else { return; };
    match sniffed {
        Some((container, key)) => {
            if container == crate::container::Container::NotMedia {
                tracing::info!(path=%path.display(), "Ignoring a file whose content is not video");
            }
            object.insert("container".into(), json!(container.name()));
            object.insert("container_key".into(), json!(key));
        }
        None => {
            object.remove("container");
            object.remove("container_key");
        }
    }
}
pub struct Inventory {
    pub assets: Vec<Value>,
    pub errors: Vec<String>,
    /// Untracked records of in-progress torrent files (see `torrent_copy`).
    pub superseded: Vec<String>,
}

/// Transmission keeps unfinished files in its incomplete directory (on Diamond,
/// `Downloads/Incomplete/<torrent files>`) and moves them to the download directory on
/// completion. Such a file is a torrent's file, tracked by torrent hash and file index;
/// a separate record would duplicate it in matching and review, then linger as
/// unavailable after the move. Recognised by the incomplete directory Transmission
/// reports, or, without one, as a file under the source root at another location whose
/// trailing path is a present torrent's file name.
fn torrent_copy(path: &Path, source: &Path, incomplete: Option<&Path>, names: &HashSet<PathBuf>, tracked: &HashSet<PathBuf>) -> bool {
    if tracked.contains(path) { return false; }
    if incomplete.is_some_and(|incomplete| path.starts_with(incomplete)) { return true; }
    let Ok(relative) = path.strip_prefix(source) else { return false; };
    let parts: Vec<_> = relative.components().collect();
    (0..parts.len()).any(|start| names.contains(&parts[start..].iter().collect::<PathBuf>()))
}

/// Whether a subtitle file belonging to this video (same folder, same name, as
/// `subtitles::discover` pairs them) is still downloading.
fn awaiting_sidecar(video: &Path, files: &[Value]) -> bool {
    let Some(stem) = video.file_stem().and_then(|s| s.to_str()) else { return false };
    files.iter().any(|file| {
        let Some(other) = file["name"].as_str().map(Path::new) else { return false };
        other.parent() == video.parent()
            && other.extension().is_some_and(|e| e.eq_ignore_ascii_case("srt"))
            && other.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s == stem || s.starts_with(&format!("{stem}.")))
            && file["bytes_completed"] != file["length"]
    })
}
/// Where a file can be read for preparation: its library path once complete, or
/// its finished copy in a package that is still downloading.
pub fn work_path(asset: &Value) -> Option<PathBuf> {
    if asset["complete"] == true { asset["path"].as_str().map(PathBuf::from) }
    else { asset["early_path"].as_str().map(PathBuf::from) }
}

fn package_complete(files: &[Value]) -> bool {
    !files.is_empty() && files.iter().all(|file| {
        file["length"].as_u64().is_some_and(|length|
            file["bytes_completed"].as_u64() == Some(length))
    })
}

pub fn inventory(config: &Config, old: Vec<Value>, torrents: Vec<Value>) -> Inventory {
    inventory_with(config, old, torrents, None)
}

/// `incomplete` is Transmission's incomplete directory, when enabled and known.
pub fn inventory_with(config: &Config, old: Vec<Value>, torrents: Vec<Value>, incomplete: Option<&Path>) -> Inventory {
    // A misconfigured incomplete directory must never hide a library root.
    let drives = crate::drives::Drives::of(config);
    let incomplete = incomplete.filter(|dir| dir.is_absolute() && dir.components().count() > 2
        && !drives.all_roots().any(|(_, _, root)| root.starts_with(dir)));
    let mut candidates: HashMap<PathBuf, Value> = HashMap::new();
    let mut errors = Vec::new();
    // Present torrents' video file names, those not yet at their final path, and those paths.
    let (mut torrent_names, mut unfinished_names, mut tracked) = (HashSet::new(), HashSet::new(), HashSet::new());
    for torrent in torrents {
        let Some(root) = torrent["download_dir"].as_str() else {
            continue;
        };
        if crate::trials::trial_path(root) {continue;}
        if drives.all_roots().any(|(_, _, managed)|Path::new(root).starts_with(managed.join("Junk"))) { continue; }
        if let Some(files) = torrent["files"].as_array() {
            let package_complete = package_complete(files) && files.iter().all(|file| {
                let Some(name) = file["name"].as_str() else { return false; };
                let relative = Path::new(name);
                // A sidecar renamed or removed after download (a subtitle given its language,
                // say) must not hold the videos back; Transmission already has every byte.
                if !video(relative) { return true; }
                if !relative.components().all(|p|matches!(p,std::path::Component::Normal(_))) { return false; }
                let mut full = PathBuf::from(root);
                for part in relative.components() {
                    full.push(part);
                    if full.is_symlink() { return false; }
                }
                fs::metadata(full).is_ok_and(|m| m.is_file() && Some(m.len()) == file["length"].as_u64())
            });
            for (index, file) in files.iter().enumerate() {
                let Some(name) = file["name"].as_str() else {
                    continue;
                };
                if Path::new(name).is_absolute()
                    || Path::new(name)
                        .components()
                        .any(|p| !matches!(p, std::path::Component::Normal(_)))
                {
                    continue;
                }
                let path = Path::new(root).join(name);
                if !video(&path) {
                    continue;
                }
                torrent_names.insert(PathBuf::from(name));
                if !path.is_file() { unfinished_names.insert(PathBuf::from(name)); }
                tracked.insert(path.clone());
                // A finished file of a package still downloading can be prepared where
                // it sits, unless one of its own subtitle files has yet to arrive.
                let finished = file["length"].as_u64().unwrap_or(0)>0 && file["bytes_completed"]==file["length"];
                let early = (!package_complete && finished && !awaiting_sidecar(Path::new(name), files)).then(|| {
                    [Some(Path::new(root)), incomplete].into_iter().flatten().map(|base| base.join(name))
                        .find(|at| fs::symlink_metadata(at).is_ok_and(|m| m.is_file() && Some(m.len()) == file["length"].as_u64()))
                }).flatten();
                candidates.insert(path,json!({"torrent_hash":torrent["hash_string"],"torrent_present":true,"file_index":index,
                    "early_path":early,
                    "size":file["length"],"complete":package_complete && finished,
                    "added_at":torrent["added_date"],"done_at":torrent["done_date"],"torrent_name":torrent["name"]}));
            }
        }
    }
    // Library roots first, then Downloads, drive by drive.
    use crate::drives::Role;
    let walk: Vec<(&Path, Role)> = [Role::Movies, Role::Tv, Role::Downloads].into_iter()
        .flat_map(|role| drives.roots(role).map(move |root| (root, role))).collect();
    for (root, role) in walk {
        let complete = role != Role::Downloads;
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            let entries = match fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(error) => {
                    errors.push(format!("{}: {error}", directory.display()));
                    continue;
                }
            };
            for result in entries {
                let entry = match result {
                    Ok(entry) => entry,
                    Err(error) => {
                        errors.push(error.to_string());
                        continue;
                    }
                };
                let kind = match entry.file_type() {
                    Ok(kind) => kind,
                    Err(error) => {
                        errors.push(error.to_string());
                        continue;
                    }
                };
                if kind.is_symlink() {
                    continue;
                }
                if kind.is_dir() {
                    if entry.file_name()==".torrbaby-trials" {continue;}
                    if entry.path() == root.join("Junk") { continue; }
                    pending.push(entry.path());
                } else if kind.is_file() && video(&entry.path())
                    && !(role == Role::Downloads && torrent_copy(&entry.path(), root, incomplete, &unfinished_names, &tracked)) {
                    candidates.entry(entry.path()).or_insert_with(
                        || json!({"complete":complete,"added_at":now(),"torrent_present":false}),
                    );
                }
            }
        }
    }
    let by_path: HashMap<_, _> = old
        .iter()
        .filter_map(|a| a["path"].as_str().map(|p| (p, a)))
        .collect();
    let by_inode: HashMap<_, _> = old
        .iter()
        .filter_map(|a| a["inode"].as_str().map(|p| (p, a)))
        .collect();
    let by_torrent: HashMap<_, _> = old
        .iter()
        .filter_map(|a| Some(((a["torrent_hash"].as_str()?, a["file_index"].as_u64()?), a)))
        .collect();
    let mut assets = Vec::new();
    let mut seen = HashSet::new();
    for (path, info) in candidates {
        let valid_root = drives.all_roots()
            .any(|(_, _, root)| {
                let Ok(relative) = path.strip_prefix(root) else {
                    return false;
                };
                let mut current = root.to_path_buf();
                relative.components().all(|part| {
                    current.push(part);
                    !current.is_symlink()
                })
            });
        if !valid_root {
            continue;
        }
        let metadata = match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => Some(metadata),
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && info["torrent_present"] == true => None,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    errors.push(format!("{}: {error}", path.display()));
                }
                continue;
            }
        };
        let inode = metadata.as_ref().map(|m| format!("{}:{}", m.dev(), m.ino()));
        let name = path.to_string_lossy();
        let previous = info["torrent_hash"]
            .as_str()
            .zip(info["file_index"].as_u64())
            .and_then(|key| by_torrent.get(&key).copied())
            .or_else(|| by_path.get(name.as_ref()).copied())
            .or_else(|| inode.as_ref().and_then(|inode| by_inode.get(inode.as_str()).copied()));
        let mut asset = previous.cloned().unwrap_or(
            json!({"id":format!("asset-{}",uuid::Uuid::new_v4().simple()),"status":"pending"}),
        );
        // Retain the old torrent identity for playback history even after removal.
        asset
            .as_object_mut()
            .unwrap()
            .extend(info.as_object().unwrap().clone());
        // A filesystem walk cannot certify completion of a known incomplete package
        // when Transmission is unavailable (or the download was removed).
        if info["torrent_present"] != true && previous.is_some_and(|a|a["torrent_hash"].is_string() && a["complete"] != true) {
            asset["complete"] = json!(false);
        }
        asset["path"] = json!(name);
        asset["library_ready"] = json!(drives.in_library(&path));
        asset["inode"] = json!(inode);
        asset["available"] = json!(metadata.is_some());
        if metadata.is_none() { asset["complete"] = json!(false); }
        tag_container(&mut asset, &path, metadata.as_ref());
        asset["seen_at"] = json!(now());
        if info["torrent_present"] != true || asset["size"].as_u64().unwrap_or(0) == 0 {
            asset["size"] = json!(metadata.as_ref().map(|m|m.len()).unwrap_or(0));
        }
        if let Some(previous) = previous {
            asset["added_at"] = previous["added_at"].clone();
        }
        // "Recently added" follows when a file became watchable, not when its torrent was
        // queued. Files that were already in the library take their torrent's finish time.
        let ready = |a: &Value| a["complete"] == true && a["available"] == true && a["library_ready"] == true;
        if ready(&asset) && !asset["ready_at"].as_f64().is_some_and(|at| at > 0.0) {
            let finished = info["done_at"].as_f64().filter(|at| *at > 0.0);
            let known = previous.is_none_or(ready);
            asset["ready_at"] = json!(match finished {
                Some(at) if known => at.max(asset["added_at"].as_f64().unwrap_or(0.0)),
                _ if previous.is_some_and(ready) => asset["added_at"].as_f64().unwrap_or_else(now),
                _ => now(),
            });
        }
        if asset["confirmed"] != true {
            if let Some((series, query, title)) = named_episode(&path) {
                asset["series_name"] = json!(series);
                asset["series_query"] = json!(query);
                asset["episode_title"] = json!(title);
            }
        }
        if let Some(capture) = EPISODE.captures(&name).filter(|_| asset["confirmed"] != true) {
            let season = capture
                .get(1)
                .or(capture.get(3))
                .and_then(|m| m.as_str().parse::<u32>().ok());
            let episode = capture
                .get(2)
                .or(capture.get(4))
                .and_then(|m| m.as_str().parse::<u32>().ok());
            asset["season"] = json!(season);
            asset["episode"] = json!(episode);
        }
        let extra = path
            .components()
            .filter_map(|p| p.as_os_str().to_str())
            .any(|p| {
                [
                    "extras",
                    "featurettes",
                    "samples",
                    "sample",
                    "trailers",
                    "bonus",
                ]
                .contains(&p.to_ascii_lowercase().as_str())
            });
        asset["role"] = json!(if extra { "extra" } else { "main" });
        if let Some(id) = asset["id"].as_str() {
            seen.insert(id.to_owned());
        }
        assets.push(asset);
    }
    let mut superseded = Vec::new();
    for mut asset in old {
        if seen.contains(asset["id"].as_str().unwrap_or("")) {
            continue;
        }
        // An explicit user match is kept even on a duplicate record. Once a torrent's file
        // is in place, only a record whose own file has gone can be its earlier copy.
        if asset["torrent_hash"].is_null() && asset["confirmed"] != true
            && asset["path"].as_str().is_some_and(|path| {
                let gone = matches!(fs::metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound);
                drives.roots(Role::Downloads).any(|downloads| torrent_copy(Path::new(path), downloads, incomplete,
                    if gone { &torrent_names } else { &unfinished_names }, &tracked))
            })
        {
            if let Some(id) = asset["id"].as_str() { superseded.push(id.to_owned()); }
            continue;
        }
        asset["torrent_present"] = json!(false);
        if let Some(path) = asset["path"].as_str() {
            if matches!(fs::metadata(path),Err(e) if e.kind()==std::io::ErrorKind::NotFound) {
                asset["available"] = json!(false);
            }
        }
        assets.push(asset);
    }
    Inventory { assets, errors, superseded }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_package_keeps_identity_and_matches_through_completion_and_move() {
        let root = std::env::temp_dir().join(format!("torrbaby-pending-{}",uuid::Uuid::new_v4()));
        let config: Config = serde_json::from_value(json!({
            "listen_host":"127.0.0.1","listen_port":8765,"token_file":"unused","state_file":"unused",
            "transmission_url":"http://127.0.0.1:9091","transmission_auth_file":null,
            "source":root.join("Downloads"),"movies":root.join("Movies"),"tv":root.join("TV"),"poll_seconds":60
        })).unwrap();
        for directory in [&config.source,&config.movies,&config.tv] { fs::create_dir_all(directory).unwrap(); }
        let mut torrent = json!({"hash_string":"pending","download_dir":config.source,"name":"Show S01",
            "files":[{"name":"Show S01E01.mkv","length":100,"bytes_completed":0},
                     {"name":"Show S01E02.mkv","length":100,"bytes_completed":0}]});
        let mut pending = inventory(&config,vec![],vec![torrent.clone()]).assets;
        assert_eq!(pending.len(),2);
        for asset in &mut pending {
            assert_eq!(asset["available"],false);
            assert_eq!(asset["complete"],false);
            assert_eq!(asset["season"],1);
            asset["entity_id"] = json!("show");
            asset["confirmed"] = json!(true);
        }
        let ids: HashSet<_> = pending.iter().map(|a|a["id"].as_str().unwrap().to_owned()).collect();
        fs::write(Path::new(&config.source).join("Show S01E01.mkv"),vec![0;100]).unwrap();
        torrent["files"][0]["bytes_completed"] = json!(100);
        let partial = inventory(&config,pending,vec![torrent.clone()]).assets;
        assert!(partial.iter().all(|a|a["complete"] == false));
        assert_eq!(partial.iter().filter(|a|a["available"] == true).count(),1);
        fs::write(Path::new(&config.source).join("Show S01E02.mkv"),vec![0;100]).unwrap();
        torrent["files"][1]["bytes_completed"] = json!(100);
        let complete = inventory(&config,partial,vec![torrent.clone()]).assets;
        assert!(complete.iter().all(|a|a["complete"] == true && a["available"] == true));
        for name in ["Show S01E01.mkv","Show S01E02.mkv"] {
            fs::rename(Path::new(&config.source).join(name),Path::new(&config.tv).join(name)).unwrap();
        }
        torrent["download_dir"] = json!(config.tv);
        let moved = inventory(&config,complete,vec![torrent.clone()]).assets;
        assert_eq!(moved.len(),2);
        assert!(moved.iter().all(|a|ids.contains(a["id"].as_str().unwrap()) && a["entity_id"] == "show" && a["confirmed"] == true));
        // RPC completion alone cannot publish a package with a missing file.
        fs::remove_file(Path::new(&config.tv).join("Show S01E02.mkv")).unwrap();
        let missing = inventory(&config,moved,vec![torrent]).assets;
        assert!(missing.iter().all(|a|a["complete"] == false));
        let rpc_unavailable = inventory(&config,missing,vec![]).assets;
        assert!(rpc_unavailable.iter().all(|a|a["complete"] == false));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn complete_files_are_tagged_by_content_and_adverts_are_not_media() {
        let root = std::env::temp_dir().join(format!("torrbaby-sniff-{}",uuid::Uuid::new_v4()));
        let config: Config = serde_json::from_value(json!({
            "listen_host":"127.0.0.1","listen_port":8765,"token_file":"unused","state_file":"unused",
            "transmission_url":"http://127.0.0.1:9091","transmission_auth_file":null,
            "source":root.join("Downloads"),"movies":root.join("Movies"),"tv":root.join("TV"),"poll_seconds":60
        })).unwrap();
        for directory in [&config.source,&config.movies,&config.tv] { fs::create_dir_all(directory).unwrap(); }
        let movies = Path::new(&config.movies);
        let mut matroska = vec![0x1A,0x45,0xDF,0xA3,0xA3,0x42,0x86,0x81,0x01];
        matroska.resize(1024,0x5A);
        fs::write(movies.join("Dr.No.1962.mp4"),&matroska).unwrap();
        // A real Matroska file whose body is damaged is still media and still fails visibly.
        let mut damaged = matroska.clone();
        damaged[600..].fill(0xFF);
        fs::write(movies.join("Damaged.1999.mkv"),&damaged).unwrap();
        fs::write(movies.join("Advert.MKV"),b"\x10\x00\x00\x00q4l\x0f\xbb\x7c\xd5\x01 www.example.invalid advert").unwrap();
        let assets = inventory(&config,vec![],vec![]).assets;
        let tag = |name:&str| assets.iter().find(|a|a["path"].as_str().unwrap().ends_with(name)).unwrap()["container"].clone();
        assert_eq!(tag("Dr.No.1962.mp4"),"matroska");
        assert_eq!(tag("Damaged.1999.mkv"),"matroska");
        assert_eq!(tag("Advert.MKV"),"not_media");
        assert!(assets.iter().all(|a|a["available"] == true && a["container_key"].is_string()));
        // Unchanged files keep their tag; a replaced file is judged again.
        fs::write(movies.join("Advert.MKV"),&matroska).unwrap();
        let rescanned = inventory(&config,assets,vec![]).assets;
        assert!(rescanned.iter().all(|a|a["container"] == "matroska"));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn recency_follows_when_a_file_became_watchable() {
        let root = std::env::temp_dir().join(format!("torrbaby-ready-{}",uuid::Uuid::new_v4()));
        let config: Config = serde_json::from_value(json!({
            "listen_host":"127.0.0.1","listen_port":8765,"token_file":"unused","state_file":"unused",
            "transmission_url":"http://127.0.0.1:9091","transmission_auth_file":null,
            "source":root.join("Downloads"),"movies":root.join("Movies"),"tv":root.join("TV"),"poll_seconds":60
        })).unwrap();
        for directory in [&config.source,&config.movies,&config.tv] { fs::create_dir_all(directory).unwrap(); }
        let torrent = |hash:&str,name:&str,done:u64,bytes:u64| json!({"hash_string":hash,"download_dir":config.movies,"name":name,
            "added_date":1000,"done_date":done,"files":[{"name":name,"length":100,"bytes_completed":bytes}]});
        let of = |assets:&[Value],hash:&str| assets.iter().find(|a|a["torrent_hash"]==hash).cloned().unwrap();
        for name in ["Old.mkv","Slow.mkv"] { fs::write(Path::new(&config.movies).join(name),vec![0u8;100]).unwrap(); }
        // Already in the library before this field existed: dated by the torrent's finish time.
        let mut legacy = inventory(&config,vec![],vec![torrent("old","Old.mkv",5000,100)]).assets;
        for asset in &mut legacy { asset.as_object_mut().unwrap().remove("ready_at"); }
        let backfilled = inventory(&config,legacy,vec![torrent("old","Old.mkv",5000,100)]).assets;
        assert_eq!(of(&backfilled,"old")["ready_at"],json!(5000.0));
        // Queued long ago, finished just now: dated by the moment it became watchable.
        let waiting = inventory(&config,vec![],vec![torrent("slow","Slow.mkv",0,50)]).assets;
        assert!(of(&waiting,"slow")["ready_at"].is_null());
        let before = now();
        let finished = inventory(&config,waiting,vec![torrent("slow","Slow.mkv",0,100)]).assets;
        let stamped = of(&finished,"slow")["ready_at"].as_f64().unwrap();
        assert!(stamped >= before);
        // Later scans keep the date.
        let again = inventory(&config,finished,vec![torrent("slow","Slow.mkv",9_999_999_999,100)]).assets;
        assert_eq!(of(&again,"slow")["ready_at"].as_f64().unwrap(),stamped);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn finished_episode_waits_for_entire_package() {
        let done = json!({"length":100,"bytes_completed":100});
        let partial = json!({"length":100,"bytes_completed":99});
        assert!(!package_complete(&[done.clone(), partial]));
        assert!(package_complete(&[done.clone(), done]));
        assert!(!package_complete(&[]));
        assert!(!package_complete(&[json!({})]));
    }
    #[test]
    fn complete_series_is_tv_but_movie_collections_stay_movies() {
        let parse = crate::metadata::parse_name;
        assert_eq!(parse("Firefly Complete Series - Sci-Fi 2002-2003 Eng Subs 1080p [H264-mp4]"),
            ("Firefly".into(), "2002".into(), "tv"));
        assert_eq!(parse("Firefly.Complete.TV.Series.2002.1080p").2, "tv");
        assert_eq!(parse("Serenity (2005) 1080p").2, "movie");
        assert_eq!(parse("Alien Complete Collection 1979").2, "movie");
        assert_eq!(parse("The Complete Works 2001").2, "movie");
    }
    #[test]
    fn named_pack_episodes_use_titles_not_release_numbers() {
        let root = "/Movies/Firefly Complete Series 2002-2003/";
        for (file, expected) in [
            ("00 Firefly Serenity - Pilot Film 2002.mp4", "Serenity"),
            ("03 Firefly Shindig - Episode 03 2002.mp4", "Shindig"),
            ("05 Firefly Our Mrs Reynolds - Episode 05 2002.mp4", "Our Mrs Reynolds"),
            ("10 Firefly Trash - Episode 10 2003.mp4", "Trash"),
        ] {
            let (series, query, title) = named_episode(Path::new(&format!("{root}{file}"))).unwrap();
            assert_eq!(series, "Firefly");
            assert_eq!(crate::metadata::parse_name(&query).2, "tv");
            assert_eq!(title, expected);
        }
        assert!(named_episode(Path::new("/Movies/Serenity (2005).mp4")).is_none());
        assert!(named_episode(Path::new(&format!("{root}Serenity (2005).mp4"))).is_none());
        assert!(named_episode(Path::new("/Movies/Collection/01 Firefly Trash - Episode 10.mp4")).is_none());
        assert!(named_episode(Path::new(&format!("{root}Firefly S01E11 Serenity.mkv"))).is_none());
    }
    #[test]
    fn provider_order_and_specials_are_resolved_without_guessing() {
        let entity = json!({"kind":"tv","episodes":[
            {"title":"Serenity","season":1,"episode":11},
            {"title":"Shindig","season":1,"episode":6},
            {"title":"Our Mrs. Reynolds","season":1,"episode":3},
            {"title":"Trash","season":0,"episode":1}
        ]});
        for (title, season, episode) in [("Serenity",1,11),("Shindig",1,6),("Our Mrs Reynolds",1,3),("Trash",0,1)] {
            let mut a = json!({"episode_title":title});
            resolve_episode_title(&mut a,&entity);
            assert_eq!(a["season"],season);
            assert_eq!(a["episode"],episode);
        }
        for episodes in [json!([]),json!([{"title":"Serenity","season":1,"episode":1},{"title":"Serenity","season":2,"episode":1}])] {
            let mut a = json!({"episode_title":"Serenity"});
            resolve_episode_title(&mut a,&json!({"kind":"tv","episodes":episodes}));
            assert!(a.get("episode").is_none());
        }
    }
    #[test]
    fn incomplete_directory_copies_are_not_separate_assets() {
        let root = std::env::temp_dir().join(format!("torrbaby-incomplete-{}",uuid::Uuid::new_v4()));
        let config: Config = serde_json::from_value(json!({
            "listen_host":"127.0.0.1","listen_port":8765,"token_file":"unused","state_file":"unused",
            "transmission_url":"http://127.0.0.1:9091","transmission_auth_file":null,
            "source":root.join("Downloads"),"movies":root.join("Movies"),"tv":root.join("TV"),"poll_seconds":60
        })).unwrap();
        for directory in [&config.source,&config.movies,&config.tv] { fs::create_dir_all(directory).unwrap(); }
        let source = Path::new(&config.source);
        let name = "007.Set/11.Moonraker.1979/007.James.Bond.Moonraker.1979.mp4";
        let incomplete = source.join("Incomplete").join(name);
        fs::create_dir_all(incomplete.parent().unwrap()).unwrap();
        fs::write(&incomplete, vec![0;100]).unwrap();
        // An unrelated file that merely shares the film folder name stays a separate asset.
        fs::create_dir_all(source.join("Other/11.Moonraker.1979")).unwrap();
        fs::write(source.join("Other/11.Moonraker.1979/Moonraker.mp4"), vec![0;100]).unwrap();
        let mut torrent = json!({"hash_string":"bond","download_dir":config.source,"name":"007.Set",
            "files":[{"name":name,"length":100,"bytes_completed":100},{"name":"007.Set/other.mp4","length":100,"bytes_completed":0}]});
        // While downloading, the finished file sits in Transmission's incomplete directory.
        let downloading = inventory(&config,vec![],vec![torrent.clone()]);
        assert!(downloading.superseded.is_empty());
        assert_eq!(downloading.assets.len(),3);
        assert!(!downloading.assets.iter().any(|a|a["path"].as_str().unwrap().contains("/Incomplete/")));
        let tracked = downloading.assets.iter().find(|a|a["file_index"]==0).unwrap().clone();
        assert_eq!((tracked["available"].clone(),tracked["complete"].clone()),(json!(false),json!(false)));
        // The finished file can be prepared where it sits, once its own subtitles are down.
        assert!(tracked["early_path"].is_null(), "unknown without the incomplete directory");
        let early = |files: Value| {
            let mut torrent = torrent.clone(); torrent["files"] = files;
            let found = inventory_with(&config,vec![],vec![torrent],Some(&source.join("Incomplete")));
            found.assets.iter().find(|a|a["file_index"]==0).unwrap().clone()
        };
        let other = json!({"name":"007.Set/other.mp4","length":100,"bytes_completed":0});
        let ready = early(json!([{"name":name,"length":100,"bytes_completed":100},other]));
        assert_eq!(work_path(&ready), Some(incomplete.clone()));
        assert_eq!((ready["available"].clone(),ready["complete"].clone()),(json!(false),json!(false)));
        let subtitle = |file: &str, done: u64| json!({"name":format!("007.Set/11.Moonraker.1979/{file}"),"length":10,"bytes_completed":done});
        let waiting = early(json!([{"name":name,"length":100,"bytes_completed":100},other,subtitle("007.James.Bond.Moonraker.1979.en.srt",3)]));
        assert_eq!(work_path(&waiting), None, "its subtitle file is still downloading");
        let arrived = early(json!([{"name":name,"length":100,"bytes_completed":100},other,subtitle("007.James.Bond.Moonraker.1979.en.srt",10),subtitle("Another.Film.srt",0)]));
        assert_eq!(work_path(&arrived), Some(incomplete.clone()), "another video's subtitles do not hold it back");
        let partial = early(json!([{"name":name,"length":100,"bytes_completed":60},other]));
        assert_eq!(work_path(&partial), None);
        // Records left by earlier builds: an automatic match and an explicit one.
        let stale = json!({"id":"asset-stale","path":incomplete,"status":"review","available":true,"complete":false,"role":"main"});
        let chosen = json!({"id":"asset-chosen","path":incomplete,"status":"matched","confirmed":true,"entity_id":"tmdb:movie:698","available":true,"complete":false,"role":"main"});
        // A torrent since renamed or removed: only Transmission's incomplete directory identifies it.
        let renamed = json!({"id":"asset-renamed","path":source.join("Incomplete/Show/Show.S01E01.mkv"),"status":"review","available":false,"complete":false,"role":"main"});
        let mut old = downloading.assets.clone();
        old.extend([stale, chosen, renamed]);
        // Transmission moves the finished package out of the incomplete directory.
        fs::create_dir_all(source.join("007.Set/11.Moonraker.1979")).unwrap();
        fs::rename(&incomplete, source.join(name)).unwrap();
        fs::write(source.join("007.Set/other.mp4"), vec![0;100]).unwrap();
        torrent["files"][1]["bytes_completed"] = json!(100);
        let complete = inventory(&config,old.clone(),vec![torrent.clone()]);
        assert_eq!(complete.superseded,["asset-stale"]);
        let moonraker = complete.assets.iter().find(|a|a["file_index"]==0).unwrap();
        assert_eq!(moonraker["id"],tracked["id"]);
        assert_eq!((moonraker["available"].clone(),moonraker["complete"].clone()),(json!(true),json!(true)));
        assert!(complete.assets.iter().any(|a|a["id"]=="asset-chosen" && a["available"]==false));
        assert!(complete.assets.iter().any(|a|a["path"].as_str().unwrap().ends_with("Other/11.Moonraker.1979/Moonraker.mp4")));
        assert_eq!(complete.assets.len(),5);
        let with_directory = inventory_with(&config,old,vec![torrent],Some(&source.join("Incomplete")));
        assert_eq!(with_directory.superseded,["asset-stale","asset-renamed"]);
        assert_eq!(with_directory.assets.len(),4);
        // A deliberate copy of a finished torrent's file elsewhere stays its own asset.
        fs::write(source.join("Single.mkv"), vec![0;100]).unwrap();
        fs::create_dir_all(source.join("Copies")).unwrap();
        fs::write(source.join("Copies/Single.mkv"), vec![0;100]).unwrap();
        let single = json!({"hash_string":"single","download_dir":config.source,"name":"Single.mkv","files":[{"name":"Single.mkv","length":100,"bytes_completed":100}]});
        let copied = inventory(&config,vec![],vec![single.clone()]);
        assert!(copied.assets.iter().any(|a|a["path"].as_str().unwrap().ends_with("Copies/Single.mkv") && a["torrent_hash"].is_null()));
        let copy = copied.assets.iter().find(|a|a["torrent_hash"].is_null() && a["path"].as_str().unwrap().ends_with("Copies/Single.mkv")).unwrap().clone();
        assert!(inventory(&config,vec![copy],vec![single]).superseded.is_empty());
        // An incomplete directory that would contain a library root is ignored.
        let stale = json!({"id":"asset-movie","path":Path::new(&config.movies).join("Film.mkv"),"available":false,"role":"main"});
        assert!(inventory_with(&config,vec![stale],vec![],Some(&root)).superseded.is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
