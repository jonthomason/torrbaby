//! Revision-bound HLS packages. Cache is disposable; media originals are never changed.
use crate::storage::now;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
const LIMIT: u64 = 32 * 1024 * 1024 * 1024;
#[derive(Clone)]
struct Job {
    id: String,
    asset: String,
    input: PathBuf,
    revision: String,
    subtitles: Vec<String>,
    state: String,
    error: Option<String>,
    expires: u64,
    budget: u64,
}
pub struct Caption {
    pub id: String,
    pub language: String,
    pub text: String,
    /// Rendition name shown in the player's subtitle menu.
    pub name: String,
    pub sdh: bool,
}
pub struct Manager {
    root: PathBuf,
    jobs: Mutex<HashMap<String, Job>>,
}
fn revision(path: &Path) -> Result<String> {
    let m = path.metadata()?;
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
    uuid::Uuid::parse_str(id).is_ok() && id.len() == 36
}
impl Manager {
    pub fn new(data: &Path) -> Result<Arc<Self>> {
        let root = data.join("hls-cache");
        fs::create_dir_all(&root)?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        // No leases survive restart. Only remove directories in our dedicated cache.
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            if valid(&entry.file_name().to_string_lossy()) && entry.file_type()?.is_dir() {
                fs::remove_dir_all(entry.path())?;
            }
        }
        Ok(Arc::new(Self {
            root,
            jobs: Mutex::new(HashMap::new()),
        }))
    }
    fn job(&self, id: &str) -> Result<Job> {
        ensure!(valid(id), "Invalid HLS job");
        let job = self
            .jobs
            .lock()
            .map_err(|_| anyhow::anyhow!("HLS cache unavailable"))?
            .get(id)
            .cloned()
            .context("HLS package expired; prepare again")?;
        ensure!(job.expires > now() as u64, "HLS package expired");
        ensure!(
            revision(&job.input)? == job.revision,
            "Media changed; prepare again"
        );
        Ok(job)
    }
    /// A file moved to another drive, its copy verified: its packages read the copy, so
    /// playback under way goes on.
    pub fn relocate(&self, asset: &str, from: &Path, to: &Path) {
        let Ok(revision) = revision(to) else { return };
        let Ok(mut jobs) = self.jobs.lock() else { return };
        for job in jobs.values_mut().filter(|j| j.asset == asset && j.input == from) {
            job.input = to.to_path_buf();
            job.revision = revision.clone();
        }
    }
    pub fn purge_assets(&self, assets: &std::collections::BTreeSet<String>) -> Result<()> {
        let mut jobs = self.jobs.lock().map_err(|_| anyhow::anyhow!("HLS cache unavailable"))?;
        ensure!(!jobs.values().any(|j| assets.contains(&j.asset) && j.state == "preparing"), "Waiting for playback preparation to finish");
        let ids: Vec<_> = jobs.values().filter(|j| assets.contains(&j.asset)).map(|j|j.id.clone()).collect();
        for id in ids {
            ensure!(valid(&id), "Invalid cache identity");
            let path = self.root.join(&id);
            match fs::symlink_metadata(&path) {
                Ok(info) => {
                    ensure!(info.is_dir() && !info.file_type().is_symlink(), "Unsafe HLS cache path");
                    fs::remove_dir_all(path)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(error) => return Err(error.into()),
            }
            jobs.remove(&id);
        }
        Ok(())
    }
    pub fn status(&self, id: &str) -> Result<Value> {
        let j = self.job(id)?;
        Ok(
            json!({"id":j.id,"asset_id":j.asset,"state":j.state,"error":j.error,"expires":j.expires}),
        )
    }
    pub fn asset(&self, id: &str) -> Result<String> {
        Ok(self.job(id)?.asset)
    }
    pub fn file(&self, id: &str, name: &str) -> Result<PathBuf> {
        ensure!(self.job(id)?.state == "ready", "HLS package is not ready");
        ensure!(
            !name.is_empty()
                && name.split('/').all(|p| !p.is_empty()
                    && p != "."
                    && p != ".."
                    && p.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))),
            "Invalid HLS path"
        );
        let base = self.root.join(id).canonicalize()?;
        let path = base.join(name).canonicalize()?;
        ensure!(
            path.starts_with(&base) && path.is_file(),
            "Invalid HLS file"
        );
        Ok(path)
    }
    pub fn prepare(
        self: &Arc<Self>,
        asset: &str,
        input: PathBuf,
        captions: Vec<Caption>,
    ) -> Result<Value> {
        ensure!(captions.len() <= 8, "Too many subtitle tracks");
        let revision = revision(&input)?;
        let subtitle_ids: Vec<_> = captions.iter().map(|c| c.id.clone()).collect();
        let mut jobs = self
            .jobs
            .lock()
            .map_err(|_| anyhow::anyhow!("HLS cache unavailable"))?;
        let expired: Vec<_> = jobs
            .values()
            .filter(|j| j.expires <= now() as u64 && j.state != "preparing")
            .map(|j| j.id.clone())
            .collect();
        for id in expired {
            if self.root.join(&id).exists() {
                fs::remove_dir_all(self.root.join(&id))?;
            }
            jobs.remove(&id);
        }
        if let Some(j) = jobs.values().find(|j| {
            j.asset == asset
                && j.revision == revision
                && j.subtitles == subtitle_ids
                && j.state != "failed"
        }) {
            let id = j.id.clone();
            drop(jobs);
            return self.status(&id);
        }
        ensure!(
            jobs.len() < 64,
            "Too many HLS packages; try again after existing packages expire"
        );
        ensure!(
            !jobs.values().any(|j| j.state == "preparing"),
            "Media preparation is busy; try again shortly"
        );
        let budget = input
            .metadata()?
            .len()
            .saturating_mul(2)
            .saturating_add(256 * 1024 * 1024)
            .max(1_048_576);
        ensure!(
            budget <= LIMIT
                && jobs
                    .values()
                    .map(|j| j.budget)
                    .sum::<u64>()
                    .saturating_add(budget)
                    <= LIMIT,
            "HLS cache is full; existing playback expires after six hours"
        );
        let id = uuid::Uuid::new_v4().to_string();
        let job = Job {
            id: id.clone(),
            asset: asset.into(),
            input,
            revision,
            subtitles: subtitle_ids,
            state: "preparing".into(),
            error: None,
            expires: now() as u64 + 21600,
            budget,
        };
        jobs.insert(id.clone(), job.clone());
        drop(jobs);
        let manager = self.clone();
        tokio::spawn(async move {
            let output = manager.root.join(&job.id);
            let result = async {
                let helper = std::env::current_exe()?.with_file_name("torrbaby-media-segmenter");
                crate::media_packaging::package(
                    &job.input,
                    &output,
                    &helper,
                    job.budget - 129 * 1024 * 1024,
                )
                .await?;
                attach_captions(&output, &captions)?;
                ensure!(
                    revision_of(&job.input)? == job.revision,
                    "Media changed during packaging"
                );
                Ok::<(), anyhow::Error>(())
            }
            .await;
            let cleaned = if result.is_err() {
                !output.exists() || fs::remove_dir_all(&output).is_ok()
            } else {
                false
            };
            if let Ok(mut jobs) = manager.jobs.lock() {
                if let Some(j) = jobs.get_mut(&job.id) {
                    j.state = if result.is_ok() { "ready" } else { "failed" }.into();
                    j.error = result.err().map(|e| e.to_string());
                    if j.state == "failed" && cleaned {
                        j.budget = 0;
                    }
                }
            }
        });
        self.status(&id)
    }
}
fn revision_of(path: &Path) -> Result<String> {
    revision(path)
}
fn attach_captions(root: &Path, captions: &[Caption]) -> Result<()> {
    if captions.is_empty() {
        return Ok(());
    }
    let master = fs::read_to_string(root.join("media.m3u8"))?;
    let primary = master
        .lines()
        .filter(|s| !s.starts_with('#') && !s.is_empty())
        .last()
        .context("Missing HLS primary track")?;
    let media = fs::read_to_string(root.join(primary))?;
    let duration: f64 = media
        .lines()
        .filter_map(|s| s.strip_prefix("#EXTINF:"))
        .map(|s| s.trim_end_matches(',').parse::<f64>())
        .collect::<std::result::Result<Vec<_>, _>>()?
        .iter()
        .sum();
    ensure!(
        duration.is_finite() && duration > 0.0,
        "Invalid HLS duration"
    );
    let mut declarations = String::new();
    for (i, c) in captions.iter().enumerate() {
        ensure!(
            ["en", "zh-Hans", "und"].contains(&c.language.as_str())
                && c.text.starts_with("WEBVTT\n")
                && c.text.len() <= 16_777_216,
            "Invalid caption rendition"
        );
        let vtt = c.text.replacen(
            "WEBVTT\n",
            "WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:0\n",
            1,
        );
        fs::write(root.join(format!("subtitle-{i}.vtt")), vtt)?;
        fs::write(
            root.join(format!("subtitle-{i}.m3u8")),
            format!(
                "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:{}\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXTINF:{duration:.6},\nsubtitle-{i}.vtt\n#EXT-X-ENDLIST\n",
                duration.ceil() as u64
            ),
        )?;
        // Names must be unique within the group; repeat a name only with a number.
        let mut name: String = c.name.chars().filter(|ch| !ch.is_control() && *ch != '"').collect();
        if name.trim().is_empty() { name = crate::subtitle_jobs::track_name(&c.language, "text", false); }
        if captions[..i].iter().any(|other| other.name == c.name) { name = format!("{name} {}", i + 1); }
        declarations += &format!(
            "#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subtitles\",NAME=\"{name}\",LANGUAGE=\"{}\",DEFAULT=NO,AUTOSELECT=YES,{}URI=\"subtitle-{i}.m3u8\"\n",
            c.language,
            if c.sdh { format!("CHARACTERISTICS=\"{}\",", crate::virtual_captions::SDH_CHARACTERISTICS.join(",")) } else { String::new() }
        );
    }
    let mut updated = String::new();
    for line in master.lines() {
        if line.starts_with("#EXT-X-STREAM-INF:") {
            updated += &declarations;
            updated += line;
            updated += ",SUBTITLES=\"subtitles\"\n";
        } else {
            updated += line;
            updated.push('\n');
        }
    }
    fs::write(root.join("media.m3u8"), updated)?;
    Ok(())
}
/// Every relative playlist reference inherits this package's signed lease.
pub fn signed_playlist(text: &str, query: &str) -> String {
    text.lines()
        .map(|line| {
            if !line.starts_with('#') && !line.is_empty() {
                format!("{line}?{query}")
            } else if let Some((before, after)) = line.split_once("URI=\"") {
                if let Some((uri, tail)) = after.split_once('"') {
                    format!("{before}URI=\"{uri}?{query}\"{tail}")
                } else {
                    line.into()
                }
            } else {
                line.into()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}
