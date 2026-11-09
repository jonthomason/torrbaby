use crate::config::{Config, expand};
use anyhow::{Context, Result, bail};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::Mutex;

pub struct Transmission {
    client: Client,
    url: String,
    credentials: Option<(String, String)>,
    session: Mutex<String>,
}

impl Transmission {
    pub fn new(config: &Config) -> Result<Self> {
        let credentials = config
            .transmission_auth_file
            .as_ref()
            .map(|path| -> Result<_> {
                let value = std::fs::read_to_string(expand(path)?)
                    .context("Read Transmission credentials")?;
                let (user, password) = value
                    .trim()
                    .split_once(':')
                    .context("Invalid Transmission credentials")?;
                Ok((user.to_owned(), password.to_owned()))
            })
            .transpose()?;
        Ok(Self {
            client: Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            url: config.transmission_url.clone(),
            credentials,
            session: Mutex::new(String::new()),
        })
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let mut session = self.session.lock().await;
        for _ in 0..2 {
            let mut request = self
                .client
                .post(&self.url)
                .json(&json!({"jsonrpc":"2.0", "method":method, "params":params, "id":1}));
            if !session.is_empty() {
                request = request.header("X-Transmission-Session-Id", &*session);
            }
            if let Some((user, password)) = &self.credentials {
                request = request.basic_auth(user, Some(password));
            }
            let response = request
                .send()
                .await
                .map_err(|_| anyhow::anyhow!("Cannot reach Transmission"))?;
            if response.status() == StatusCode::CONFLICT {
                *session = response
                    .headers()
                    .get("X-Transmission-Session-Id")
                    .context("Missing Transmission session ID")?
                    .to_str()?
                    .to_owned();
                continue;
            }
            if !response.status().is_success() {
                bail!("Transmission HTTP {}", response.status().as_u16());
            }
            let mut result: Value = response
                .json()
                .await
                .context("Invalid Transmission response")?;
            if result.get("error").is_some() {
                bail!("Transmission rejected {method}");
            }
            return result
                .get_mut("result")
                .map(Value::take)
                .context("Missing Transmission result");
        }
        bail!("Transmission session negotiation failed")
    }
}

pub const FIELDS: &[&str] = &[
    "hash_string",
    "name",
    "status",
    "error",
    "error_string",
    "percent_done",
    "metadata_percent_complete",
    "left_until_done",
    "download_dir",
    "files",
    "file_stats",
    "rate_download",
    "rate_upload",
    "eta",
    "upload_ratio",
    "seed_ratio_limit",
    "seed_ratio_mode",
    "added_date",
    "activity_date",
    "peers_connected",
    "peers_sending_to_us",
    "downloaded_ever",
    "uploaded_ever",
    "labels",
    "is_stalled",
];
