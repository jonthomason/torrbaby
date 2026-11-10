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
    Regex::new(r"(?i)(?:^|[^a-z0-9])s(\d{1,2})[ ._-]*e(\d{1,3})(?:[^0-9]|$)|(?:^|[^0-9])(\d{1,2})x(\d{2})(?:[^0-9]|$)").unwrap()
});
/// Nonstandard episode names require explicit series-pack context and an episode title.
/// The release's numbers may be DVD order, so resolve by title against provider metadata.
pub fn named_episode(path: &Path) -> Option<(String, String, String)> {
    let stem = path.file_stem()?.to_str()?;
    if EPISODE.is_match(stem) { return None; }
    static MARKER: LazyLock<Regex> = LazyLock::new(||
        Regex::new(r"(?i)\s+-\s+(?:episode[ ._-]+\d+|pilot[ ._-]+film)(?:\s|$)").unwrap());
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+[ ._-]+").unwrap());
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
pub struct Inventory {
    pub assets: Vec<Value>,
    pub errors: Vec<String>,
}

fn package_complete(files: &[Value]) -> bool {
    !files.is_empty() && files.iter().all(|file| {
        file["length"].as_u64().is_some_and(|length|
            file["bytes_completed"].as_u64() == Some(length))
    })
}

pub fn inventory(config: &Config, old: Vec<Value>, torrents: Vec<Value>) -> Inventory {
    let mut candidates: HashMap<PathBuf, Value> = HashMap::new();
    let mut errors = Vec::new();
    for torrent in torrents {
        let Some(root) = torrent["download_dir"].as_str() else {
            continue;
        };
        if [&config.source,&config.movies,&config.tv].iter().any(|managed|Path::new(root).starts_with(Path::new(managed).join("Junk"))) { continue; }
        if let Some(files) = torrent["files"].as_array() {
            let package_complete = package_complete(files) && files.iter().all(|file| {
                let Some(name) = file["name"].as_str() else { return false; };
                let relative = Path::new(name);
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
                candidates.insert(path,json!({"torrent_hash":torrent["hash_string"],"torrent_present":true,"file_index":index,
                    "size":file["length"],"complete":package_complete && file["length"].as_u64().unwrap_or(0)>0 && file["bytes_completed"]==file["length"],
                    "added_at":torrent["added_date"],"torrent_name":torrent["name"]}));
            }
        }
    }
    for (root, complete) in [
        (&config.movies, true),
        (&config.tv, true),
        (&config.source, false),
    ] {
        let mut pending = vec![PathBuf::from(root)];
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
                    if entry.path() == Path::new(root).join("Junk") { continue; }
                    pending.push(entry.path());
                } else if kind.is_file() && video(&entry.path()) {
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
        let valid_root = [&config.source, &config.movies, &config.tv]
            .iter()
            .any(|root| {
                let Ok(relative) = path.strip_prefix(root) else {
                    return false;
                };
                let mut current = PathBuf::from(root);
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
        asset["library_ready"] = json!(path.starts_with(&config.movies) || path.starts_with(&config.tv));
        asset["inode"] = json!(inode);
        asset["available"] = json!(metadata.is_some());
        if metadata.is_none() { asset["complete"] = json!(false); }
        asset["seen_at"] = json!(now());
        if info["torrent_present"] != true || asset["size"].as_u64().unwrap_or(0) == 0 {
            asset["size"] = json!(metadata.as_ref().map(|m|m.len()).unwrap_or(0));
        }
        if let Some(previous) = previous {
            asset["added_at"] = previous["added_at"].clone();
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
    for mut asset in old {
        if seen.contains(asset["id"].as_str().unwrap_or("")) {
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
    Inventory { assets, errors }
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
}
