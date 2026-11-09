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

pub fn inventory(config: &Config, old: Vec<Value>, torrents: Vec<Value>) -> Inventory {
    let mut candidates: HashMap<PathBuf, Value> = HashMap::new();
    let mut errors = Vec::new();
    for torrent in torrents {
        let Some(root) = torrent["download_dir"].as_str() else {
            continue;
        };
        if let Some(files) = torrent["files"].as_array() {
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
                    "size":file["length"],"complete":file["length"].as_u64().unwrap_or(0)>0 && file["bytes_completed"]==file["length"],
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
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => continue,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    errors.push(format!("{}: {error}", path.display()));
                }
                continue;
            }
        };
        let inode = format!("{}:{}", metadata.dev(), metadata.ino());
        let name = path.to_string_lossy();
        let previous = info["torrent_hash"]
            .as_str()
            .zip(info["file_index"].as_u64())
            .and_then(|key| by_torrent.get(&key).copied())
            .or_else(|| by_path.get(name.as_ref()).copied())
            .or_else(|| by_inode.get(inode.as_str()).copied());
        let mut asset = previous.cloned().unwrap_or(
            json!({"id":format!("asset-{}",uuid::Uuid::new_v4().simple()),"status":"pending"}),
        );
        // Retain the old torrent identity for playback history even after removal.
        asset
            .as_object_mut()
            .unwrap()
            .extend(info.as_object().unwrap().clone());
        asset["path"] = json!(name);
        asset["inode"] = json!(inode);
        asset["available"] = json!(true);
        asset["seen_at"] = json!(now());
        if info["torrent_present"] != true || asset["size"].as_u64().unwrap_or(0) == 0 {
            asset["size"] = json!(metadata.len());
        }
        if let Some(previous) = previous {
            asset["added_at"] = previous["added_at"].clone();
        }
        if let Some(capture) = EPISODE.captures(&name) {
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
        if let Some(path) = asset["path"].as_str() {
            if matches!(fs::metadata(path),Err(e) if e.kind()==std::io::ErrorKind::NotFound) {
                asset["available"] = json!(false);
            }
        }
        assets.push(asset);
    }
    Inventory { assets, errors }
}
