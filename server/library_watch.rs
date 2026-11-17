//! The library is scanned when its files change, not on a timer. macOS reports changes under
//! every drive's library folders (FSEvents, through the notify crate); a scan follows once they
//! have settled for a few seconds, at most once a minute while a download keeps writing. The
//! scan used to walk every folder and write every file's record each minute, all day (the user,
//! 2026-11-17: "it's going to wear out our SSDs").
//!
//! A torrent still downloading writes all the time, so changes inside it are not library changes
//! (they made the whole library be scanned every minute while anything downloaded). The 5 s
//! transfer sampler says where downloads are writing (`downloading`), and asks for a scan when a
//! torrent, or a file inside one, finishes (`completed`): Transmission knows that, the disk does not.
//!
//! The folders watched are worked out again each minute (an in-memory comparison: a drive
//! connected or removed, a folder made in Settings), and a change of them scans at once. Torrbaby's
//! own operations (a cloud download landing, files removed) still ask for a scan themselves, and
//! a full scan every 6 hours catches anything the file system did not report.
use crate::api::Service;
use notify::{RecursiveMode, Watcher};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Quiet this long after the last change before scanning.
const SETTLE: Duration = Duration::from_secs(3);
/// At most one scan this often while changes keep coming (a download writing).
const MIN_INTERVAL: Duration = Duration::from_secs(60);
/// A full scan this often regardless, for anything not reported.
const SAFETY: Duration = Duration::from_secs(6 * 3600);

/// Where torrents still downloading write: each one's folder or file, in its download folder
/// and in Transmission's folder for downloads in progress.
static DOWNLOADING: std::sync::RwLock<Vec<PathBuf>> = std::sync::RwLock::new(Vec::new());
/// What the sampler last saw finished: each torrent's count of complete files.
static FINISHED: std::sync::Mutex<Option<std::collections::BTreeMap<String, usize>>> = std::sync::Mutex::new(None);
static REQUESTED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// From each transfer sample: where downloads write, and a scan when something finished.
pub fn sampled(torrents: &[serde_json::Value], incomplete: Option<&std::path::Path>) {
    let mut writing = Vec::new();
    let mut finished = std::collections::BTreeMap::new();
    for t in torrents {
        let (Some(hash), Some(name)) = (t["hash_string"].as_str(), t["name"].as_str()) else { continue };
        let files = t["files"].as_array().map(|f| f.iter().filter(|f| f["length"].as_u64().is_some_and(|l| f["bytes_completed"].as_u64() == Some(l))).count()).unwrap_or(0);
        finished.insert(hash.to_owned(), files + usize::from(t["left_until_done"].as_u64() == Some(0)));
        if t["left_until_done"].as_u64().unwrap_or(0) == 0 || name.is_empty() || name.contains('/') { continue; }
        if let Some(dir) = t["download_dir"].as_str() { writing.push(PathBuf::from(dir).join(name)); }
        if let Some(dir) = incomplete { writing.push(dir.join(name)); }
    }
    if let Ok(mut d) = DOWNLOADING.write() { *d = writing; }
    let Ok(mut last) = FINISHED.lock() else { return };
    // A torrent removed is a change the file system reports itself.
    let more = last.as_ref().is_some_and(|last| finished.iter().any(|(hash, n)| last.get(hash).is_some_and(|was| n > was)));
    *last = Some(finished);
    if more { REQUESTED.notify_one(); }
}
/// A change inside a torrent still downloading (or a partial file) is the download's own.
fn downloading(path: &std::path::Path) -> bool {
    path.extension().is_some_and(|x| x == "part")
        || DOWNLOADING.read().is_ok_and(|d| d.iter().any(|at| path.starts_with(at)))
}

/// The library folders that exist now, on every drive.
fn roots(s: &Service) -> BTreeSet<PathBuf> {
    let Ok(config) = s.effective_config() else { return BTreeSet::new() };
    crate::drives::Drives::of(&config).all_roots().map(|(_, _, root)| root.to_path_buf()).filter(|root| root.is_dir()).collect()
}

/// Watch `roots`; each change sends on `changed`. None when nothing could be watched.
fn watch(roots: &BTreeSet<PathBuf>, changed: tokio::sync::mpsc::UnboundedSender<()>) -> Option<notify::RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        // Only what can change the library: files and folders made, changed, renamed or removed.
        if let Ok(event) = event && !matches!(event.kind, notify::EventKind::Access(_)) && event.paths.iter().any(|p| !hidden(p) && !downloading(p)) {
            let _ = changed.send(());
        }
    }).map_err(|error| tracing::warn!(%error, "Library watch could not start")).ok()?;
    let mut watched = 0;
    for root in roots {
        match watcher.watch(root, RecursiveMode::Recursive) {
            Ok(()) => watched += 1,
            Err(error) => tracing::warn!(root=%root.display(), %error, "Library folder not watched"),
        }
    }
    (watched > 0).then_some(watcher)
}

/// Finder's and the system's own files (.DS_Store, ._ resource forks, .Spotlight-V100): not library changes.
fn hidden(path: &std::path::Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'))
}

/// Scan now, logging a failure as the timer loop did.
async fn scan(s: &Service, why: &str) {
    tracing::info!(why, "Library scan");
    if let Err(error) = crate::api::scan_library(s).await {
        let _ = s.change_state(|state| state["library_scan"] = serde_json::json!({"error": error.to_string()}));
        eprintln!("Library scan: {error}");
    }
}

/// Run for the server's life: a scan at start, then on changes (see the top).
pub async fn run(s: Arc<Service>) {
    let (sender, mut changes) = tokio::sync::mpsc::unbounded_channel();
    let mut watched = roots(&s);
    let mut watcher = watch(&watched, sender.clone());
    eprintln!("Library watch: {} folder(s) watched for changes", if watcher.is_some() { watched.len() } else { 0 });
    scan(&s, "start").await;
    let mut last_scan = Instant::now();
    // The first change not yet scanned, and the latest.
    let mut pending: Option<(Instant, Instant)> = None;
    let mut roots_checked = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            () = REQUESTED.notified() => {
                // Something finished downloading: scan once it settles, like a change.
                let now = Instant::now();
                pending = Some(pending.map_or((now, now), |(first, _)| (first, now)));
            }
            Some(()) = changes.recv() => {
                let now = Instant::now();
                pending = Some(pending.map_or((now, now), |(first, _)| (first, now)));
                // Drain a burst at once (a copy reports every block).
                while changes.try_recv().is_ok() {}
            }
            _ = tick.tick() => {
                // The folders watched: a drive connected or removed changes them.
                if roots_checked.elapsed() >= Duration::from_secs(60) {
                    roots_checked = Instant::now();
                    let now = roots(&s);
                    if now != watched {
                        watched = now;
                        watcher = watch(&watched, sender.clone());
                        eprintln!("Library watch: folders changed ({} watched)", if watcher.is_some() { watched.len() } else { 0 });
                        scan(&s, "library folders changed").await;
                        last_scan = Instant::now(); pending = None;
                        continue;
                    }
                }
                if let Some((first, latest)) = pending {
                    let settled = latest.elapsed() >= SETTLE;
                    // A download writing for minutes still shows up within a minute.
                    let overdue = first.elapsed() >= MIN_INTERVAL;
                    if settled || overdue {
                        pending = None;
                        scan(&s, "files changed").await;
                        last_scan = Instant::now();
                    }
                } else if last_scan.elapsed() >= SAFETY || watcher.is_none() && last_scan.elapsed() >= MIN_INTERVAL {
                    // Every 6 hours anyway; every minute when nothing can be watched.
                    scan(&s, if watcher.is_none() { "no folders watched" } else { "every 6 hours" }).await;
                    last_scan = Instant::now();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn a_new_file_in_a_watched_folder_is_reported_and_finders_files_are_not() {
        let dir = std::env::temp_dir().join(format!("torrbaby-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Movies")).unwrap();
        let dir = dir.canonicalize().unwrap();
        let (sender, mut changes) = tokio::sync::mpsc::unbounded_channel();
        let _watcher = watch(&BTreeSet::from([dir.clone()]), sender).expect("watching");
        tokio::time::sleep(Duration::from_millis(500)).await;
        // Finder's own file: not a library change.
        std::fs::write(dir.join("Movies/.DS_Store"), b"x").unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        while changes.try_recv().is_ok() {}
        std::fs::write(dir.join("Movies/Film (2020).mkv"), b"video").unwrap();
        let reported = tokio::time::timeout(Duration::from_secs(10), changes.recv()).await;
        let _ = std::fs::remove_dir_all(&dir);
        assert!(reported.is_ok_and(|r| r.is_some()), "a new file is reported");
        assert!(hidden(std::path::Path::new("/M/.DS_Store")) && !hidden(std::path::Path::new("/M/Film.mkv")));
    }
}
