use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Deserialize, Serialize)]
pub struct Config {
    pub listen_host: String,
    pub listen_port: u16,
    #[serde(default)]
    pub lan_listen_host: Option<String>,
    #[serde(default)]
    pub trusted_lan_cidrs: Vec<String>,
    #[serde(default)]
    pub advertise_lan: bool,
    #[serde(default = "background_subtitles_default")]
    pub background_subtitles: bool,
    /// Files processed at once while nothing is playing, when translation is external (1-8).
    #[serde(default = "processing_concurrency_default")]
    pub processing_concurrency: usize,
    /// Files processed at once with on-device translation (1-8). macOS runs every
    /// translation through one system service, so more files only queue behind it;
    /// two keep one translating while another reads or scans.
    #[serde(default = "on_device_processing_concurrency_default")]
    pub on_device_processing_concurrency: usize,
    /// Temporary: hand translation to an external outbox/inbox instead of the
    /// on-device translator. Off restores on-device translation for leftovers.
    #[serde(default)]
    pub external_translation: bool,
    pub token_file: String,
    pub state_file: String,
    pub transmission_url: String,
    pub transmission_auth_file: Option<String>,
    /// Drive 1's folders when there is no `drives` list. Empty when there is one.
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub movies: String,
    #[serde(default)]
    pub tv: String,
    /// The drives media is kept on (docs/disk-aggregation.md). Absent: `source`,
    /// `movies` and `tv` are the only drive. Ask `crate::drives`, not these fields.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drives: Vec<DriveConfig>,
    pub poll_seconds: f64,
    #[serde(default)]
    pub paused: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DriveConfig {
    /// Stable identity, recorded on the drive; never derived from its mount path.
    pub id: String,
    /// The drive's `Media` folder, holding `Downloads`, `Movies` and `TV`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<String>,
    /// Exact folders, overriding the ones under `media`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloads: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub movies: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tv: Option<String>,
    /// False keeps new downloads off this drive (full, or being retired).
    #[serde(default = "accept_new_default")]
    pub accept_new: bool,
    /// Free space never to place below; default 50 GB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserve_bytes: Option<u64>,
}

pub fn expand(value: &str) -> Result<PathBuf> {
    if let Some(rest) = value.strip_prefix("~/") {
        Ok(PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(rest))
    } else {
        Ok(PathBuf::from(value))
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let config: Self = serde_json::from_slice(&fs::read(path).context("Read configuration")?)
            .context("Invalid configuration")?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        let config = self;
        for range in &config.trusted_lan_cidrs {
            crate::lan::network(range)?;
        }
        if let Some(host) = &config.lan_listen_host {
            let ip: std::net::Ipv4Addr = host
                .parse()
                .context("LAN listener must be an IPv4 address")?;
            if !ip.is_private() && !ip.is_loopback() {
                bail!("LAN listener must use a private local address");
            }
        }
        if config.advertise_lan && config.lan_listen_host.is_none() {
            bail!("LAN advertisement needs a LAN listener");
        }
        if !(1..=8).contains(&config.processing_concurrency) {
            bail!("processing_concurrency must be between 1 and 8");
        }
        if !(1..=8).contains(&config.on_device_processing_concurrency) {
            bail!("on_device_processing_concurrency must be between 1 and 8");
        }
        if !config.poll_seconds.is_finite() || !(5.0..=3600.0).contains(&config.poll_seconds) {
            bail!("poll_seconds must be between 5 and 3600");
        }
        if config.drives.is_empty() {
            if [&config.source, &config.movies, &config.tv].iter().any(|r| r.is_empty()) {
                bail!("Configure source, movies and tv, or a drives list");
            }
        } else {
            if [&config.source, &config.movies, &config.tv].iter().any(|r| !r.is_empty()) {
                bail!("Media folders are set per drive when a drives list is configured");
            }
            let mut ids = std::collections::HashSet::new();
            for drive in &config.drives {
                if drive.id.is_empty() || drive.id.len() > 64
                    || !drive.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
                    bail!("Drive ids must be 1-64 letters, digits, '-' or '_'");
                }
                if !ids.insert(drive.id.as_str()) { bail!("Drive ids must be unique"); }
                if let Some(media) = &drive.media { absolute(media)?; }
            }
        }
        let drives = crate::drives::Drives::of(config);
        let roots: Vec<&Path> = drives.all_roots().map(|(_, _, root)| root).collect();
        for (index, path) in roots.iter().enumerate() {
            absolute(&path.to_string_lossy())?;
            for other in roots.iter().skip(index + 1) {
                if path.starts_with(other) || other.starts_with(path) {
                    bail!("Media folders must not overlap");
                }
            }
        }
        Ok(())
    }

    pub fn token(&self) -> Result<String> {
        let token = fs::read_to_string(expand(&self.token_file)?).context("Read API token")?;
        let token = token.trim();
        if token.len() < 32 {
            bail!("API token must be at least 32 characters");
        }
        Ok(token.to_owned())
    }
}

fn absolute(folder: &str) -> Result<()> {
    let path = Path::new(folder);
    if !path.is_absolute() || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        bail!("Media folders must be absolute paths without parent components");
    }
    Ok(())
}

fn accept_new_default() -> bool { true }
fn background_subtitles_default() -> bool { true }
fn processing_concurrency_default() -> usize { 8 }
fn on_device_processing_concurrency_default() -> usize { 2 }
