use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Deserialize)]
pub struct Config {
    pub listen_host: String,
    pub listen_port: u16,
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
        if !config.poll_seconds.is_finite() || config.poll_seconds < 5.0 {
            bail!("poll_seconds must be at least 5");
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
        Ok(config)
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
