//! Small, durable, revision-keyed cache for asynchronous subtitle preparation.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest;
use std::{
    collections::HashSet,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Serialize, Deserialize)]
struct Job {
    id: String,
    asset_id: String,
    input: PathBuf,
    revision: String,
    track: u64,
    source: Option<String>,
    target: Option<String>,
    state: String,
    updated: f64,
    error: Option<String>,
    #[serde(default)]
    completed_cues: usize,
    #[serde(default)]
    total_cues: usize,
}

#[derive(Deserialize)]
struct Progress { completed: usize, cues: Vec<crate::subtitles::TimedCue> }
#[derive(Serialize)]
struct ProgressRef<'a> { completed: usize, cues: &'a [crate::subtitles::TimedCue] }

pub struct Manager {
    root: PathBuf,
    pending: Mutex<HashSet<String>>,
    gate: tokio::sync::Semaphore,
}
pub(crate) fn revision(path: &Path) -> Result<String> {
    let m = path.metadata()?;
    ensure!(m.is_file(), "Media is not a file");
    Ok(format!(
        "{}:{}:{}:{}:{}:{}:{}",
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec()
    ))
}
fn valid(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl Manager {
    pub fn new(data: &Path) -> Result<Arc<Self>> {
        let root = data.join("subtitles");
        fs::create_dir_all(&root)?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        // A server lease already protects this data directory. Recover only our
        // own interrupted atomic writes; never sweep arbitrary user filenames.
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name
                .strip_suffix(".progress.tmp")
                .or_else(|| name.strip_suffix(".vtt.tmp"))
                .or_else(|| name.strip_suffix(".json.tmp"))
                .is_some_and(valid)
            {
                fs::remove_file(entry.path())?;
            } else if let Some(id) = name.strip_suffix(".vtt").filter(|id| valid(id)) {
                if !root.join(format!("{id}.json")).exists() {
                    fs::remove_file(entry.path())?;
                }
            }
        }
        Ok(Arc::new(Self {
            root,
            pending: Mutex::new(HashSet::new()),
            gate: tokio::sync::Semaphore::new(1),
        }))
    }
    pub fn busy(&self) -> bool {
        self.pending.lock().map(|p| !p.is_empty()).unwrap_or(true)
    }
    fn read(&self, id: &str) -> Result<Job> {
        ensure!(valid(id), "Invalid subtitle job ID");
        let bytes = fs::read(self.root.join(format!("{id}.json")))?;
        ensure!(bytes.len() <= 16_384, "Invalid subtitle job record");
        let job: Job = serde_json::from_slice(&bytes)?;
        ensure!(job.id == id, "Subtitle job identity mismatch");
        Ok(job)
    }
    fn save(&self, job: &Job) -> Result<()> {
        let temporary = self.root.join(format!("{}.json.tmp", job.id));
        fs::write(&temporary, serde_json::to_vec(job)?)?;
        fs::rename(temporary, self.root.join(format!("{}.json", job.id)))?;
        Ok(())
    }
    pub fn status(&self, id: &str) -> Result<Value> {
        let mut job = self.read(id)?;
        if revision(&job.input).ok().as_deref() != Some(&job.revision) {
            job.state = "stale".into();
        } else if job.state == "ready" && !self.root.join(format!("{id}.vtt")).is_file() {
            job.state = "expired".into();
        } else if ["queued", "running"].contains(&job.state.as_str())
            && !self
                .pending
                .lock()
                .map_err(|_| anyhow::anyhow!("Subtitle queue unavailable"))?
                .contains(id)
        {
            job.state = "interrupted".into();
        }
        Ok(
            json!({"id":job.id,"asset_id":job.asset_id,"state":job.state,"error":job.error,
            "track":job.track,"source":job.source,"target":job.target,"completed_cues":job.completed_cues,"total_cues":job.total_cues,
            "path":if job.state=="ready" {Some(format!("/v1/subtitle-jobs/{id}.vtt"))} else {None}}),
        )
    }
    pub fn artifact(&self, id: &str) -> Result<String> {
        ensure!(
            self.status(id)?["state"] == "ready",
            "Subtitles are not ready"
        );
        let data = fs::read(self.root.join(format!("{id}.vtt")))?;
        ensure!(data.len() <= 16_777_216, "Subtitle output too large");
        String::from_utf8(data).context("Invalid prepared subtitles")
    }
    pub fn prepare(
        self: &Arc<Self>,
        asset_id: &str,
        input: PathBuf,
        track: u64,
        source: Option<String>,
        target: Option<String>,
    ) -> Result<Value> {
        ensure!(track > 0, "Invalid subtitle track");
        ensure!(
            input
                .extension()
                .and_then(|s|s.to_str())
                .is_some_and(|s| ["mkv", "webm", "mp4", "m4v", "mov"].contains(&s.to_ascii_lowercase().as_str())),
            "Embedded preparation supports Matroska and MP4 text subtitles"
        );
        ensure!(
            (target.is_none() && source.as_deref().is_none_or(|s| ["en", "zh-Hans"].contains(&s)))
                || (source != target
                    && source
                        .as_deref()
                        .is_some_and(|s| ["en", "zh-Hans"].contains(&s))
                    && target
                        .as_deref()
                        .is_some_and(|s| ["en", "zh-Hans"].contains(&s))),
            "Choose English and Simplified Chinese for translation"
        );
        let revision = revision(&input)?;
        let key = serde_json::to_vec(&(1, asset_id, &input, &revision, track, &source, &target))?;
        let id: String = sha2::Sha256::digest(key)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if let Ok(status) = self.status(&id) {
            if status["state"] == "ready" {
                return Ok(status);
            }
        }
        {
            let mut pending = self
                .pending
                .lock()
                .map_err(|_| anyhow::anyhow!("Subtitle queue unavailable"))?;
            if pending.contains(&id) {
                drop(pending);
                return self.status(&id);
            }
            ensure!(
                pending.len() < 4,
                "Subtitle preparation queue is full. Try again shortly."
            );
            self.prune(&pending)?;
            let job = Job {
                id: id.clone(),
                asset_id: asset_id.into(),
                input,
                revision,
                track,
                source,
                target,
                state: "queued".into(),
                updated: crate::storage::now(),
                error: None,
                completed_cues: 0, total_cues: 0,
            };
            self.save(&job)?;
            pending.insert(id.clone());
            let manager = self.clone();
            tokio::spawn(async move {
                manager.run(job).await;
            });
        }
        self.status(&id)
    }
    fn prune(&self, pending: &HashSet<String>) -> Result<()> {
        let mut entries = Vec::new();
        let mut bytes = 0u64;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type()?.is_file() {
                continue;
            }
            bytes = bytes.saturating_add(entry.metadata()?.len());
            if path.extension().is_some_and(|e| e == "json") {
                if let Some(id) = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .filter(|id| valid(id) && !pending.contains(*id))
                {
                    entries.push((entry.metadata()?.modified()?, id.to_owned()));
                }
            }
        }
        entries.sort();
        let mut count = entries.len() + pending.len();
        for (_, id) in entries {
            if bytes < 496 * 1024 * 1024 && count < 4096 {
                break;
            }
            if self.read(&id).is_ok_and(|job| job.state == "ready") { continue; }
            for ext in ["json", "vtt", "progress"] {
                let path = self.root.join(format!("{id}.{ext}"));
                if path.exists() {
                    let size = path.metadata()?.len();
                    fs::remove_file(path)?;
                    bytes = bytes.saturating_sub(size);
                }
            }
            count = count.saturating_sub(1);
        }
        ensure!(bytes < 496 * 1024 * 1024 && count < 4096, "Subtitle storage is full");
        Ok(())
    }
    async fn run(self: Arc<Self>, mut job: Job) {
        let result = async {
            let _permit = self.gate.acquire().await?;
            job.state = "running".into();
            self.save(&job)?;
            let mut cues = crate::native_subtitles::extract(&job.input, job.track).await?;
            job.total_cues = cues.len();
            let checkpoint = self.root.join(format!("{}.progress",job.id));
            if let (Some(source), Some(target)) = (job.source.clone(), job.target.clone()) {
                if let Ok(data) = fs::read(&checkpoint) {
                    if data.len() <= 16_777_216 {
                        if let Ok(saved) = serde_json::from_slice::<Progress>(&data) {
                            if saved.completed <= cues.len() && saved.cues.len() == cues.len()
                                && saved.cues.iter().zip(&cues).all(|(a,b)| a.start_ms==b.start_ms && a.end_ms==b.end_ms && !a.text.trim().is_empty() && a.text.len()<=16_384) {
                                for (cue, translated) in cues.iter_mut().zip(saved.cues).take(saved.completed) { cue.text=translated.text; }
                                job.completed_cues=saved.completed;
                            }
                        }
                    }
                }
                self.save(&job)?;
                let helper = std::env::current_exe()?.with_file_name("torrbaby-subtitle-translator");
                while job.completed_cues < cues.len() {
                    let end=(job.completed_cues+8).min(cues.len());
                    crate::subtitles::translate_cues(&helper,&source,&target,&mut cues[job.completed_cues..end]).await?;
                    job.completed_cues=end;
                    let progress=serde_json::to_vec(&ProgressRef {completed:end,cues:&cues})?;
                    ensure!(progress.len()<=16_777_216,"Subtitle checkpoint too large");
                    let used=fs::read_dir(&self.root)?.try_fold(0u64, |sum, entry| -> Result<u64> { Ok(sum.saturating_add(entry?.metadata()?.len())) })?;
                    ensure!(used.saturating_add(progress.len() as u64)<=512*1024*1024,"Subtitle storage is full");
                    let temporary=self.root.join(format!("{}.progress.tmp",job.id));
                    fs::write(&temporary,progress)?;
                    fs::rename(temporary,&checkpoint)?;
                    job.updated=crate::storage::now();
                    self.save(&job)?;
                }
            } else { job.completed_cues = cues.len(); }

            ensure!(
                revision(&job.input)? == job.revision,
                "Media changed during subtitle preparation"
            );
            let output = crate::subtitles::webvtt(&cues);
            ensure!(output.len() <= 16_777_216, "Subtitle output too large");
            let pending = self
                .pending
                .lock()
                .map_err(|_| anyhow::anyhow!("Subtitle queue unavailable"))?;
            self.prune(&pending)?;
            let temporary = self.root.join(format!("{}.vtt.tmp", job.id));
            fs::write(&temporary, output)?;
            fs::rename(temporary, self.root.join(format!("{}.vtt", job.id)))?;
            if checkpoint.exists() { fs::remove_file(checkpoint)?; }
            Ok::<(), anyhow::Error>(())
        };
        let result = tokio::time::timeout(Duration::from_secs(1200), result).await;
        job.updated = crate::storage::now();
        match result {
            Ok(Ok(())) => {
                job.state = "ready".into();
                job.error = None;
            }
            Ok(Err(error)) => {
                job.state = "failed".into();
                job.error = Some(error.to_string());
            }
            Err(_) => {
                job.state = "failed".into();
                job.error = Some("Subtitle preparation timed out".into());
            }
        }
        if self.save(&job).is_err() {
            eprintln!("Could not persist subtitle job status");
        }
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&job.id);
        }
    }
}
