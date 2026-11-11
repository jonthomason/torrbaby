use crate::config::{Config, expand};
use anyhow::{Context, Result, bail};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::Mutex;

const ADD_UNCONFIRMED: &str = "Transmission has not confirmed the add yet. The torrent may still appear in Torrents; check there before trying again.";

#[derive(Debug)]
struct UnconfirmedAdd;
impl std::fmt::Display for UnconfirmedAdd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(ADD_UNCONFIRMED) }
}
impl std::error::Error for UnconfirmedAdd {}

fn request_error(method: &str, error: reqwest::Error) -> anyhow::Error {
    // A timeout or lost response does not mean a mutation failed.
    if method == "torrent_add" && (!error.is_connect() || error.is_timeout()) {
        return UnconfirmedAdd.into();
    }
    if error.is_timeout() { anyhow::anyhow!("Transmission took too long to respond. Please try again.") }
    else if error.is_connect() { anyhow::anyhow!("Cannot reach Transmission") }
    else { anyhow::anyhow!("The connection to Transmission was interrupted. Please try again.") }
}

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

    /// A unique label proves which add completed after its response was lost.
    /// Never replay a timed-out mutation or infer success from a similar title.
    pub async fn add(&self, mut params: Value) -> Result<Value> {
        let marker = format!("torrbaby-add-{}", uuid::Uuid::new_v4());
        let original_labels = params["labels"].as_array().cloned().unwrap_or_default();
        let mut labels = original_labels.clone();
        labels.push(json!(marker));
        params["labels"] = json!(labels);
        let result = match self.call("torrent_add", params).await {
            Ok(result) => result,
            Err(error) if error.is::<UnconfirmedAdd>() => {
                let mut recovered = None;
                // Keep recovery below clients' request deadlines, including a busy RPC lock.
                for attempt in 0..3 {
                    if attempt > 0 { tokio::time::sleep(Duration::from_secs(1)).await; }
                    if let Ok(Ok(state)) = tokio::time::timeout(Duration::from_secs(3), self.call("torrent_get", json!({"fields":["id","name","hash_string","labels"]}))).await {
                        recovered = state["torrents"].as_array().and_then(|items| items.iter().find(|item| item["labels"].as_array().is_some_and(|labels| labels.iter().any(|label| label.as_str() == Some(&marker))))).cloned();
                        if recovered.is_some() { break; }
                    }
                }
                match recovered { Some(torrent) => json!({"torrent_added":torrent}), None => return Err(error) }
            }
            Err(error) => return Err(error),
        };
        // Duplicates are not relabeled; preserve their existing labels.
        if let Some(hash) = result["torrent_added"]["hash_string"].as_str() {
            let _ = tokio::time::timeout(Duration::from_secs(2), self.call("torrent_set", json!({"ids":[hash],"labels":original_labels}))).await;
        }
        Ok(result)
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
                .map_err(|error| request_error(method, error))?;
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
                .map_err(|error| if method == "torrent_add" { anyhow::Error::from(UnconfirmedAdd) } else { anyhow::Error::new(error).context("Invalid Transmission response") })?;
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
    "seed_idle_limit",
    "seed_idle_mode",
    "added_date",
    "done_date",
    "activity_date",
    "peers_connected",
    "peers_sending_to_us",
    "downloaded_ever",
    "uploaded_ever",
    "labels",
    "is_stalled",
    "is_finished",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

    async fn lost_response_case(visible: bool) {
        let adds = Arc::new(AtomicUsize::new(0));
        let labels = Arc::new(Mutex::new(json!([])));
        let app = axum::Router::new().route("/", axum::routing::post({
            let adds=adds.clone(); let labels=labels.clone();
            move |axum::Json(request): axum::Json<Value>| {
                let adds=adds.clone(); let labels=labels.clone();
                async move {
                    match request["method"].as_str().unwrap() {
                        "torrent_add" => { adds.fetch_add(1,Ordering::SeqCst); *labels.lock().await=request["params"]["labels"].clone(); if visible { tokio::time::sleep(Duration::from_millis(100)).await; } "truncated response".to_string() }
                        "torrent_get" => json!({"result":{"torrents":if visible {json!([{"hash_string":"abc","name":"Exact request","labels":labels.lock().await.clone()}])} else {json!([{ "hash_string":"other","name":"Exact request","labels":[] }])}}}).to_string(),
                        "torrent_set" => { *labels.lock().await=request["params"]["labels"].clone(); json!({"result":{}}).to_string() }
                        _ => panic!("Unexpected RPC"),
                    }
                }
            }
        }));
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address=listener.local_addr().unwrap();
        let server=tokio::spawn(async move { axum::serve(listener,app).await.unwrap(); });
        let rpc=Transmission {client:Client::builder().timeout(Duration::from_millis(30)).build().unwrap(),url:format!("http://{address}/"),credentials:None,session:Mutex::new(String::new())};
        let result=rpc.add(json!({"filename":"magnet:?xt=urn:btih:test","labels":["existing"]})).await;
        assert_eq!(adds.load(Ordering::SeqCst),1,"Never replay an unconfirmed add");
        if visible {
            assert_eq!(result.unwrap()["torrent_added"]["hash_string"],"abc");
            assert_eq!(*labels.lock().await,json!(["existing"]));
        } else { assert_eq!(result.unwrap_err().to_string(),ADD_UNCONFIRMED); }
        server.abort();
    }

    #[tokio::test]
    async fn recovers_exact_add_after_lost_response() { lost_response_case(true).await; }
    #[tokio::test]
    async fn does_not_guess_success_or_repeat_add() { lost_response_case(false).await; }
}
