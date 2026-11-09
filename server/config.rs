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
    pub token_file: String,
    pub state_file: String,
    pub transmission_url: String,
    pub transmission_auth_file: Option<String>,
    pub source: String,
    pub movies: String,
    pub tv: String,
    pub poll_seconds: f64,
    #[serde(default)]
    pub paused: bool,
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
        if !config.poll_seconds.is_finite() || !(5.0..=3600.0).contains(&config.poll_seconds) {
            bail!("poll_seconds must be between 5 and 3600");
        }
        let roots = [&config.source, &config.movies, &config.tv];
        for (index, root) in roots.iter().enumerate() {
            let path = Path::new(root);
            if !path.is_absolute()
                || path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                bail!("Media folders must be absolute paths without parent components");
            }
            for other in roots.iter().skip(index + 1) {
                if path.starts_with(other) || Path::new(other).starts_with(path) {
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

fn background_subtitles_default() -> bool { true }
