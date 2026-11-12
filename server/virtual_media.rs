//! Persistent, bounded MKV indices. Original media is always opened read-only.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
#[derive(Serialize, Deserialize)]
struct Job {
    id: String,
    asset: String,
    input: PathBuf,
    revision: String,
    state: String,
    error: Option<String>,
    #[serde(default)]
    preparation_version: u32,
    #[serde(default)]
    captions: Vec<crate::virtual_captions::Caption>,
    /// Set when every source audio track uses a codec the virtual MP4 cannot
    /// carry. The video-only index serves analysis; playback stays refused.
    #[serde(default)]
    missing_audio: Option<String>,
}
// Version 4 tolerates Matroska timestamp jitter, empty video cadence slots and
// unsupported audio codecs, so earlier failures are retried once.
// Version 5 writes open-GOP random-access tables (stss/stps). The virtual MP4
// bytes differ, so the version is part of new job IDs (download ETags) and
// index paths. Earlier ready jobs keep their own index, so resumed downloads
// never mix layouts; titles get the new layout lazily on their next
// preparation, and old indexes leave through normal cache reclamation.
// Version 6 stops reading at the end of the Matroska segment, so files with bytes
// appended after it no longer fail; those earlier failures are retried once.
const PREPARATION_VERSION: u32 = 6;
/// The preparation version from which indexes carry random-access tables (see version 5).
const RANDOM_ACCESS_VERSION: u32 = 5;
const LAYOUT_VERSION: u32 = 3;
/// Dolby Vision files are indexed again, apart from their earlier index: their sample
/// description now declares Dolby Vision, and the earlier one played in the wrong
/// colours on Apple devices. Other files keep the index they have.
const DOLBY_LAYOUT: u32 = 1;
/// Whether a file carries Dolby Vision, remembered per file and state so the reader runs once.
fn dolby(input: &Path) -> bool {
    static KNOWN: std::sync::LazyLock<Mutex<HashMap<(PathBuf, String), bool>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
    let Ok(revision) = crate::subtitle_jobs::revision(input) else { return false };
    let key = (input.to_owned(), revision);
    if let Some(known) = KNOWN.lock().ok().and_then(|known| known.get(&key).copied()) { return known; }
    let found = crate::media_format::dolby_vision(input).is_some();
    if let Ok(mut known) = KNOWN.lock() { if known.len() > 4096 { known.clear(); } known.insert(key, found); }
    found
}
/// Disk budget for virtual-media indexes. Layout changes make every MKV rebuild
/// once while older indexes still serve existing jobs, so leave ample headroom.
const INDEX_BUDGET: u64 = 24 * 1024 * 1024 * 1024;
fn playback_refusal(j: &Job) -> Option<String> {
    // Same wording as the remux failure these titles reported before.
    j.missing_audio.as_ref().map(|codec| format!("Remux codec not implemented yet: {codec}"))
}
pub struct Manager {
    root: PathBuf,
    pending: Mutex<HashSet<String>>,
    /// Pending builds requested by background processing rather than playback.
    background: Mutex<HashSet<String>>,
    gate: tokio::sync::Semaphore,
    background_gate: tokio::sync::Semaphore,
    readers: Mutex<HashMap<String, Arc<crate::virtual_captions::Reader>>>,
}
fn valid(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl Manager {
    pub fn new(data: &Path) -> Result<Arc<Self>> {
        let root = data.join("virtual-media-v1");
        fs::create_dir_all(&root)?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        // An unfinished build is never published; completed indices survive restart.
        for item in fs::read_dir(&root)? {
            let item = item?;
            let name = item.file_name();
            let name = name.to_string_lossy();
            if name
                .strip_suffix(".building")
                .is_some_and(|n| valid(n) || n.strip_prefix("base-").is_some_and(valid))
                && item.file_type()?.is_dir()
            {
                fs::remove_dir_all(item.path())?;
            }
        }
        crate::audio_transcode::sweep(data);
        Ok(Arc::new(Self {
            root,
            pending: Mutex::new(HashSet::new()),
            background: Mutex::new(HashSet::new()),
            gate: tokio::sync::Semaphore::new(1),
            background_gate: tokio::sync::Semaphore::new(4),
            readers: Mutex::new(HashMap::new()),
        }))
    }
    fn index_path(&self, job: &Job) -> Result<PathBuf> {
        use sha2::{Digest, Sha256};
        // Jobs from before random-access tables keep their original index.
        let layout = if job.preparation_version >= RANDOM_ACCESS_VERSION { 2 } else { 1 } + if dolby(&job.input) { 100 * DOLBY_LAYOUT } else { 0 };
        let key: String = Sha256::digest(serde_json::to_vec(&(
            layout,
            &job.asset,
            &job.input,
            &job.revision,
        ))?)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
        Ok(self.root.join(format!("base-{key}")))
    }
    /// Playback preparation only. Background indexing (credits analysis) must not
    /// pause the processing queue, or every MKV would stall all other work.
    pub fn preparing(&self) -> bool {
        let Ok(pending)=self.pending.lock() else {return true};
        let Ok(background)=self.background.lock() else {return true};
        pending.iter().any(|id|!background.contains(id))
    }
    /// Whether a file's shared playback index is on disk. Building it reads the
    /// whole file, so a missing index is what makes a first play slow.
    pub fn indexed(&self, asset: &str, input: &Path) -> bool {
        let Ok(revision) = crate::subtitle_jobs::revision(input) else { return false };
        let job = Job {
            id: String::new(), asset: asset.into(), input: input.into(), revision, state: String::new(), error: None,
            preparation_version: PREPARATION_VERSION, captions: Vec::new(), missing_audio: None,
        };
        self.index_path(&job).is_ok_and(|path| path.is_dir())
    }
    pub fn idle(&self) -> bool {
        self.pending.lock().is_ok_and(|p| p.is_empty())
            && self
                .readers
                .lock()
                .is_ok_and(|r| r.values().all(|v| Arc::strong_count(v) == 1))
    }
    fn data(&self) -> &Path { self.root.parent().unwrap_or(&self.root) }
    fn job(&self, id: &str) -> Result<Job> {
        ensure!(valid(id), "Invalid virtual media ID");
        Ok(serde_json::from_slice(&fs::read(
            self.root.join(format!("{id}.json")),
        )?)?)
    }
    fn save(&self, j: &Job) -> Result<()> {
        let path = self.root.join(format!("{}.json", j.id));
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, serde_json::to_vec(j)?)?;
        fs::File::open(&tmp)?.sync_all()?;
        fs::rename(tmp, path)?;
        Ok(())
    }
    pub fn purge_assets(&self, assets: &std::collections::BTreeSet<String>) -> Result<()> {
        let pending = self
            .pending
            .lock()
            .map_err(|_| anyhow::anyhow!("Media queue unavailable"))?;
        let mut ids = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(id) = name
                .to_str()
                .and_then(|n| n.strip_suffix(".json"))
                .filter(|id| valid(id))
            else {
                continue;
            };
            let job = self.job(id)?;
            if assets.contains(&job.asset) {
                ensure!(
                    !pending.contains(id),
                    "Waiting for media preparation to finish"
                );
                ids.push(id.to_owned());
            }
        }
        for id in ids {
            let base = self.index_path(&self.job(&id)?)?;
            if base.exists() {
                ensure!(base.is_dir() && !base.is_symlink(), "Unsafe index path");
                fs::remove_dir_all(base)?;
            }
            for suffix in ["", ".building"] {
                let path = self.root.join(format!("{id}{suffix}"));
                match fs::symlink_metadata(&path) {
                    Ok(info) => {
                        ensure!(
                            info.is_dir() && !info.file_type().is_symlink(),
                            "Unsafe media cache path"
                        );
                        fs::remove_dir_all(path)?;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            self.readers
                .lock()
                .map_err(|_| anyhow::anyhow!("Media cache unavailable"))?
                .remove(&id);
            fs::remove_file(self.root.join(format!("{id}.json")))?;
        }
        // Converted audio copies belong to the title and leave with it.
        crate::audio_transcode::purge_assets(self.data(), assets);
        Ok(())
    }
    pub fn status(&self, id: &str) -> Result<Value> {
        self.status_for(id, false)
    }
    fn status_for(&self, id: &str, analysis: bool) -> Result<Value> {
        let j = self.job(id)?;
        ensure!(
            crate::subtitle_jobs::revision(&j.input)? == j.revision,
            "Media changed; prepare playback again"
        );
        if j.state == "ready" && !analysis {
            if let Some(error) = playback_refusal(&j) {
                return Ok(json!({"id":j.id,"asset_id":j.asset,"state":"failed","error":error}));
            }
        }
        Ok(json!({"id":j.id,"asset_id":j.asset,"state":j.state,"error":j.error}))
    }
    pub async fn reader(
        self: &Arc<Self>,
        id: &str,
        asset: &str,
        path: &Path,
    ) -> Result<Arc<crate::virtual_captions::Reader>> {
        self.reader_for(id, asset, path, false).await
    }
    /// Video analysis reader; unlike playback it accepts a video-only index.
    pub async fn analysis_reader(
        self: &Arc<Self>,
        id: &str,
        asset: &str,
        path: &Path,
    ) -> Result<Arc<crate::virtual_captions::Reader>> {
        self.reader_for(id, asset, path, true).await
    }
    async fn reader_for(
        self: &Arc<Self>,
        id: &str,
        asset: &str,
        path: &Path,
        analysis: bool,
    ) -> Result<Arc<crate::virtual_captions::Reader>> {
        let manager = self.clone();
        let id = id.to_owned();
        let asset = asset.to_owned();
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || manager.reader_sync(&id, &asset, &path, analysis)).await?
    }
    fn reader_sync(
        &self,
        id: &str,
        asset: &str,
        path: &Path,
        analysis: bool,
    ) -> Result<Arc<crate::virtual_captions::Reader>> {
        let j = self.job(id)?;
        ensure!(
            j.asset == asset && j.input == path && j.state == "ready",
            "Virtual media is not ready for this title"
        );
        if !analysis {
            if let Some(error) = playback_refusal(&j) {
                anyhow::bail!(error);
            }
        }
        ensure!(
            crate::subtitle_jobs::revision(path)? == j.revision,
            "Media changed; prepare again"
        );
        if let Ok(directory)=fs::File::open(self.index_path(&j)?) {
            let _=directory.set_times(fs::FileTimes::new().set_modified(std::time::SystemTime::now()));
        }
        let mut readers = self
            .readers
            .lock()
            .map_err(|_| anyhow::anyhow!("Playback index unavailable"))?;
        if let Some(reader) = readers.get(id) {
            reader.validate_source()?;
            return Ok(reader.clone());
        }
        let source = if is_mkv(path) {
            crate::virtual_captions::Source::Indexed(Arc::new(crate::virtual_mkv::Reader::open(
                path,
                &self.index_path(&j)?,
            )?))
        } else {
            crate::virtual_captions::Source::file(path)?
        };
        let reader = Arc::new(crate::virtual_captions::Reader::new(source, &j.captions)?);
        if readers.len() >= 8 {
            readers.clear();
        }
        readers.insert(id.into(), reader.clone());
        Ok(reader)
    }
    pub fn prepare(
        self: &Arc<Self>,
        asset: &str,
        input: PathBuf,
        captions: Vec<crate::virtual_captions::Caption>,
    ) -> Result<Value> {
        let id = self.prepare_job(asset, input, captions, false)?;
        self.status(&id)
    }
    /// Background analysis index (credits): shares the playback index, accepts a
    /// video-only index, runs on its own gate so playback preparation never
    /// queues behind it, and does not pause the processing queue.
    pub fn prepare_analysis(self: &Arc<Self>, asset: &str, input: PathBuf) -> Result<Value> {
        let id = self.prepare_job(asset, input, Vec::new(), true)?;
        self.status_for(&id, true)
    }
    fn prepare_job(
        self: &Arc<Self>,
        asset: &str,
        input: PathBuf,
        captions: Vec<crate::virtual_captions::Caption>,
        background: bool,
    ) -> Result<String> {
        use sha2::{Digest, Sha256};
        ensure!(
            crate::container::virtual_mp4_source(&input),
            "This container needs another playback path"
        );
        let revision = crate::subtitle_jobs::revision(&input)?;
        let id: String = Sha256::digest(serde_json::to_vec(&(
            LAYOUT_VERSION + if dolby(&input) { 1000 * DOLBY_LAYOUT } else { 0 }, asset, &input, &revision, &captions,
        ))?)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| anyhow::anyhow!("Playback queue unavailable"))?;
        if let Ok(j) = self.job(&id) {
            // Transient failures are rebuilt: storage pressure, and indexes that a
            // concurrent reclaim removed mid-build (before reuse was marked).
            let retry_storage=j.error.as_deref()==Some("Virtual media index storage limit reached")
                || j.error.as_deref()==Some("Playback cache is busy. Please try again shortly.")
                || j.error.as_deref().is_some_and(|e|e.contains("No such file or directory"))
                // Earlier builds rejected skewed constant video clocks with this text;
                // irregular timing now fails with a different message, so this retries once.
                || j.error.as_deref()==Some(crate::remux::LEGACY_VIDEO_TIMING)
                // Dolby Atmos (E-AC-3 JOC) audio is supported since this build.
                || j.error.as_deref()==Some("E-AC-3 additional/JOC signalling needs additional support");
            if j.state == "ready" {
                // Mark a reused index as recently used before anyone reclaims space:
                // otherwise a concurrent build can evict it between this check and
                // the caller opening its reader.
                if let Ok(directory)=fs::File::open(self.index_path(&j)?) {
                    let _=directory.set_times(fs::FileTimes::new().set_modified(std::time::SystemTime::now()));
                }
            }
            // A ready index is rebuilt when a converted audio copy can now give it
            // sound (it was built before conversion existed), or when the copy it
            // was built with has gone.
            let mut rebuild = false;
            if j.state == "ready" && !pending.contains(&id) && is_mkv(&j.input) {
                let base = self.index_path(&j)?;
                if base.exists() {
                    let silent = j.missing_audio.is_some()
                        && crate::audio_transcode::wanted(self.data(), &j.asset, &j.revision)
                        && crate::virtual_mkv::Reader::open(&j.input, &base).map(|r| r.missing_audio().is_some()).unwrap_or(true);
                    let lost = !crate::audio_transcode::intact(&crate::virtual_mkv::side_files(&base));
                    if silent || lost {
                        ensure!(base.is_dir() && !base.is_symlink(), "Unsafe index path");
                        fs::remove_dir_all(&base)?;
                    }
                    // Another job may already have rebuilt the shared index with sound.
                    rebuild = silent || lost || j.missing_audio.is_some() && crate::audio_transcode::wanted(self.data(), &j.asset, &j.revision);
                }
            }
            if rebuild {
                if let Ok(mut readers) = self.readers.lock() { readers.remove(&id); }
            } else if (j.state == "ready" && self.index_path(&j)?.exists()) || (j.state == "failed" && j.preparation_version >= PREPARATION_VERSION && !retry_storage) || pending.contains(&id) {
                // A playback request for a background build now counts as playback.
                if !background && pending.contains(&id) {
                    if let Ok(mut set)=self.background.lock() {set.remove(&id);}
                }
                return Ok(id);
            }
        }
        ensure!(pending.len() < 128, "Playback queue is full");
        let job = Job {
            id: id.clone(),
            asset: asset.into(),
            input,
            revision,
            state: "queued".into(),
            error: None,
            preparation_version: PREPARATION_VERSION,
            captions,
            missing_audio: None,
        };
        self.save(&job)?;
        if background {
            self.background.lock().map_err(|_| anyhow::anyhow!("Playback queue unavailable"))?.insert(id.clone());
        }
        pending.insert(id.clone());
        drop(pending);
        let manager = self.clone();
        let run_id = id.clone();
        tokio::spawn(async move {
            let result = async {
                let gate = if background { &manager.background_gate } else { &manager.gate };
                let _permit = gate.acquire().await?;
                let worker = manager.clone();
                let key = run_id.clone();
                tokio::task::spawn_blocking(move || worker.build(&key)).await??;
                Ok::<_, anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                if let Ok(mut j) = manager.job(&run_id) {
                    j.state = "failed".into();
                    j.error = Some(error.to_string());
                    let _ = manager.save(&j);
                }
            }
            if let Ok(mut pending) = manager.pending.lock() {
                pending.remove(&run_id);
                if let Ok(mut set) = manager.background.lock() { set.remove(&run_id); }
            }
        });
        Ok(id)
    }
    fn index_usage(&self) -> Result<Vec<(PathBuf, u64, std::time::SystemTime)>> {
        let mut entries=Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry=entry?;let name=entry.file_name();let name=name.to_string_lossy();
            if !entry.file_type()?.is_dir() || !(valid(&name) || name.strip_prefix("base-").is_some_and(valid)) {continue;}
            let mut size=0u64;
            for child in fs::read_dir(entry.path())? {let child=child?;if child.file_type()?.is_file(){size=size.saturating_add(child.metadata()?.len());}}
            entries.push((entry.path(),size,entry.metadata()?.modified()?));
        }
        Ok(entries)
    }
    pub fn can_preindex(&self) -> bool {
        self.index_usage().is_ok_and(|entries|entries.iter().map(|(_,n,_)|*n).sum::<u64>() + 256*1024*1024 <= INDEX_BUDGET)
    }
    fn reclaim_indexes(&self) -> Result<()> {
        let mut entries=self.index_usage()?;
        let mut used=entries.iter().map(|(_,n,_)|*n).sum::<u64>();
        if used + 256*1024*1024 <= INDEX_BUDGET {return Ok(());}
        let mut protected=HashSet::new();
        {
            let pending=self.pending.lock().map_err(|_|anyhow::anyhow!("Playback queue unavailable"))?;
            for id in pending.iter() {if let Ok(job)=self.job(id){protected.insert(self.index_path(&job)?);}}
        }
        {
            let readers=self.readers.lock().map_err(|_|anyhow::anyhow!("Playback index unavailable"))?;
            for id in readers.keys() {if let Ok(job)=self.job(id){protected.insert(self.index_path(&job)?);}}
        }
        entries.sort_by_key(|(_,_,modified)|*modified);
        for (path,size,modified) in entries {
            if used + 256*1024*1024 <= INDEX_BUDGET {break;}
            // Recent readers may still be in flight after leaving the small RAM cache.
            if protected.contains(&path) || modified.elapsed().unwrap_or_default().as_secs()<3600 {continue;}
            fs::remove_dir_all(&path)?;
            used=used.saturating_sub(size);
        }
        ensure!(used+256*1024*1024<=INDEX_BUDGET,"Playback cache is busy. Please try again shortly.");
        Ok(())
    }
    fn build(&self, id: &str) -> Result<()> {
        let mut job = self.job(id)?;
        job.state = "running".into();
        self.save(&job)?;
        let final_path = self.index_path(&job)?;
        if !final_path.exists() {
            // MP4 captions are virtual; they do not allocate an MKV disk index.
            if is_mkv(&job.input) { self.reclaim_indexes()?; }
            let temporary = final_path.with_extension("building");
            if is_mkv(&job.input) {
                // Converts audio Apple devices cannot decode on first use; later builds reuse the copy.
                let sidecars = crate::audio_transcode::ensure(self.data(), &job.asset, &job.input, &job.revision);
                if let Err(error) = crate::virtual_mkv::build(&job.input, &temporary, &sidecars) {
                    if sidecars.is_empty() { return Err(error); }
                    // The copy could not be used: keep the title indexed without it.
                    crate::audio_transcode::fail(self.data(), &job.asset, &job.input, &job.revision, &error.to_string());
                    crate::virtual_mkv::build(&job.input, &temporary, &[])?;
                }
            } else {
                fs::create_dir(&temporary)?;
            }
            ensure!(
                crate::subtitle_jobs::revision(&job.input)? == job.revision,
                "Media changed during preparation"
            );
            fs::rename(&temporary, &final_path)?;
            fs::File::open(&self.root)?.sync_all()?;
        }
        let source = if is_mkv(&job.input) {
            let index = crate::virtual_mkv::Reader::open(&job.input, &final_path)?;
            job.missing_audio = index.missing_audio().map(str::to_owned);
            crate::virtual_captions::Source::Indexed(Arc::new(index))
        } else {
            crate::virtual_captions::Source::file(&job.input)?
        };
        let reader = Arc::new(
            crate::virtual_captions::Reader::new(source, &job.captions)
                .context("Prepare native subtitle tracks")?,
        );
        {
            let mut readers = self
                .readers
                .lock()
                .map_err(|_| anyhow::anyhow!("Playback index unavailable"))?;
            if readers.len() >= 8 {
                readers.clear();
            }
            readers.insert(id.into(), reader);
        }
        job.state = "ready".into();
        job.error = None;
        self.save(&job)
    }
}

/// By content, not extension: a Matroska file named .mp4 still needs the index.
fn is_mkv(path: &Path) -> bool {
    crate::container::is_matroska(path)
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    #[test]
    fn only_old_unprotected_indexes_are_reclaimed() -> Result<()> {
        let root=std::env::temp_dir().join(format!("torrbaby-index-reclaim-{}-{}",std::process::id(),uuid::Uuid::new_v4()));
        let manager=Manager::new(&root)?;
        let base=manager.root.join(format!("base-{}","a".repeat(64)));
        fs::create_dir(&base)?;
        fs::File::create(base.join("metadata.bin"))?.set_len(INDEX_BUDGET)?;
        assert!(!manager.can_preindex());
        assert!(manager.reclaim_indexes().is_err());
        assert!(base.exists());
        fs::File::open(&base)?.set_times(fs::FileTimes::new().set_modified(std::time::SystemTime::now()-std::time::Duration::from_secs(7200)))?;
        manager.reclaim_indexes()?;
        assert!(!base.exists());
        assert!(manager.can_preindex());
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn a_reclaimed_index_reads_as_missing_until_rebuilt() -> Result<()> {
        let root=std::env::temp_dir().join(format!("torrbaby-indexed-{}",uuid::Uuid::new_v4()));
        let manager=Manager::new(&root)?;
        let input=root.join("episode.mkv");
        fs::write(&input,b"media")?;
        assert!(!manager.indexed("asset-one",&input));
        let job=Job{id:String::new(),asset:"asset-one".into(),input:input.clone(),revision:crate::subtitle_jobs::revision(&input)?,
            state:String::new(),error:None,preparation_version:PREPARATION_VERSION,captions:Vec::new(),missing_audio:None};
        fs::create_dir(manager.index_path(&job)?)?;
        assert!(manager.indexed("asset-one",&input));
        assert!(!manager.indexed("asset-two",&input));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn earlier_ready_jobs_keep_their_index_while_new_jobs_use_the_new_layout() -> Result<()> {
        let root=std::env::temp_dir().join(format!("torrbaby-layout-{}",uuid::Uuid::new_v4()));
        let manager=Manager::new(&root)?;
        let input=root.join("title.mkv");
        fs::write(&input,b"\x00\x00\x00\x10ftypisom\x00\x00\x02\x00source")?; // sniffed as MP4
        let revision=crate::subtitle_jobs::revision(&input)?;
        let id="c".repeat(64);
        fs::write(root.join("virtual-media-v1").join(format!("{id}.json")),serde_json::to_vec(&json!({
            "id":id,"asset":"asset-1","input":input,"revision":revision,"state":"ready","error":null,"preparation_version":4}))?)?;
        let mut old=manager.job(&id)?;
        let old_path=manager.index_path(&old)?;
        old.preparation_version=PREPARATION_VERSION;
        // Same title, new layout: a separate index and (in prepare) a separate job ID/ETag.
        assert_ne!(manager.index_path(&old)?,old_path);
        fs::create_dir(&old_path)?;
        // The old job stays ready against its own index for resumed downloads.
        assert!(manager.index_path(&manager.job(&id)?)?.exists());
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn video_only_index_serves_analysis_but_not_playback() -> Result<()> {
        let root=std::env::temp_dir().join(format!("torrbaby-video-only-{}",uuid::Uuid::new_v4()));
        let manager=Manager::new(&root)?;
        let input=root.join("title.mkv");
        fs::write(&input,b"\x00\x00\x00\x10ftypisom\x00\x00\x02\x00source")?; // sniffed as MP4
        let revision=crate::subtitle_jobs::revision(&input)?;
        // Jobs saved by earlier releases have no missing_audio field.
        let id="b".repeat(64);
        fs::write(root.join("virtual-media-v1").join(format!("{id}.json")),serde_json::to_vec(&json!({
            "id":id,"asset":"asset-1","input":input,"revision":revision,"state":"ready","error":null,"preparation_version":3}))?)?;
        assert_eq!(manager.status(&id)?["state"],"ready");
        let mut job=manager.job(&id)?;
        job.missing_audio=Some("A_DTS".into());
        manager.save(&job)?;
        let playback=manager.status(&id)?;
        assert_eq!((playback["state"].as_str(),playback["error"].as_str()),(Some("failed"),Some("Remux codec not implemented yet: A_DTS")));
        assert_eq!(manager.status_for(&id,true)?["state"],"ready");
        let refused=manager.reader_sync(&id,"asset-1",&input,false).err().map(|e|e.to_string());
        assert_eq!(refused.as_deref(),Some("Remux codec not implemented yet: A_DTS"));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn earlier_preparation_failures_are_retried_once() -> Result<()> {
        let root=std::env::temp_dir().join(format!("torrbaby-retry-{}",uuid::Uuid::new_v4()));
        let manager=Manager::new(&root)?;
        let input=root.join("title.mp4");
        fs::write(&input,b"\x00\x00\x00\x10ftypisom\x00\x00\x02\x00source")?; // sniffed as MP4
        let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        runtime.block_on(async {
            let id=manager.prepare_job("asset-1",input.clone(),Vec::new(),false)?;
            while manager.preparing() {tokio::time::sleep(std::time::Duration::from_millis(5)).await;}
            for (version,retried) in [(3,true),(PREPARATION_VERSION,false)] {
                let mut job=manager.job(&id)?;
                job.state="failed".into();job.error=Some("Audio offset/gap needs additional timing support".into());job.preparation_version=version;
                manager.save(&job)?;
                manager.prepare_job("asset-1",input.clone(),Vec::new(),false)?;
                while manager.preparing() {tokio::time::sleep(std::time::Duration::from_millis(5)).await;}
                assert_eq!(manager.job(&id)?.state=="ready",retried);
            }
            Ok::<_,anyhow::Error>(())
        })?;
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
