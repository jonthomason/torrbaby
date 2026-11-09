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
    pub(crate) config: Config,
    token: String,
    pub(crate) db: Mutex<Database>,
    pub(crate) rpc: Transmission,
    started: Instant,
    preview: bool,
    torrents: Mutex<Option<(Instant, Vec<Value>)>>,
    data_dir: PathBuf,
    http: reqwest::Client,
    scan_lock: tokio::sync::Mutex<()>,
    metadata: crate::metadata::Metadata,
    metadata_lock: tokio::sync::Mutex<()>,
    pub(crate) mutation_lock: tokio::sync::Mutex<()>,
}

// Keep unimplemented routes explicit during the port; never silently report success.
pub async fn serve(
    config_path: PathBuf,
    database: PathBuf,
    listen: String,
    preview: bool,
) -> Result<()> {
    let config = Config::load(&config_path)?;
    let live = expand(&config.state_file)?.with_extension("sqlite3");
    if preview
        && (database == live
            || (database.exists()
                && live.exists()
                && database.canonicalize()? == live.canonicalize()?))
    {
        bail!("The migration preview must use a copy of the live database");
    }
    // Bind before starting workers: a second server must never organize files.
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    if let Some(parent) = database.parent() {
        std::fs::create_dir_all(parent)?;
    }
    use std::os::unix::fs::OpenOptionsExt;
    let database = database
        .parent()
        .context("Database needs a parent directory")?
        .canonicalize()?
        .join(database.file_name().context("Invalid database path")?);
    let lease = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(database.with_extension("server.lock"))?;
    lease
        .try_lock()
        .context("Another Torrbaby server owns this database")?;
    let service = Arc::new(Service {
        token: config.token()?,
        rpc: Transmission::new(&config)?,
        db: Mutex::new(Database::open(&database, None)?),
        config,
        started: Instant::now(),
        preview,
        torrents: Mutex::new(None),
        data_dir: database
            .parent()
            .context("Database needs a parent directory")?
            .to_path_buf(),
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
        scan_lock: tokio::sync::Mutex::new(()),
        metadata: crate::metadata::Metadata::new(
            database
                .parent()
                .context("Database needs a parent directory")?,
        )?,
        metadata_lock: tokio::sync::Mutex::new(()),
        mutation_lock: tokio::sync::Mutex::new(()),
    });
    let sampling_service = service.clone();
    let cached_entities = service
        .db
        .lock()
        .map_err(|_| anyhow::anyhow!("Database lock failed"))?
        .documents("entities")?;
    service.metadata.seed_cache(cached_entities).await;
    let sampler = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let result = sampling_service
                .rpc
                .call("torrent_get", json!({"fields":crate::transmission::FIELDS}))
                .await;
            let policy = sampling_service
                .rpc
                .call(
                    "session_get",
                    json!({"fields":["seed_ratio_limit","seed_ratio_limited"]}),
                )
                .await
                .ok();
            if let Ok(mut result) = result {
                if let Some(torrents) = result["torrents"].as_array_mut() {
                    if let Ok(mut db) = sampling_service.db.lock() {
                        if let Err(error) = db.record_activity(torrents, policy.as_ref()) {
                            eprintln!("Activity tracking: {error}");
                        }
                        if let Err(error) = db.sample_torrents(torrents) {
                            eprintln!("Transfer sampling: {error}");
                        }
                    }
                    if let Ok(mut cache) = sampling_service.torrents.lock() {
                        *cache = Some((Instant::now(), torrents.clone()));
                    }
                }
            }
        }
    });
    let mut workers = Vec::new();
    if !preview {
        let organizer = service.clone();
        workers.push(tokio::spawn(async move {
            loop {
                if let Err(error) = crate::organizer::tick(&organizer).await {
                    let message = error.to_string();
                    let _ = organizer.change_state(|state| state["last_error"] = json!(message));
                    eprintln!("Organization: {error}");
                }
                let seconds = organizer
                    .effective_config()
                    .map(|c| c.poll_seconds)
                    .unwrap_or(30.0);
                tokio::time::sleep(std::time::Duration::from_secs_f64(seconds)).await;
            }
        }));
        let scanner = service.clone();
        workers.push(tokio::spawn(async move {
            loop {
                if let Err(error) = scan_library(&scanner).await {
                    let _ = scanner.change_state(|state| {
                        state["library_scan"]["scanning"] = json!(false);
                        state["library_scan"]["error"] = json!(error.to_string());
                    });
                    eprintln!("Library scan: {error}");
                }
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        }));
        let metadata = service.clone();
        workers.push(tokio::spawn(async move {
            loop {
                if let Err(error) = collect_metadata(&metadata).await {
                    eprintln!("Metadata collection: {error}");
                }
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        }));
    }
    let router = Router::new()
        .fallback(any(dispatch))
        .layer(DefaultBodyLimit::max(65536))
        .with_state(service);
    eprintln!(
        "Torrbaby Rust server listening on {}; automation {}",
        listener.local_addr()?,
        if preview {
            "disabled (preview)"
        } else {
            "enabled"
        }
    );
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("Install termination handler");
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
        })
        .await?;
    sampler.abort();
    for worker in workers {
        worker.abort();
        let _ = worker.await;
    }
    drop(lease);
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
    let redeem = method == Method::POST && uri.path() == "/v1/pairings/redeem";
    let device_authorized =
        s.db.lock()
            .map(|db| crate::pairing::authorized(&db, supplied))
            .unwrap_or(false);
    if !redeem && actual.verify_slice(&tag).is_err() && !device_authorized {
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
    if let Some(id) = uri
        .path()
        .strip_prefix("/v1/library/assets/")
        .and_then(|p| p.strip_suffix("/download"))
    {
        let result = async {
            if !id.starts_with("asset-")
                || id.contains('/')
                || (method != Method::GET && method != Method::HEAD)
            {
                bail!("Invalid download request");
            }
            let path = media_file(&s, id, 0).await?;
            file_response(&path, &method, &headers).await
        }
        .await;
        return match result {
            Ok(response) => response,
            Err(error) => (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":error.to_string()})),
            )
                .into_response(),
        };
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

async fn handle(s: &Arc<Service>, method: &Method, path: &str, body: Value) -> Result<Value> {
    if method == Method::GET {
        if let Some(id) = path
            .strip_prefix("/v1/library/assets/")
            .and_then(|p| p.strip_suffix("/download-info"))
        {
            if !id.starts_with("asset-") || id.contains('/') {
                bail!("Invalid asset ID")
            }
            let path = media_file(s, id, 0).await?;
            let file = tokio::fs::File::open(&path)
                .await
                .context("Torrbaby Server needs permission to read this media volume")?;
            let metadata = file.metadata().await?;
            return Ok(
                json!({"asset_id":id,"path":format!("/v1/library/assets/{id}/download"),
                "name":path.file_name().unwrap_or_default().to_string_lossy(),"size":metadata.len(),
                "etag":file_revision(&metadata),"content_type":media_type(&path),"representation":"original"}),
            );
        }
    }
    if path == "/v1/pairings"
        || path.starts_with("/v1/pairings/")
        || path.starts_with("/v1/device-credentials/")
    {
        let mut db =
            s.db.lock()
                .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
        return crate::pairing::handle(&mut db, method.as_str(), path, &body);
    }
    if method == Method::POST && path == "/v1/reconcile" {
        if s.preview {
            bail!("Automatic organization is disabled in preview mode");
        }
        crate::organizer::tick(s).await?;
        scan_library(s).await?;
        return Ok(json!({"accepted":true}));
    }
    if let Some((hash, action)) = path
        .strip_prefix("/v1/torrents/")
        .and_then(|p| p.split_once('/'))
    {
        if method == Method::POST
            && ["metadata-refresh", "metadata-select", "lookup"].contains(&action)
        {
            let _guard = s.metadata_lock.lock().await;
            let torrent = crate::organizer::torrent(s, hash).await?;
            let hash = text(&torrent, "hash_string")?;
            let mut record = if action == "metadata-select" {
                let mut record = torrent_metadata(s, hash)?.context("Look up metadata first")?;
                let index = body["candidate"].as_u64().context("Invalid candidate")?;
                let candidate = record["candidates"]
                    .as_array()
                    .and_then(|a| a.get(index as usize))
                    .context("Invalid candidate")?;
                record["selected"] = s.metadata.enrich(candidate).await?;
                record["status"] = json!("matched");
                record["confirmed"] = json!(true);
                record
            } else {
                s.metadata.search(text(&torrent, "name")?).await?
            };
            if action == "lookup" {
                let candidates: Vec<Value> = record["candidates"].as_array().into_iter().flatten().map(|c| json!({"title":c["title"],"year":c["year"],"source_url":c["source_url"],"path":null,"proposed_root_name":null})).collect();
                return Ok(
                    json!({"query":record["query"],"provider":"TMDB","candidates":candidates,"note":"Review the title and year before entering a new file or folder name."}),
                );
            }
            record["hash"] = json!(hash);
            record["updated_at"] = json!(now());
            save_torrent_metadata(s, &record)?;
            return Ok(record);
        }
        if method == Method::GET && action == "artwork" {
            return artwork(s, &format!("torrent:{hash}"), "poster").await;
        }
    }
    if path == "/v1/settings" && (method == Method::GET || method == Method::PATCH) {
        let _guard = s.mutation_lock.lock().await;
        let mut config = serde_json::to_value(s.effective_config()?)?;
        if method == Method::PATCH {
            let patch = body.as_object().context("Expected settings object")?;
            if patch.is_empty()
                || patch.keys().any(|k| {
                    !["paused", "poll_seconds", "source", "movies", "tv"].contains(&k.as_str())
                })
            {
                bail!("Allowed settings: paused, poll_seconds, source, movies, tv");
            }
            for (key, value) in patch {
                config[key] = value.clone();
            }
            let updated: Config = serde_json::from_value(config.clone())?;
            updated.validate()?;
            for key in ["source", "movies", "tv"] {
                if let Some(value) = patch.get(key) {
                    if !std::path::Path::new(value.as_str().context("Folder must be text")?)
                        .is_dir()
                    {
                        bail!("Media directory does not exist or is inaccessible");
                    }
                }
            }
            s.change_state(|state| {
                if !state["settings"].is_object() {
                    state["settings"] = json!({});
                }
                for (key, value) in patch {
                    state["settings"][key] = value.clone();
                }
            })?;
            s.event("settings_changed", json!({"settings":body}))?;
        }
        return Ok(
            json!({"paused":config["paused"],"poll_seconds":config["poll_seconds"],"source":config["source"],"movies":config["movies"],"tv":config["tv"]}),
        );
    }
    if method == Method::POST && path == "/v1/torrents" {
        return crate::organizer::add(s, &body).await;
    }
    if method == Method::POST {
        if let Some(id) = path
            .strip_prefix("/v1/library/assets/")
            .and_then(|p| p.strip_suffix("/watched"))
        {
            let watched = body["watched"]
                .as_bool()
                .context("watched must be boolean")?;
            let mut db =
                s.db.lock()
                    .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
            let asset = db.asset(id)?;
            let hash = asset["torrent_hash"].as_str().unwrap_or(id);
            return db.mark(
                hash,
                asset["file_index"].as_u64().unwrap_or(0),
                std::path::Path::new(text(&asset, "path")?)
                    .file_name()
                    .unwrap_or_default()
                    .to_str()
                    .context("Invalid filename")?,
                asset["size"].as_u64().context("Invalid file size")?,
                watched,
            );
        }
        if let Some((hash, action)) = path
            .strip_prefix("/v1/torrents/")
            .and_then(|p| p.split_once('/'))
        {
            if action == "watched" {
                let index = body["file_index"].as_u64().context("Invalid file index")?;
                let watched = body["watched"]
                    .as_bool()
                    .context("watched must be boolean")?;
                let path = torrent_media_file(s, hash, index).await?;
                return s
                    .db
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Database lock failed"))?
                    .mark(
                        &hash.to_ascii_lowercase(),
                        index,
                        &path.file_name().unwrap_or_default().to_string_lossy(),
                        path.metadata()?.len(),
                        watched,
                    );
            }
            if [
                "start",
                "stop",
                "verify",
                "reannounce",
                "rename",
                "remove",
                "retry",
                "ignore",
            ]
            .contains(&action)
            {
                return crate::organizer::action(s, hash, action, &body).await;
            }
        }
    }
    if method == Method::GET && path == "/v1/providers" {
        return Ok(json!({"tmdb_configured":s.metadata.configured()}));
    }
    if method == Method::POST && path == "/v1/library/search" {
        return s.metadata.search(text(&body, "query")?).await;
    }
    if method == Method::POST && path == "/v1/providers/tmdb" {
        s.metadata.configure(text(&body, "token")?).await?;
        return Ok(json!({"configured":true}));
    }
    if method == Method::POST && path == "/v1/library/metadata-refresh" {
        let service = s.clone();
        tokio::spawn(async move {
            if let Err(error) = collect_metadata(&service).await {
                eprintln!("Metadata collection: {error}");
            }
        });
        return Ok(json!({"accepted":true}));
    }
    if method == Method::POST {
        if let Some(id) = path
            .strip_prefix("/v1/library/assets/")
            .and_then(|p| p.strip_suffix("/match"))
        {
            let asset =
                s.db.lock()
                    .map_err(|_| anyhow::anyhow!("Database lock failed"))?
                    .asset(id)?;
            let index = body["candidate"].as_u64().context("Invalid candidate")?;
            let candidate = asset["candidates"]
                .as_array()
                .and_then(|a| a.get(index as usize))
                .context("Invalid candidate")?;
            let entity = s.metadata.enrich(candidate).await?;
            save_match(s, id, entity, true)?;
            return Ok(json!({"accepted":true}));
        }
    }
    if method == Method::POST && path == "/v1/library/scan" {
        let service = s.clone();
        tokio::spawn(async move {
            if let Err(error) = scan_library(&service).await {
                eprintln!("Library scan: {error}");
            }
        });
        return Ok(json!({"accepted":true}));
    }
    if method == Method::GET {
        if let Some(route) = path.strip_prefix("/v1/library/artwork/") {
            let (id, variant) = route.rsplit_once('/').context("Missing artwork variant")?;
            return artwork(s, id, variant).await;
        }
    }
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
        if let Some(id) = path
            .strip_prefix("/v1/library/assets/")
            .and_then(|s| s.strip_suffix("/play"))
        {
            return playback_link(s, id, &body).await;
        }
        if let Some(hash) = path
            .strip_prefix("/v1/torrents/")
            .and_then(|s| s.strip_suffix("/play"))
        {
            return playback_link(s, hash, &body).await;
        }
    }
    if method == Method::GET && path == "/v1/torrents" {
        if let Some((updated, torrents)) = s
            .torrents
            .lock()
            .map_err(|_| anyhow::anyhow!("Torrent cache lock failed"))?
            .as_ref()
        {
            if updated.elapsed().as_secs() < 15 {
                return Ok(json!(torrents));
            }
        }
        return Ok(s
            .rpc
            .call("torrent_get", json!({"fields":crate::transmission::FIELDS}))
            .await?["torrents"]
            .take());
    }
    let effective = if path == "/v1/status" {
        Some(s.effective_config()?)
    } else {
        None
    };
    // No lock is held across an await. SQLite operations remain short and transactional.
    let mut db =
        s.db.lock()
            .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
    if method == Method::GET {
        if let Some(hash) = path
            .strip_prefix("/v1/torrents/")
            .and_then(|s| s.strip_suffix("/rates"))
        {
            return Ok(json!(db.torrent_rates(hash)?));
        }
    }
    match (method.as_str(),path) {
        ("GET","/v1/library") => db.catalog(),
        ("GET","/v1/library/matches") => Ok(json!(db.documents("assets")?.into_iter().filter(|a|a["available"]==true && a["status"]!="matched" && a["role"]!="extra").collect::<Vec<_>>())),
        ("GET","/v1/status") => {
            let state = db.state()?;
            let config = effective.as_ref().context("Missing settings")?;
            Ok(json!({"version":env!("CARGO_PKG_VERSION"),"uptime_seconds":s.started.elapsed().as_secs(),
                "settings":{"source":config.source,"movies":config.movies,"tv":config.tv,"poll_seconds":config.poll_seconds,"paused":s.preview || config.paused},
                "last_poll":state["last_poll"],"last_error":if s.preview { json!("Migration preview: organizer not enabled") } else {state["last_error"].clone()},"pending_scans":if state["library_scan"]["scanning"] == true {1} else {0},"scan_error":state["library_scan"]["error"],"jobs":state["jobs"]}))
        }
        ("GET","/v1/events") => Ok(db.state()?["events"].take()),
        ("GET","/v1/clients") => Ok(json!(db.rows("SELECT clients.*, EXISTS(SELECT 1 FROM device_credentials WHERE client_id=clients.id) AS paired_credential FROM clients ORDER BY last_seen DESC",[])?)),
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

pub(crate) fn checked_path(root: &std::path::Path, relative: &str) -> Result<PathBuf> {
    use std::path::Component;
    let relative = std::path::Path::new(relative);
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
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
    if hash.starts_with("asset-") {
        if index != 0 {
            bail!("Invalid asset file index");
        }
        let asset =
            s.db.lock()
                .map_err(|_| anyhow::anyhow!("Database lock failed"))?
                .asset(hash)?;
        if asset["complete"] != true {
            bail!("Download is not complete");
        }
        let path = PathBuf::from(text(&asset, "path")?);
        let mut allowed = None;
        let config = s.effective_config()?;
        for root in [&config.source, &config.movies, &config.tv] {
            if let Ok(relative) = path.strip_prefix(root) {
                allowed = Some(checked_path(
                    std::path::Path::new(root),
                    relative.to_str().context("Invalid media path")?,
                )?);
                break;
            }
        }
        let path = allowed.context("Media is outside managed roots")?;
        if let Ok(metadata) = path.metadata() {
            if metadata.is_file() && Some(metadata.len()) == asset["size"].as_u64() {
                return Ok(path);
            }
        }
        if let Some(torrent) = asset["torrent_hash"].as_str() {
            return torrent_media_file(
                s,
                torrent,
                asset["file_index"].as_u64().context("Invalid file index")?,
            )
            .await;
        }
        bail!("Media is unavailable or changed");
    }
    torrent_media_file(s, hash, index).await
}

async fn torrent_media_file(s: &Service, hash: &str, index: u64) -> Result<PathBuf> {
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
    let config = s.effective_config()?;
    if ![&config.source, &config.movies, &config.tv]
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
    let index = if hash.starts_with("asset-") {
        0
    } else {
        body["file_index"].as_u64().context("Invalid file index")?
    };
    let path = media_file(s, hash, index).await?;
    let size = path.metadata()?.len();
    // Opening now makes permission errors visible before presenting the player.
    std::fs::File::open(&path)
        .context("Torrbaby Server needs permission to read this media volume")?;
    let expires = now() as u64 + 21600;
    let mut result = json!({"path":format!("/v1/stream/{hash}/{index}?expires={expires}&sig={}",signature(&s.token,hash,index,expires)),"expires":expires,"name":path.file_name().unwrap_or_default().to_string_lossy()});
    if let Some(client) = body.get("client_id") {
        let mut db =
            s.db.lock()
                .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
        let (media_hash, media_index) = if hash.starts_with("asset-") {
            let asset = db.asset(hash)?;
            (
                asset["torrent_hash"].as_str().unwrap_or(hash).to_owned(),
                asset["file_index"].as_u64().unwrap_or(0),
            )
        } else {
            (hash.to_owned(), index)
        };
        let progress = db.begin(
            &media_hash,
            media_index,
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
    file_response(&path, method, headers).await
}

// File-system revision validator, not a content checksum. Includes inode and nanosecond
// change times so replacement and in-place modification invalidate a resumed download.
fn file_revision(metadata: &std::fs::Metadata) -> String {
    use sha2::Digest;
    use std::os::unix::fs::MetadataExt;
    let revision = format!(
        "{}:{}:{}:{}:{}:{}:{}",
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec()
    );
    let hash: String = sha2::Sha256::digest(revision.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("\"{hash}\"")
}

fn media_type(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

async fn file_response(
    path: &std::path::Path,
    method: &Method,
    headers: &HeaderMap,
) -> Result<Response> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut file = tokio::fs::File::open(path)
        .await
        .context("Torrbaby Server needs permission to read this media volume")?;
    let metadata = file.metadata().await?;
    let size = metadata.len();
    let revision = file_revision(&metadata);
    if let Some(condition) = headers.get("if-match") {
        let condition = condition.to_str()?;
        if condition != "*" && !condition.split(',').any(|value| value.trim() == revision) {
            return Ok(Response::builder()
                .status(412)
                .header("ETag", &revision)
                .header("Cache-Control", "private, no-store")
                .body(axum::body::Body::empty())?);
        }
    }
    let mut range = headers.get("range").map(|h| h.to_str()).transpose()?;
    if let Some(condition) = headers.get("if-range") {
        if condition.to_str()? != revision {
            range = None;
        }
    }
    let selected = byte_range(range, size);
    if selected.is_err() {
        return Ok(Response::builder()
            .status(416)
            .header("ETag", &revision)
            .header("Content-Range", format!("bytes */{size}"))
            .body(axum::body::Body::empty())?);
    }
    let (start, end) = selected?;
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let mut response = Response::builder()
        .status(if range.is_some() { 206 } else { 200 })
        .header("Accept-Ranges", "bytes")
        .header("X-Torrbaby-Source", "filesystem")
        .header("Cache-Control", "private, no-store")
        .header("Content-Length", end - start + 1)
        .header("ETag", &revision)
        .header("Content-Type", media_type(path));
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

async fn artwork(s: &Service, id: &str, variant: &str) -> Result<Value> {
    use base64::Engine;
    use sha2::Digest;
    if !["poster", "backdrop", "logo", "episode"].contains(&variant) {
        bail!("Unknown artwork variant");
    }
    let url = {
        let db =
            s.db.lock()
                .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
        let entities = db.documents("entities")?;
        if let Some(hash) = id.strip_prefix("torrent:") {
            db.documents("metadata")?
                .iter()
                .find(|r| r["hash"] == hash)
                .and_then(|r| r["selected"]["thumbnail"].as_str())
                .map(str::to_owned)
        } else if let Some(entity) = entities.iter().find(|e| e["id"] == id) {
            entity[match variant {
                "backdrop" => "backdrop",
                "logo" => "logo",
                _ => "thumbnail",
            }]
            .as_str()
            .map(str::to_owned)
        } else {
            let asset = db.asset(id)?;
            entities
                .iter()
                .find(|e| e["id"] == asset["entity_id"])
                .and_then(|entity| {
                    entity["episodes"]
                        .as_array()
                        .and_then(|episodes| {
                            episodes.iter().find(|e| {
                                e["season"] == asset["season"] && e["episode"] == asset["episode"]
                            })
                        })
                        .and_then(|episode| episode["thumbnail"].as_str())
                        .or(entity["thumbnail"].as_str())
                        .map(str::to_owned)
                })
        }
    };
    let Some(url) = url else {
        return Ok(json!({"data":null}));
    };
    let key = sha2::Sha256::digest(url.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let cache = s.data_dir.join("artwork");
    tokio::fs::create_dir_all(&cache).await?;
    let path = cache.join(format!("{key}.image"));
    let bytes = if path.exists() {
        tokio::fs::read(&path).await?
    } else {
        // Legacy Plex references are deliberately not fetched by the standalone server.
        if url.starts_with('/') {
            return Ok(json!({"data":null}));
        }
        let parsed = reqwest::Url::parse(&url)?;
        if parsed.scheme() != "https"
            || ![Some("image.tmdb.org"), Some("static.tvmaze.com")].contains(&parsed.host_str())
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            bail!("Unsupported artwork source");
        }
        let mut response = s
            .http
            .get(parsed)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Artwork service unavailable"))?;
        if !response.status().is_success() {
            bail!("Artwork service HTTP {}", response.status().as_u16());
        }
        let mut data = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if data.len() + chunk.len() > 8 * 1024 * 1024 {
                bail!("Artwork too large");
            }
            data.extend_from_slice(&chunk);
        }
        let temporary = cache.join(format!("{key}.{}.tmp", uuid::Uuid::new_v4()));
        tokio::fs::write(&temporary, &data).await?;
        tokio::fs::rename(&temporary, &path).await?;
        data
    };
    if bytes.len() > 8 * 1024 * 1024 {
        bail!("Artwork too large");
    }
    Ok(json!({"data":base64::engine::general_purpose::STANDARD.encode(bytes)}))
}

async fn scan_library(s: &Service) -> Result<()> {
    let Ok(_guard) = s.scan_lock.try_lock() else {
        return Ok(());
    };
    let _mutation = s.mutation_lock.lock().await;
    let old = {
        let mut db =
            s.db.lock()
                .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
        let mut state = db.state()?;
        state["library_scan"] =
            json!({"scanning":true,"last_scan":state["library_scan"]["last_scan"],"error":null});
        db.save_state(&state)?;
        db.documents("assets")?
    };
    let torrents = match s
        .rpc
        .call("torrent_get", json!({"fields":crate::transmission::FIELDS}))
        .await
    {
        Ok(value) => value["torrents"].as_array().cloned().unwrap_or_default(),
        Err(_) => Vec::new(), // Filesystem discovery still works when Transmission is unavailable.
    };
    let config = s.effective_config()?;
    let result =
        tokio::task::spawn_blocking(move || crate::scanner::inventory(&config, old, torrents))
            .await;
    let mut db =
        s.db.lock()
            .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
    let mut state = db.state()?;
    match result {
        Ok(inventory) => {
            let tx = db.connection.transaction()?;
            for mut asset in inventory.assets {
                use rusqlite::OptionalExtension;
                let id = asset["id"].as_str().context("Invalid asset ID")?.to_owned();
                let current: Option<String> = tx
                    .query_row(
                        "SELECT document FROM library_assets WHERE id=?",
                        [&id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(current) = current {
                    let current: Value = serde_json::from_str(&current)?;
                    for key in [
                        "entity_id",
                        "status",
                        "candidates",
                        "confirmed",
                        "matched_at",
                        "match_confidence",
                        "error",
                    ] {
                        if let Some(value) = current.get(key) {
                            asset[key] = value.clone();
                        }
                    }
                }
                tx.execute(
                    "INSERT OR REPLACE INTO library_assets VALUES(?,?)",
                    rusqlite::params![id, asset.to_string()],
                )?;
            }
            tx.commit()?;
            db.link_extras()?;
            state["library_scan"] = json!({"scanning":false,"last_scan":now(),"error":if inventory.errors.is_empty(){None}else{Some(inventory.errors.join("\n"))}});
        }
        Err(error) => {
            state["library_scan"] = json!({"scanning":false,"last_scan":state["library_scan"]["last_scan"],"error":error.to_string()})
        }
    }
    db.save_state(&state)?;
    Ok(())
}

fn save_match(s: &Service, id: &str, entity: Value, confirmed: bool) -> Result<()> {
    let mut db =
        s.db.lock()
            .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
    let mut asset = db.asset(id)?;
    if asset["confirmed"] == true && !confirmed {
        return Ok(());
    }
    let entity_id = entity["id"].as_str().context("Invalid metadata identity")?;
    asset["entity_id"] = json!(entity_id);
    asset["matched_at"] = json!(now());
    asset["status"] = json!("matched");
    asset["match_confidence"] = entity["match_confidence"].clone();
    asset["error"] = Value::Null;
    if confirmed {
        asset["confirmed"] = json!(true);
    }
    let tx = db.connection.transaction()?;
    tx.execute(
        "INSERT OR REPLACE INTO library_entities VALUES(?,?)",
        rusqlite::params![entity_id, entity.to_string()],
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO library_assets VALUES(?,?)",
        rusqlite::params![id, asset.to_string()],
    )?;
    tx.commit()?;
    db.link_extras()?;
    Ok(())
}

async fn collect_metadata(s: &Service) -> Result<()> {
    if !s.metadata.configured() {
        return Ok(());
    }
    let Ok(_guard) = s.metadata_lock.try_lock() else {
        return Ok(());
    };
    if let Ok(torrents) = crate::organizer::torrents(s).await {
        for torrent in torrents {
            let hash = text(&torrent, "hash_string")?;
            let old = torrent_metadata(s, hash)?;
            if old.as_ref().is_some_and(|r| {
                r["confirmed"] == true
                    || now() - r["updated_at"].as_f64().unwrap_or(0.0)
                        < if r["status"] == "matched" {
                            86400.0
                        } else {
                            900.0
                        }
            }) {
                // Warm the disk cache even for metadata collected before artwork prefetch existed.
                if let Err(error) = artwork(s, &format!("torrent:{hash}"), "poster").await {
                    tracing::debug!(%hash, %error, "Torrent artwork is not available yet");
                }
                continue;
            }
            let mut record = match s.metadata.search(text(&torrent, "name")?).await {
                Ok(record) => record,
                Err(error) => {
                    json!({"status":"error","candidates":[],"selected":null,"error":error.to_string()})
                }
            };
            record["hash"] = json!(hash);
            record["updated_at"] = json!(now());
            save_torrent_metadata(s, &record)?;
            if let Err(error) = artwork(s, &format!("torrent:{hash}"), "poster").await {
                tracing::debug!(%hash, %error, "Torrent artwork is not available yet");
            }
        }
    }
    let assets =
        s.db.lock()
            .map_err(|_| anyhow::anyhow!("Database lock failed"))?
            .documents("assets")?;
    for asset in &assets {
        if asset["available"] != true || asset["role"] == "extra" || asset["confirmed"] == true {
            continue;
        }
        if let Some(hash) = asset["torrent_hash"].as_str() {
            if let Some(record) = torrent_metadata(s, hash)?
                .filter(|r| r["confirmed"] == true && r["selected"].is_object())
            {
                let path = std::path::Path::new(text(&asset, "path")?);
                let query = crate::metadata::parse_name(
                    &path.file_stem().unwrap_or_default().to_string_lossy(),
                )
                .0;
                let normalize = |s: &str| {
                    s.chars()
                        .filter(|c| c.is_alphanumeric())
                        .flat_map(char::to_lowercase)
                        .collect::<String>()
                };
                let same_title =
                    normalize(&query) == normalize(record["query"].as_str().unwrap_or(""));
                // A collection-level choice must not overwrite every film in a collection.
                let single_video = assets
                    .iter()
                    .filter(|a| a["torrent_hash"] == hash && a["role"] != "extra")
                    .count()
                    == 1;
                if same_title || single_video {
                    save_match(s, text(&asset, "id")?, record["selected"].clone(), true)?;
                    continue;
                }
            }
        }
        let age = now() - asset["matched_at"].as_f64().unwrap_or(0.0);
        if age
            < if asset["entity_id"].is_string() {
                86400.0
            } else {
                900.0
            }
        {
            continue;
        }
        let id = text(&asset, "id")?;
        let path = std::path::Path::new(text(&asset, "path")?);
        let mut name = asset["series_name"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
        if let Some(season) = asset["season"].as_u64() {
            if !crate::scanner::EPISODE.is_match(&name) {
                name += &format!(
                    " S{season:02}E{:02}",
                    asset["episode"].as_u64().unwrap_or(0)
                );
            }
        }
        let result = s.metadata.search(&name).await;
        match result {
            Ok(result) => {
                if result["selected"].is_object() {
                    save_match(s, id, result["selected"].clone(), false)?;
                } else {
                    let db =
                        s.db.lock()
                            .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
                    let mut current = db.asset(id)?;
                    if current["confirmed"] != true {
                        current["status"] = result["status"].clone();
                        current["candidates"] = result["candidates"].clone();
                        current["matched_at"] = json!(now());
                        db.connection.execute(
                            "UPDATE library_assets SET document=? WHERE id=?",
                            rusqlite::params![current.to_string(), id],
                        )?;
                    }
                }
            }
            Err(error) => {
                let db =
                    s.db.lock()
                        .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
                let mut current = db.asset(id)?;
                if current["confirmed"] != true {
                    current["error"] = json!(error.to_string());
                    current["matched_at"] = json!(now());
                    db.connection.execute(
                        "UPDATE library_assets SET document=? WHERE id=?",
                        rusqlite::params![current.to_string(), id],
                    )?;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    Ok(())
}

impl Service {
    pub(crate) fn effective_config(&self) -> Result<Config> {
        let state = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Database lock failed"))?
            .state()?;
        let mut config = serde_json::to_value(&self.config)?;
        for key in ["paused", "poll_seconds", "source", "movies", "tv"] {
            if let Some(value) = state["settings"].get(key) {
                config[key] = value.clone();
            }
        }
        let config: Config = serde_json::from_value(config)?;
        config.validate()?;
        Ok(config)
    }
    pub(crate) fn change_state(&self, change: impl FnOnce(&mut Value)) -> Result<()> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
        let mut state = db.state()?;
        change(&mut state);
        db.save_state(&state)
    }
    pub(crate) fn event(&self, kind: &str, mut data: Value) -> Result<()> {
        data["kind"] = json!(kind);
        data["time"] = json!(now());
        self.change_state(|state| {
            if !state["events"].is_array() {
                state["events"] = json!([]);
            }
            let events = state["events"].as_array_mut().unwrap();
            events.push(data);
            if events.len() > 1000 {
                events.drain(..events.len() - 1000);
            }
        })
    }
    pub(crate) fn invalidate_torrents(&self) {
        if let Ok(mut cache) = self.torrents.lock() {
            *cache = None;
        }
    }
}

fn torrent_metadata(s: &Service, hash: &str) -> Result<Option<Value>> {
    use rusqlite::OptionalExtension;
    let db =
        s.db.lock()
            .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
    let document: Option<String> = db
        .connection
        .query_row("SELECT document FROM metadata WHERE hash=?", [hash], |r| {
            r.get(0)
        })
        .optional()?;
    document
        .map(|d| serde_json::from_str(&d).map_err(Into::into))
        .transpose()
}
fn save_torrent_metadata(s: &Service, record: &Value) -> Result<()> {
    let mut db =
        s.db.lock()
            .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
    let tx = db.connection.transaction()?;
    tx.execute(
        "INSERT OR REPLACE INTO metadata VALUES(?,?)",
        rusqlite::params![text(record, "hash")?, record.to_string()],
    )?;
    if let Some(id) = record["selected"]["id"].as_str() {
        tx.execute(
            "INSERT OR REPLACE INTO library_entities VALUES(?,?)",
            rusqlite::params![id, record["selected"].to_string()],
        )?;
    }
    tx.commit()?;
    Ok(())
}
