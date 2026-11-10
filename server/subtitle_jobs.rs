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
    #[serde(default)]
    extraction_method: Option<String>,
    #[serde(default)]
    unreadable_cues: usize,
}

#[derive(Deserialize)]
struct Progress { completed: usize, cues: Vec<crate::subtitles::TimedCue> }
#[derive(Serialize)]
struct ProgressRef<'a> { completed: usize, cues: &'a [crate::subtitles::TimedCue] }

pub struct Manager {
    root: PathBuf,
    pending: Mutex<HashSet<String>>,
    gate: tokio::sync::Semaphore,
    concurrency: std::sync::atomic::AtomicUsize,
    urgent: std::sync::atomic::AtomicBool,
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
        crate::ocr_subtitles::cleanup(&root)?;
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
            gate: tokio::sync::Semaphore::new(2),
            concurrency: std::sync::atomic::AtomicUsize::new(1),
            urgent: std::sync::atomic::AtomicBool::new(false),
        }))
    }
    pub fn set_urgent(&self, urgent:bool) {
        self.urgent.store(urgent,std::sync::atomic::Ordering::Relaxed);
    }
    pub fn can_admit(&self)->bool {
        self.pending.lock().map(|p|p.len()<self.concurrency.load(std::sync::atomic::Ordering::Relaxed)).unwrap_or(false)
    }
    pub fn set_concurrency(&self, limit:usize) {
        self.concurrency.store(limit.min(2),std::sync::atomic::Ordering::Relaxed);
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
        let ocr_progress=if job.state=="running" {fs::read(self.root.join(format!("{id}.ocr-progress"))).ok().and_then(|b|serde_json::from_slice::<Value>(&b).ok())} else {None};
        if let Some(p)=&ocr_progress {job.completed_cues=p["completed_cues"].as_u64().unwrap_or(0) as usize;job.total_cues=p["total_cues"].as_u64().unwrap_or(0) as usize;}
        Ok(
            json!({"updated":job.updated,"extraction_method":job.extraction_method,"unreadable_cues":job.unreadable_cues,"stage":if ocr_progress.is_some(){"ocr"}else if job.target.is_some(){"translation"}else{"extraction"},"id":job.id,"asset_id":job.asset_id,"state":job.state,"error":job.error,
            "sidecar":if job.input.extension().is_some_and(|e|e.eq_ignore_ascii_case("srt")) { job.input.file_name().and_then(|n|n.to_str()).map(|name|sha2::Sha256::digest(name.as_bytes()).iter().map(|b|format!("{b:02x}")).collect::<String>()) } else {None},
            "track":job.track,"source":job.source.as_deref().filter(|s|*s!="auto"),"target":job.target.as_deref().filter(|s|*s!="auto"),"completed_cues":job.completed_cues,"total_cues":job.total_cues,
            "path":if job.state=="ready" {Some(format!("/v1/subtitle-jobs/{id}.vtt"))} else {None}}),
        )
    }
    /// Include manually requested and recovered work as well as background jobs.
    pub fn purge_assets(&self, assets: &std::collections::BTreeSet<String>) -> Result<()> {
        let pending = self.pending.lock().map_err(|_| anyhow::anyhow!("Subtitle queue unavailable"))?;
        let mut ids = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(id) = name.to_str().and_then(|n| n.strip_suffix(".json")).filter(|id|valid(id)) else { continue; };
            let job = self.read(id)?;
            if assets.contains(&job.asset_id) {
                ensure!(!pending.contains(id), "Waiting for subtitle preparation to finish");
                ids.push(id.to_owned());
            }
        }
        for id in ids {
            // Keep the identity record until every associated artifact is removed.
            for ext in ["vtt", "progress", "vtt.tmp", "progress.tmp", "json.tmp", "json"] {
                let path = self.root.join(format!("{id}.{ext}"));
                match fs::symlink_metadata(&path) {
                    Ok(info) => { ensure!(info.is_file() && !info.file_type().is_symlink(), "Unsafe subtitle cache path"); fs::remove_file(path)?; }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(())
    }
    pub fn all_statuses(&self) -> Result<Vec<Value>> {
        let mut result = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(id) = name.to_str().and_then(|n| n.strip_suffix(".json")).filter(|id| valid(id)) else { continue; };
            if let Ok(status) = self.status(id) {
                if status["state"] != "stale" && status["state"] != "expired" { result.push(status); }
            }
        }
        result.sort_by(|a,b| a["id"].as_str().cmp(&b["id"].as_str()));
        Ok(result)
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
    pub fn lookup(&self, asset: &str, input: &Path, track: u64, source: &Option<String>, target: &Option<String>) -> Option<Value> {
        let revision=revision(input).ok()?;
        let key=serde_json::to_vec(&(1,asset,input,&revision,track,source,target)).ok()?;
        let id:String=sha2::Sha256::digest(key).iter().map(|b|format!("{b:02x}")).collect();
        let exact=self.status(&id).ok();
        if exact.as_ref().is_some_and(|j|j["state"]=="ready"){return exact;}
        // Older OCR jobs were keyed as auto, then saved with the detected language.
        // Reuse those validated outputs rather than extracting/translating them again.
        for entry in fs::read_dir(&self.root).ok()?.flatten() {
            let path=entry.path();
            let Some(candidate)=path.file_stem().and_then(|s|s.to_str()).filter(|s|valid(s)) else{continue};
            if path.extension().is_none_or(|s|s!="json"){continue;}
            let Ok(job)=self.read(candidate) else{continue};
            let source_matches=source.as_deref()==Some("auto") || job.source==*source;
            let target_matches=if target.as_deref()==Some("auto") {job.target.is_some() && job.target!=job.source}else{job.target==*target};
            if job.asset_id==asset && job.input==input && job.revision==revision && job.track==track && source_matches && target_matches {
                if let Ok(status)=self.status(candidate) {if status["state"]=="ready" {return Some(status);}}
            }
        }
        exact
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
                .is_some_and(|s| ["mkv", "webm", "mp4", "m4v", "mov", "srt"].contains(&s.to_ascii_lowercase().as_str())),
            "Preparation supports Matroska, MP4 and SRT text subtitles"
        );
        ensure!(
            (source.as_deref()==Some("auto") && (target.is_none() || target.as_deref()==Some("auto"))) ||
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
                completed_cues: 0, total_cues: 0, extraction_method: None, unreadable_cues: 0,
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
            let _permit = loop {
                let permit=self.gate.acquire().await?;
                if 2-self.gate.available_permits()<=self.concurrency.load(std::sync::atomic::Ordering::Relaxed) {break permit;}
                drop(permit);
                tokio::time::sleep(Duration::from_millis(500)).await;
            };
            job.state = "running".into();
            self.save(&job)?;
            let original=if job.target.is_some() {
                self.lookup(&job.asset_id,&job.input,job.track,&job.source,&None)
                    .filter(|j|j["state"]=="ready")
                    .and_then(|j|self.artifact(j["id"].as_str()?).ok().map(|text|(j,text)))
            }else{None};
            let bitmap=if original.is_none() && job.input.extension().is_some_and(|e|e.eq_ignore_ascii_case("mkv") || e.eq_ignore_ascii_case("webm")) {
                let tracks=crate::native_subtitles::tracks(&job.input).await?;
                let selected=tracks["tracks"].as_array().and_then(|t|t.iter().find(|t|t["track"]==job.track)).context("Subtitle track missing")?;
                if selected["codec"]=="S_VOBSUB" || selected["codec"]=="S_HDMV/PGS" {
                    let sidecar_input=job.input.clone();
                    let sidecars=tokio::task::spawn_blocking(move ||crate::subtitles::discover(&sidecar_input)).await??;
                    ensure!(sidecars.is_empty() && !tracks["tracks"].as_array().unwrap().iter().any(|t|t["text_extractable"]==true),"Text subtitles are available; OCR is only a fallback");true
                } else {false}
            } else {false};
            let mut cues=if let Some((prior,text))=original {
                job.extraction_method=prior["extraction_method"].as_str().map(str::to_owned);
                job.unreadable_cues=prior["unreadable_cues"].as_u64().unwrap_or(0) as usize;
                // Our WebVTT serializer uses SRT-compatible timestamps and cue blocks.
                crate::subtitles::parse_srt(text.strip_prefix("WEBVTT\n\n").unwrap_or(&text))?
            }else if bitmap {
                job.extraction_method=Some("ocr".into());self.save(&job)?;
                let output=crate::ocr_subtitles::extract(&job.input,job.track,job.source.as_deref().unwrap_or("auto"),&self.root,&self.root.join(format!("{}.ocr-progress",job.id))).await?;
                job.source=Some(output.language);job.unreadable_cues=output.unreadable_cues;output.cues
            } else {job.extraction_method=Some("text".into());crate::native_subtitles::extract(&job.input,job.track).await?};
            if job.source.as_deref()==Some("auto") {job.source=Some(crate::ocr_subtitles::detect_language(&cues).await?);}
            if job.target.as_deref()==Some("auto") {job.target=Some(if job.source.as_deref()==Some("en") {"zh-Hans"}else{"en"}.into());}
            self.save(&job)?;
            let timings: Vec<_> = cues.iter().map(|cue| (cue.start_ms, cue.end_ms)).collect();
            ensure!(!cues.is_empty(), "No subtitle cues extracted");
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
                    let end=job.completed_cues+crate::subtitles::translation_batch_len(&cues[job.completed_cues..]);
                    ensure!(end > job.completed_cues, "Invalid translation batch");
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
            ensure!(cues.len() == timings.len() && cues.iter().zip(&timings).all(|(cue, timing)|
                (cue.start_ms, cue.end_ms) == *timing && cue.end_ms > cue.start_ms
                && !cue.text.trim().is_empty() && !cue.text.contains('\0') && cue.text.len() <= 16_384),
                "Subtitle validation failed: timing or text changed unexpectedly");
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
        let result = tokio::select! {
            result=tokio::time::timeout(Duration::from_secs(1200), result)=>Some(result),
            _=async {
                loop {
                    if self.urgent.load(std::sync::atomic::Ordering::Relaxed) {break;}
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }=>None,
        };
        job.updated = crate::storage::now();
        match result {
            None => {
                job.state="interrupted".into();
                job.error=None;
                tracing::info!(job=%job.id,"Subtitle work interrupted for playback preparation; queued for retry");
            }
            Some(Ok(Ok(()))) => {
                job.state = "ready".into();
                job.error = None;
            }
            Some(Ok(Err(error))) => {
                job.state = "failed".into();
                job.error = Some(error.to_string());
            }
            Some(Err(_)) => {
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

#[cfg(test)]
mod sidecar_job_tests {
    use super::*;
    #[test]
    fn detected_language_reuses_legacy_auto_keyed_artifact() -> Result<()> {
        let root=std::env::temp_dir().join(format!("torrbaby-legacy-caption-{}",uuid::Uuid::new_v4()));
        fs::create_dir_all(&root)?;let input=root.join("movie.mkv");fs::write(&input,b"fixture")?;
        let manager=Manager::new(&root)?;let id="a".repeat(64);
        let job=Job{id:id.clone(),asset_id:"movie".into(),input:input.clone(),revision:revision(&input)?,track:3,source:Some("en".into()),target:Some("zh-Hans".into()),state:"ready".into(),updated:0.0,error:None,completed_cues:1,total_cues:1,extraction_method:Some("ocr".into()),unreadable_cues:0};
        manager.save(&job)?;fs::write(manager.root.join(format!("{id}.vtt")),"WEBVTT\n\n1\n00:00:01.000 --> 00:00:02.000\n你好\n")?;
        assert_eq!(manager.lookup("movie",&input,3,&Some("en".into()),&Some("zh-Hans".into())).unwrap()["id"],id);
        assert!(manager.lookup("other",&input,3,&Some("en".into()),&Some("zh-Hans".into())).is_none());
        fs::write(&input,b"changed file")?;
        assert!(manager.lookup("movie",&input,3,&Some("en".into()),&Some("zh-Hans".into())).is_none());
        fs::remove_dir_all(root)?;Ok(())
    }
    #[tokio::test]
    async fn interrupted_job_is_requeued_and_can_complete()->Result<()> {
        let root=std::env::temp_dir().join(format!("torrbaby-interrupt-{}",uuid::Uuid::new_v4()));
        fs::create_dir_all(&root)?;let input=root.join("example.en.srt");
        fs::write(&input,"1\n00:00:01,000 --> 00:00:03,000\nHello world.\n")?;
        let manager=Manager::new(&root)?;manager.set_concurrency(0);manager.set_urgent(true);
        let job=manager.prepare("test",input.clone(),1,Some("en".into()),None)?;
        let id=job["id"].as_str().unwrap();
        for _ in 0..100 {if !manager.busy(){break;}tokio::time::sleep(Duration::from_millis(10)).await;}
        assert_eq!(manager.status(id)?["state"],"interrupted");
        manager.set_urgent(false);manager.set_concurrency(2);
        let resumed=manager.prepare("test",input,1,Some("en".into()),None)?;
        assert_eq!(resumed["id"],id);
        for _ in 0..100 {if !manager.busy(){break;}tokio::time::sleep(Duration::from_millis(10)).await;}
        assert_eq!(manager.status(id)?["state"],"ready");
        assert!(manager.artifact(id)?.contains("Hello world."));
        fs::remove_dir_all(root)?;Ok(())
    }
    #[tokio::test]
    async fn sidecar_jobs_preserve_timing_and_track_file_revisions() -> Result<()> {
        let root = std::env::temp_dir().join(format!("torrbaby-sidecar-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root)?;
        let input = root.join("example.en.srt");
        fs::write(&input, "1\n00:00:01,000 --> 00:00:03,000\nOur garden is quiet today.\n")?;
        let manager = Manager::new(&root)?;
        let job = manager.prepare("asset-test", input.clone(), 1, Some("en".into()), None)?;
        let id = job["id"].as_str().unwrap();
        for _ in 0..100 {
            if manager.status(id)?["state"] == "ready" { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let ready = manager.status(id)?;
        assert_eq!(ready["state"], "ready");
        assert_eq!(ready["sidecar"].as_str().unwrap().len(), 64);
        assert!(manager.artifact(id)?.contains("00:00:01.000 --> 00:00:03.000"));
        fs::write(&input, "changed source")?;
        assert_eq!(manager.status(id)?["state"], "stale");
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
