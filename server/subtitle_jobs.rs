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
    /// Assets the processing queue took a slot back from; their jobs stop now.
    yielded: Mutex<HashSet<String>>,
    /// Temporary external translation lane (outbox/inbox beside the subtitle cache).
    external: std::sync::atomic::AtomicBool,
    /// Job IDs per asset, so lookups do not read every job record on each call.
    index: Mutex<std::collections::HashMap<String, Vec<String>>>,
}
const MAX_CONCURRENCY: usize = 8;
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
        let mut index: std::collections::HashMap<String, Vec<String>> = Default::default();
        for entry in fs::read_dir(&root)?.flatten() {
            let path = entry.path();
            let Some(id) = path.file_stem().and_then(|s| s.to_str()).filter(|s| valid(s)) else { continue };
            if path.extension().is_none_or(|e| e != "json") { continue; }
            let Ok(bytes) = fs::read(&path) else { continue };
            if let Ok(job) = serde_json::from_slice::<Job>(&bytes) { index.entry(job.asset_id).or_default().push(id.to_owned()); }
        }
        for dir in ["outbox", "inbox"] { fs::create_dir_all(Self::fleet_dir(&root).join(dir))?; }
        Ok(Arc::new(Self {
            root,
            pending: Mutex::new(HashSet::new()),
            gate: tokio::sync::Semaphore::new(MAX_CONCURRENCY),
            concurrency: std::sync::atomic::AtomicUsize::new(1),
            urgent: std::sync::atomic::AtomicBool::new(false),
            yielded: Mutex::new(HashSet::new()),
            external: std::sync::atomic::AtomicBool::new(false),
            index: Mutex::new(index),
        }))
    }
    pub fn set_urgent(&self, urgent:bool) {
        self.urgent.store(urgent,std::sync::atomic::Ordering::Relaxed);
    }
    pub fn can_admit(&self)->bool {
        self.pending.lock().map(|p|p.len()<self.concurrency.load(std::sync::atomic::Ordering::Relaxed)).unwrap_or(false)
    }
    pub fn set_concurrency(&self, limit:usize) {
        self.concurrency.store(limit.min(MAX_CONCURRENCY),std::sync::atomic::Ordering::Relaxed);
    }
    pub fn set_external(&self, enabled:bool) {
        self.external.store(enabled,std::sync::atomic::Ordering::Relaxed);
    }
    pub fn external(&self)->bool { self.external.load(std::sync::atomic::Ordering::Relaxed) }
    /// Background jobs for `stop` assets are interrupted (and retried later);
    /// assets the queue selected again may run.
    pub fn yield_assets(&self, stop:&[String], resume:&[String]) {
        if let Ok(mut yielded)=self.yielded.lock() {
            for id in resume { yielded.remove(id); }
            for id in stop { yielded.insert(id.clone()); }
        }
    }
    fn fleet_dir(root:&Path)->PathBuf { root.parent().unwrap_or(root).join("translation-fleet") }
    /// Write an external translation request: text only, keyed by cue index.
    /// Timings stay in the server's private copy and are never round-tripped.
    fn export_external(&self, job:&Job, cues:&[crate::subtitles::TimedCue]) -> Result<()> {
        let private=serde_json::to_vec(&ProgressRef{completed:0,cues})?;
        fs::write(self.root.join(format!("{}.external.json",job.id)),private)?;
        let title=job.input.file_stem().and_then(|s|s.to_str()).unwrap_or("");
        let request=json!({"id":job.id,"asset_id":job.asset_id,"title":title,"source":job.source,"target":job.target,
            "cues":cues.iter().enumerate().map(|(i,c)|json!({"i":i,"text":c.text})).collect::<Vec<_>>()});
        let outbox=Self::fleet_dir(&self.root).join("outbox");
        let temporary=outbox.join(format!("{}.json.tmp",job.id));
        fs::write(&temporary,serde_json::to_vec(&request)?)?;
        fs::rename(temporary,outbox.join(format!("{}.json",job.id)))?;
        Ok(())
    }
    /// Accept translated cue text from the inbox as a completed translation
    /// checkpoint; the job then finishes through the normal validation path.
    /// Returns the asset IDs whose translations were accepted.
    pub fn import_external(&self) -> Result<Vec<String>> {
        let fleet=Self::fleet_dir(&self.root);
        let mut accepted=Vec::new();
        for entry in fs::read_dir(fleet.join("inbox"))?.flatten() {
            let path=entry.path();
            let Some(id)=path.file_name().and_then(|n|n.to_str()).and_then(|n|n.strip_suffix(".json")).filter(|id|valid(id)).map(str::to_owned) else {continue};
            match self.accept_external(&id,&path) {
                Ok(asset)=>{
                    let _=fs::remove_file(&path);
                    let _=fs::remove_file(fleet.join("outbox").join(format!("{id}.json")));
                    let _=fs::remove_file(self.root.join(format!("{id}.external.json")));
                    tracing::info!(job=id,asset,"External translation accepted");
                    accepted.push(asset);
                }
                Err(error)=>{
                    tracing::warn!(job=id,%error,"External translation rejected");
                    let _=fs::write(fleet.join("inbox").join(format!("{id}.rejected.txt")),error.to_string());
                    let _=fs::rename(&path,fleet.join("inbox").join(format!("{id}.rejected")));
                }
            }
        }
        Ok(accepted)
    }
    fn accept_external(&self, id:&str, path:&Path) -> Result<String> {
        let mut job=self.read(id)?;
        ensure!(job.state=="external","Job is not awaiting external translation");
        ensure!(revision(&job.input)?==job.revision,"Media changed since export");
        let bytes=fs::read(self.root.join(format!("{id}.external.json")))?;
        let original:Progress=serde_json::from_slice(&bytes)?;
        let incoming:Value=serde_json::from_slice(&fs::read(path)?)?;
        let translated=incoming["cues"].as_array().context("Missing cues")?;
        ensure!(translated.len()==original.cues.len(),"Expected {} cues, received {}",original.cues.len(),translated.len());
        let mut cues=original.cues;
        for (i,(cue,value)) in cues.iter_mut().zip(translated).enumerate() {
            ensure!(value["i"].as_u64()==Some(i as u64),"Cue {i} is out of order");
            let text=value["text"].as_str().context("Missing cue text")?.trim();
            ensure!(!text.is_empty() && text.len()<=16_384 && !text.contains('\0'),"Cue {i} text is invalid");
            cue.text=text.to_owned();
        }
        let checkpoint=serde_json::to_vec(&ProgressRef{completed:cues.len(),cues:&cues})?;
        ensure!(checkpoint.len()<=16_777_216,"Subtitle checkpoint too large");
        let temporary=self.root.join(format!("{id}.progress.tmp"));
        fs::write(&temporary,checkpoint)?;
        fs::rename(temporary,self.root.join(format!("{id}.progress")))?;
        // Re-admission resumes from the complete checkpoint and validates timing.
        job.state="interrupted".into();job.updated=crate::storage::now();
        self.save(&job)?;
        Ok(job.asset_id)
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
        if let Ok(mut index) = self.index.lock() {
            let ids = index.entry(job.asset_id.clone()).or_default();
            if !ids.contains(&job.id) { ids.push(job.id.clone()); }
        }
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
        let candidates=self.index.lock().ok()?.get(asset).cloned().unwrap_or_default();
        for candidate in candidates.iter().map(String::as_str) {
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
                pending.len() < 2 * MAX_CONCURRENCY,
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
        let asset=job.asset_id.clone();
        let result = async {
            let _permit = loop {
                let permit=self.gate.acquire().await?;
                if MAX_CONCURRENCY-self.gate.available_permits()<=self.concurrency.load(std::sync::atomic::Ordering::Relaxed) {break permit;}
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
                    ensure!(sidecars.is_empty() && !tracks["tracks"].as_array().into_iter().flatten().any(|t|t["text_extractable"]==true),"Text subtitles are available; OCR is only a fallback");true
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
                if self.external() && job.completed_cues < cues.len() {
                    self.export_external(&job,&cues)?;
                    return Ok(false);
                }
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
            Ok::<bool, anyhow::Error>(true)
        };
        let result = tokio::select! {
            result=tokio::time::timeout(Duration::from_secs(1200), result)=>Some(result),
            _=async {
                loop {
                    if self.urgent.load(std::sync::atomic::Ordering::Relaxed) {break;}
                    if self.yielded.lock().map_or(true,|y|y.contains(&asset)) {break;}
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
            Some(Ok(Ok(true))) => {
                job.state = "ready".into();
                job.error = None;
            }
            Some(Ok(Ok(false))) => {
                job.state = "external".into();
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
    async fn yielded_asset_job_is_interrupted_and_resumes()->Result<()> {
        let root=std::env::temp_dir().join(format!("torrbaby-yield-{}",uuid::Uuid::new_v4()));
        fs::create_dir_all(&root)?;let input=root.join("example.en.srt");
        fs::write(&input,"1\n00:00:01,000 --> 00:00:03,000\nHello world.\n")?;
        let manager=Manager::new(&root)?;manager.set_concurrency(0);
        manager.yield_assets(&["test".into()],&[]);
        let job=manager.prepare("test",input.clone(),1,Some("en".into()),None)?;
        let id=job["id"].as_str().unwrap().to_owned();
        for _ in 0..100 {if !manager.busy(){break;}tokio::time::sleep(Duration::from_millis(10)).await;}
        assert_eq!(manager.status(&id)?["state"],"interrupted");
        manager.yield_assets(&[],&["test".into()]);manager.set_concurrency(MAX_CONCURRENCY);
        manager.prepare("test",input,1,Some("en".into()),None)?;
        for _ in 0..100 {if !manager.busy(){break;}tokio::time::sleep(Duration::from_millis(10)).await;}
        assert_eq!(manager.status(&id)?["state"],"ready");
        fs::remove_dir_all(root)?;Ok(())
    }
    #[tokio::test]
    async fn external_translation_round_trip_keeps_server_timing()->Result<()> {
        let root=std::env::temp_dir().join(format!("torrbaby-external-{}",uuid::Uuid::new_v4()));
        fs::create_dir_all(&root)?;let input=root.join("example.en.srt");
        fs::write(&input,"1\n00:00:01,000 --> 00:00:03,000\nHello.\n\n2\n00:00:04,000 --> 00:00:06,500\nGoodbye.\n")?;
        let manager=Manager::new(&root)?;manager.set_concurrency(2);manager.set_external(true);
        let wait=|m:&Arc<Manager>|{let m=m.clone();async move {for _ in 0..200 {if !m.busy(){break;}tokio::time::sleep(Duration::from_millis(10)).await;}}};
        let job=manager.prepare("test",input.clone(),1,Some("en".into()),Some("zh-Hans".into()))?;
        let id=job["id"].as_str().unwrap().to_owned();
        wait(&manager).await;
        assert_eq!(manager.status(&id)?["state"],"external");
        let fleet=root.join("translation-fleet");
        let request:Value=serde_json::from_slice(&fs::read(fleet.join("outbox").join(format!("{id}.json")))?)?;
        assert_eq!(request["cues"],json!([{"i":0,"text":"Hello."},{"i":1,"text":"Goodbye."}]));
        assert!(request.to_string().find("00:00").is_none(),"timings never leave the server");
        // A wrong cue count is rejected and the job keeps waiting.
        fs::write(fleet.join("inbox").join(format!("{id}.json")),json!({"cues":[{"i":0,"text":"你好。"}]}).to_string())?;
        assert!(manager.import_external()?.is_empty());
        assert_eq!(manager.status(&id)?["state"],"external");
        assert!(fleet.join("inbox").join(format!("{id}.rejected")).exists());
        fs::write(fleet.join("inbox").join(format!("{id}.json")),json!({"cues":[{"i":0,"text":"你好。"},{"i":1,"text":"再见。"}]}).to_string())?;
        assert_eq!(manager.import_external()?,vec!["test".to_owned()]);
        assert!(!fleet.join("outbox").join(format!("{id}.json")).exists());
        manager.prepare("test",input,1,Some("en".into()),Some("zh-Hans".into()))?;
        wait(&manager).await;
        assert_eq!(manager.status(&id)?["state"],"ready");
        let vtt=manager.artifact(&id)?;
        assert!(vtt.contains("00:00:04.000 --> 00:00:06.500") && vtt.contains("再见。") && !vtt.contains("Goodbye"));
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
