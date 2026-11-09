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
    captions: Vec<crate::virtual_captions::Caption>,
}
pub struct Manager {
    root: PathBuf,
    pending: Mutex<HashSet<String>>,
    gate: tokio::sync::Semaphore,
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
        Ok(Arc::new(Self {
            root,
            pending: Mutex::new(HashSet::new()),
            gate: tokio::sync::Semaphore::new(1),
            readers: Mutex::new(HashMap::new()),
        }))
    }
    fn index_path(&self, job: &Job) -> Result<PathBuf> {
        use sha2::{Digest, Sha256};
        let key: String = Sha256::digest(serde_json::to_vec(&(
            1,
            &job.asset,
            &job.input,
            &job.revision,
        ))?)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
        Ok(self.root.join(format!("base-{key}")))
    }
    pub fn idle(&self) -> bool {
        self.pending.lock().is_ok_and(|p| p.is_empty())
            && self
                .readers
                .lock()
                .is_ok_and(|r| r.values().all(|v| Arc::strong_count(v) == 1))
    }
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
        Ok(())
    }
    pub fn status(&self, id: &str) -> Result<Value> {
        let j = self.job(id)?;
        ensure!(
            crate::subtitle_jobs::revision(&j.input)? == j.revision,
            "Media changed; prepare playback again"
        );
        Ok(json!({"id":j.id,"asset_id":j.asset,"state":j.state,"error":j.error}))
    }
    pub async fn reader(
        self: &Arc<Self>,
        id: &str,
        asset: &str,
        path: &Path,
    ) -> Result<Arc<crate::virtual_captions::Reader>> {
        let manager = self.clone();
        let id = id.to_owned();
        let asset = asset.to_owned();
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || manager.reader_sync(&id, &asset, &path)).await?
    }
    fn reader_sync(
        &self,
        id: &str,
        asset: &str,
        path: &Path,
    ) -> Result<Arc<crate::virtual_captions::Reader>> {
        let j = self.job(id)?;
        ensure!(
            j.asset == asset && j.input == path && j.state == "ready",
            "Virtual media is not ready for this title"
        );
        ensure!(
            crate::subtitle_jobs::revision(path)? == j.revision,
            "Media changed; prepare again"
        );
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
        use sha2::{Digest, Sha256};
        ensure!(
            input
                .extension()
                .is_some_and(|s| ["mkv", "mp4", "m4v", "mov"]
                    .iter()
                    .any(|e| s.eq_ignore_ascii_case(e))),
            "This container needs another playback path"
        );
        let revision = crate::subtitle_jobs::revision(&input)?;
        let id: String = Sha256::digest(serde_json::to_vec(&(
            2, asset, &input, &revision, &captions,
        ))?)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| anyhow::anyhow!("Playback queue unavailable"))?;
        if let Ok(j) = self.job(&id) {
            if j.state == "ready" || j.state == "failed" || pending.contains(&id) {
                return self.status(&id);
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
            captions,
        };
        self.save(&job)?;
        pending.insert(id.clone());
        let manager = self.clone();
        let run_id = id.clone();
        tokio::spawn(async move {
            let result = async {
                let _permit = manager.gate.acquire().await?;
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
            }
        });
        self.status(&id)
    }
    fn build(&self, id: &str) -> Result<()> {
        let mut job = self.job(id)?;
        job.state = "running".into();
        self.save(&job)?;
        let final_path = self.index_path(&job)?;
        if !final_path.exists() {
            let mut used = 0u64;
            for entry in fs::read_dir(&self.root)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    for file in fs::read_dir(entry.path())? {
                        used = used.saturating_add(file?.metadata()?.len());
                    }
                }
            }
            ensure!(
                used + 256 * 1024 * 1024 <= 2 * 1024 * 1024 * 1024,
                "Virtual media index storage limit reached"
            );
            let temporary = final_path.with_extension("building");
            if is_mkv(&job.input) {
                crate::virtual_mkv::build(&job.input, &temporary)?;
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
            crate::virtual_captions::Source::Indexed(Arc::new(crate::virtual_mkv::Reader::open(
                &job.input,
                &final_path,
            )?))
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

fn is_mkv(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("mkv"))
}
