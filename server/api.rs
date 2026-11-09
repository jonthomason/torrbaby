use crate::{
    config::{Config, expand},
    storage::{Database, now},
    transmission::Transmission,
};
use anyhow::{Context, Result, bail};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, OriginalUri, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

pub struct Service {
    config: Config,
    token: String,
    db: Mutex<Database>,
    rpc: Transmission,
    started: Instant,
}

// Keep unimplemented routes explicit during the port; never silently report success.
pub async fn serve(config_path: PathBuf, database: PathBuf, listen: String) -> Result<()> {
    let config = Config::load(&config_path)?;
    let live = expand(&config.state_file)?.with_extension("sqlite3");
    if database == live
        || (database.exists()
            && live.exists()
            && database.canonicalize()? == live.canonicalize()?)
    {
        bail!("The migration preview must use a copy of the live database");
    }
    let service = Arc::new(Service {
        token: config.token()?,
        rpc: Transmission::new(&config)?,
        db: Mutex::new(Database::open(&database, None)?),
        config,
        started: Instant::now(),
    });
    let router = Router::new()
        .fallback(any(dispatch))
        .layer(DefaultBodyLimit::max(65536))
        .with_state(service);
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    eprintln!(
        "Torrbaby Rust migration preview listening on {}; organizer disabled",
        listener.local_addr()?
    );
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

async fn dispatch(
    State(s): State<Arc<Service>>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    if uri.path().starts_with("/v1/stream/") {
        return match stream(&s, &uri, &method, &headers).await {
            Ok(response) => response,
            Err(error) => (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":error.to_string()})),
            )
                .into_response(),
        };
    }
    let supplied = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    // Hash both strings then use constant-time verification supplied by the crypto crate.
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut expected = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(s.token.as_bytes())
        .expect("HMAC accepts any key size");
    expected.update(format!("Bearer {}", s.token).as_bytes());
    let tag = expected.finalize().into_bytes();
    let mut actual = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(s.token.as_bytes())
        .expect("HMAC accepts any key size");
    actual.update(supplied.as_bytes());
    if actual.verify_slice(&tag).is_err() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"Unauthorized"})),
        )
            .into_response();
    }
    if headers.contains_key("origin") {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"Browser cross-origin requests are disabled"})),
        )
            .into_response();
    }
    let body = if bytes.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) if value.is_object() => value,
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":"Expected a JSON object"})),
                )
                    .into_response();
            }
        }
    };
    match handle(&s, &method, uri.path(), body).await {
        Ok(value) => ([("Cache-Control", "no-store")], Json(value)).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":error.to_string()})),
        )
            .into_response(),
    }
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .with_context(|| format!("{key} must be text"))
}

async fn handle(s: &Service, method: &Method, path: &str, body: Value) -> Result<Value> {
    if path == "/v1/seeding" {
        if method == Method::PATCH {
            let limit = body["ratio"].as_f64().context("ratio must be a number")?;
            let enabled = body["enabled"]
                .as_bool()
                .context("enabled must be boolean")?;
            if !limit.is_finite() || limit <= 0.0 || limit > 1000.0 {
                bail!("Ratio must be greater than zero and at most 1000");
            }
            s.rpc
                .call(
                    "session_set",
                    json!({"seed_ratio_limit":limit,"seed_ratio_limited":enabled}),
                )
                .await?;
        } else if method != Method::GET {
            bail!("Unsupported method");
        }
        let result = s
            .rpc
            .call(
                "session_get",
                json!({"fields":["seed_ratio_limit","seed_ratio_limited"]}),
            )
            .await?;
        return Ok(
            json!({"ratio":result["seed_ratio_limit"],"enabled":result["seed_ratio_limited"]}),
        );
    }
    if method == Method::POST {
        if let Some(hash) = path
            .strip_prefix("/v1/torrents/")
            .and_then(|s| s.strip_suffix("/play"))
        {
            return playback_link(s, hash, &body).await;
        }
    }
    if method == Method::GET && path == "/v1/torrents" {
        return Ok(s
            .rpc
            .call("torrent_get", json!({"fields":crate::transmission::FIELDS}))
            .await?["torrents"]
            .take());
    }
    // No lock is held across an await. SQLite operations remain short and transactional.
    let mut db =
        s.db.lock()
            .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
    match (method.as_str(),path) {
        ("GET","/v1/status") => {
            let state = db.state()?;
            Ok(json!({"version":env!("CARGO_PKG_VERSION"),"uptime_seconds":s.started.elapsed().as_secs(),
                "settings":{"source":s.config.source,"movies":s.config.movies,"tv":s.config.tv,"poll_seconds":s.config.poll_seconds,"paused":true},
                "last_poll":null,"last_error":"Migration preview: organizer not enabled","pending_scans":0,"scan_error":null,"jobs":state["jobs"]}))
        }
        ("GET","/v1/events") => Ok(db.state()?["events"].take()),
        ("GET","/v1/clients") => Ok(json!(db.rows("SELECT * FROM clients ORDER BY last_seen DESC",[])?)),
        ("POST","/v1/clients") => db.register(text(&body,"client_id")?,text(&body,"name")?,text(&body,"platform")?),
        ("GET","/v1/playback") => Ok(json!(db.history()?)),
        ("POST","/v1/playback/progress") => db.progress(text(&body,"session_id")?,body["sequence"].as_i64().context("Invalid sequence")?,body["position"].as_f64().context("Invalid position")?,body["duration"].as_f64().context("Invalid duration")?, match body.get("ended") {None => false,Some(v)=>v.as_bool().context("ended must be boolean")?}),
        ("GET","/v1/rates") => Ok(json!(db.rows("SELECT CAST(time/60 AS INTEGER)*60 AS time,AVG(download) AS download,AVG(upload) AS upload FROM transfer_rates WHERE time>=? GROUP BY CAST(time/60 AS INTEGER) ORDER BY time",[now()-86400.0])?)),
        ("GET","/v1/metadata") => {
            let records = db.rows("SELECT document FROM metadata",[])?;
            let values: Result<Vec<Value>> = records.iter().map(|r| Ok(serde_json::from_str(text(r,"document")?)?)).collect();
            Ok(json!(values?))
        }
        ("PATCH",p) if p.starts_with("/v1/clients/") => db.rename_client(&p[12..],text(&body,"name")?),
        _ => bail!("Route not yet available in the Rust migration preview"),
    }
}

fn signature(token: &str, hash: &str, index: u64, expires: u64) -> String {
    use hmac::{Hmac, Mac};
    let mut mac =
        <Hmac<sha2::Sha256> as hmac::KeyInit>::new_from_slice(token.as_bytes()).expect("HMAC key");
    mac.update(format!("{hash}:{index}:{expires}").as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn checked_path(root: &std::path::Path, relative: &str) -> Result<PathBuf> {
    use std::path::Component;
    let relative = std::path::Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!("Invalid media path");
    }
    let mut path = root.to_path_buf();
    for part in relative.components() {
        path.push(part);
        if std::fs::symlink_metadata(&path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            bail!("Symlinks are not served");
        }
    }
    Ok(path)
}

async fn media_file(s: &Service, hash: &str, index: u64) -> Result<PathBuf> {
    if hash.len() != 40 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) {
        bail!("Invalid torrent hash");
    }
    let data = s
        .rpc
        .call(
            "torrent_get",
            json!({"ids":[hash],"fields":["hash_string","download_dir","files"]}),
        )
        .await?;
    let torrent = data["torrents"]
        .as_array()
        .and_then(|a| a.first())
        .context("Torrent not found")?;
    let root = std::path::Path::new(text(torrent, "download_dir")?);
    if ![&s.config.source, &s.config.movies, &s.config.tv]
        .iter()
        .any(|p| root == std::path::Path::new(p))
    {
        bail!("Media is outside managed roots");
    }
    let file = torrent["files"]
        .as_array()
        .and_then(|a| a.get(index as usize))
        .context("Invalid file index")?;
    let length = file["length"].as_u64().context("Invalid file length")?;
    if length == 0 || file["bytes_completed"].as_u64() != Some(length) {
        bail!("Download is not complete");
    }
    let path = checked_path(root, text(file, "name")?)?;
    let extension = path
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ![
        "mp4", "m4v", "mkv", "mov", "avi", "webm", "ts", "m2ts", "mpg", "mpeg", "wmv",
    ]
    .contains(&extension.as_str())
    {
        bail!("Not a media file");
    }
    let metadata = std::fs::metadata(&path)?;
    if !metadata.is_file() || metadata.len() != length {
        bail!("Media is unavailable or changed");
    }
    Ok(path)
}

async fn playback_link(s: &Service, hash: &str, body: &Value) -> Result<Value> {
    let index = body["file_index"].as_u64().context("Invalid file index")?;
    let path = media_file(s, hash, index).await?;
    let size = path.metadata()?.len();
    // Opening now makes permission errors visible before presenting the player.
    std::fs::File::open(&path)
        .context("Torrbaby Server needs permission to read this media volume")?;
    let expires = now() as u64 + 21600;
    let mut result = json!({"path":format!("/v1/stream/{hash}/{index}?expires={expires}&sig={}",signature(&s.token,hash,index,expires)),"expires":expires,"name":path.file_name().unwrap_or_default().to_string_lossy()});
    if let Some(client) = body.get("client_id") {
        let progress =
            s.db.lock()
                .map_err(|_| anyhow::anyhow!("Database lock failed"))?
                .begin(
                    hash,
                    index,
                    &path.file_name().unwrap_or_default().to_string_lossy(),
                    size,
                    client.as_str().context("Invalid client ID")?,
                )?;
        result
            .as_object_mut()
            .unwrap()
            .extend(progress.as_object().unwrap().clone());
    }
    Ok(result)
}

async fn stream(
    s: &Service,
    uri: &axum::http::Uri,
    method: &Method,
    headers: &HeaderMap,
) -> Result<Response> {
    use hmac::{Hmac, Mac};
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let parts: Vec<_> = uri.path().trim_matches('/').split('/').collect();
    if parts.len() != 4 || (method != Method::GET && method != Method::HEAD) {
        bail!("Invalid streaming request");
    }
    let index: u64 = parts[3].parse()?;
    let parsed = reqwest::Url::parse(&format!("http://localhost{uri}"))?;
    let query: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
    let expires: u64 = query.get("expires").context("Missing expiry")?.parse()?;
    let sig = query.get("sig").context("Missing signature")?;
    if expires < now() as u64 || sig.len() != 64 {
        bail!("Expired or invalid playback URL");
    }
    let decoded: Result<Vec<u8>> = (0..64)
        .step_by(2)
        .map(|i| {
            Ok(u8::from_str_radix(
                sig.get(i..i + 2).context("Invalid signature")?,
                16,
            )?)
        })
        .collect();
    let mut mac = <Hmac<sha2::Sha256> as hmac::KeyInit>::new_from_slice(s.token.as_bytes())
        .expect("HMAC key");
    mac.update(format!("{}:{index}:{expires}", parts[2]).as_bytes());
    mac.verify_slice(&decoded?)
        .map_err(|_| anyhow::anyhow!("Invalid playback signature"))?;
    let path = media_file(s, parts[2], index).await?;
    let size = path.metadata()?.len();
    let range = headers.get("range").map(|h| h.to_str()).transpose()?;
    let selected = byte_range(range, size);
    if selected.is_err() {
        return Ok(Response::builder()
            .status(416)
            .header("Content-Range", format!("bytes */{size}"))
            .body(axum::body::Body::empty())?);
    }
    let (start, end) = selected?;
    let mut file = tokio::fs::File::open(&path)
        .await
        .context("Torrbaby Server needs permission to read this media volume")?;
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let mut response = Response::builder()
        .status(if range.is_some() { 206 } else { 200 })
        .header("Accept-Ranges", "bytes")
        .header("X-Torrbaby-Source", "filesystem")
        .header("Cache-Control", "private, no-store")
        .header("Content-Length", end - start + 1)
        .header(
            "Content-Type",
            match path.extension().and_then(|s| s.to_str()).unwrap_or("") {
                "mp4" | "m4v" => "video/mp4",
                "mov" => "video/quicktime",
                "mkv" => "video/x-matroska",
                "webm" => "video/webm",
                _ => "application/octet-stream",
            },
        );
    if range.is_some() {
        response = response.header("Content-Range", format!("bytes {start}-{end}/{size}"));
    }
    let body = if method == Method::HEAD {
        axum::body::Body::empty()
    } else {
        let chunks = futures_util::stream::try_unfold(
            (file, end - start + 1),
            |(mut file, remaining)| async move {
                if remaining == 0 {
                    return Ok::<_, std::io::Error>(None);
                }
                let mut buffer = vec![0; remaining.min(256 * 1024) as usize];
                let count = file.read(&mut buffer).await?;
                if count == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "Media ended early",
                    ));
                }
                buffer.truncate(count);
                Ok(Some((buffer, (file, remaining - count as u64))))
            },
        );
        axum::body::Body::from_stream(chunks)
    };
    Ok(response.body(body)?)
}

fn byte_range(header: Option<&str>, size: u64) -> Result<(u64, u64)> {
    if size == 0 {
        bail!("Empty media");
    }
    let Some(header) = header else {
        return Ok((0, size - 1));
    };
    let (first, last) = header
        .strip_prefix("bytes=")
        .and_then(|s| s.split_once('-'))
        .context("Invalid byte range")?;
    let (start, end) = if first.is_empty() {
        let count: u64 = last.parse()?;
        if count == 0 {
            bail!("Empty range");
        }
        (size.saturating_sub(count), size - 1)
    } else {
        (
            first.parse()?,
            if last.is_empty() {
                size - 1
            } else {
                last.parse::<u64>()?.min(size - 1)
            },
        )
    };
    if start > end || start >= size {
        bail!("Unsatisfiable range");
    }
    Ok((start, end))
}
