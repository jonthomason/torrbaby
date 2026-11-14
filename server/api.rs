use crate::{
    config::{Config, expand},
    storage::{Database, now},
    transmission::Transmission,
};
use anyhow::{Context, Result, bail};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{ConnectInfo, DefaultBodyLimit, OriginalUri, State},
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
    pub(crate) data_dir: PathBuf,
    http: reqwest::Client,
    scan_lock: tokio::sync::Mutex<()>,
    pub(crate) metadata: crate::metadata::Metadata,
    metadata_lock: tokio::sync::Mutex<()>,
    pub(crate) mutation_lock: tokio::sync::Mutex<()>,
    pub(crate) subtitle_jobs: Arc<crate::subtitle_jobs::Manager>,
    pub(crate) hls: Arc<crate::hls::Manager>,
    pub(crate) virtual_media: Arc<crate::virtual_media::Manager>,
    lan_limiter: crate::lan::Limiter,
    pub(crate) torrent_search: crate::torrent_search::Search,
    pub(crate) cloud: crate::cloud::Cloud,
    pub(crate) watcher: crate::watcher::Watcher,
}

// Keep unimplemented routes explicit during the port; never silently report success.
/// The library's own name and id, set once when the server first starts and kept from
/// then on. Clients remember it at pairing and key every cache, setting and download to
/// it; an address is only ever a route to the server, never its identity.
fn establish_identity(s: &Service) -> Result<Value> {
    let mut identity = Value::Null;
    s.change_state(|state| {
        let named = state["server"]["name"].as_str().is_some_and(|name| !name.trim().is_empty());
        if !named {
            state["server"] = json!({"name":computer_name(),"id":uuid::Uuid::new_v4().to_string(),"created":now()});
        }
        identity = state["server"].clone();
    })?;
    Ok(identity)
}

/// The names clients reach the server by: its Bonjour host name at home and its Tailscale
/// (MagicDNS) name away. Both are looked up from the Mac, so a new address never matters.
static ROUTES: std::sync::RwLock<Value> = std::sync::RwLock::new(Value::Null);

async fn refresh_routes(home_host: Option<String>, away_host: Option<String>) {
    let configured = |value: Option<String>| value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    if let (Some(home), Some(away)) = (configured(home_host.clone()), configured(away_host.clone())) {
        if let Ok(mut current) = ROUTES.write() { *current = json!({"home":home,"away":away}); }
        return;
    }
    async fn output(program: &str, args: &[&str]) -> Option<String> {
        let output = tokio::process::Command::new(program).args(args).output().await.ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }
    let mut home = configured(home_host);
    if home.is_none() { home = output("/usr/sbin/scutil", &["--get", "LocalHostName"]).await.filter(|n| !n.is_empty()).map(|n| format!("{n}.local")); }
    let mut away = configured(away_host);
    for program in ["/opt/homebrew/bin/tailscale", "/usr/local/bin/tailscale", "/Applications/Tailscale.app/Contents/MacOS/Tailscale"] {
        if away.is_some() { break; }
        if let Some(text) = output(program, &["status", "--json"]).await {
            away = serde_json::from_str::<Value>(&text).ok()
                .and_then(|status| status["Self"]["DNSName"].as_str().map(|name| name.trim_end_matches('.').to_owned()))
                .filter(|name| !name.is_empty());
            if away.is_some() { break; }
        }
    }
    let routes = json!({"home":home,"away":away});
    if let Ok(mut current) = ROUTES.write() {
        // Keep the last names known if a lookup fails for a moment.
        if current.is_null() || (routes["home"].is_string() && routes["away"].is_string()) { *current = routes; }
    }
}

fn computer_name() -> String {
    std::process::Command::new("/usr/sbin/scutil").args(["--get", "ComputerName"]).output().ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Torrbaby".to_owned())
}

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
    // The server listens on every network the Mac is on (`[::]` also takes IPv4), never on
    // a particular address: its addresses come from DHCP and Tailscale and may change.
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    // Where `[::]` is IPv6-only, IPv4 needs its own socket; where it is dual-stack this
    // bind fails because the port is taken, which is expected.
    let lan_listener = if listen.starts_with("[::]:") {
        tokio::net::TcpListener::bind(format!("0.0.0.0:{}", config.listen_port)).await.ok()
    } else {
        None
    };
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
        cloud: crate::cloud::Cloud::new(database.parent().context("Database needs a parent directory")?)?,
        watcher: crate::watcher::Watcher::new(database.parent().context("Database needs a parent directory")?),
        lan_limiter: crate::lan::Limiter::default(),
        torrent_search: crate::torrent_search::Search::new()?.with_cache_dir(database.parent().context("Database needs a parent directory")?.join("search-cache"))?
            .with_private_site(database.parent().context("Database needs a parent directory")?.join("private-site.json")),
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
        virtual_media: crate::virtual_media::Manager::new(database.parent().context("Missing data directory")?)?,
        hls: crate::hls::Manager::new(database.parent().context("Missing data directory")?)?,
        subtitle_jobs: crate::subtitle_jobs::Manager::new(
            database.parent().context("Missing data directory")?,
        )?,
    });
    let identity = establish_identity(&service)?;
    let name = identity["name"].as_str().unwrap_or("Torrbaby").to_owned();
    eprintln!("Torrbaby server {name}");
    // Clients at home find the server by this Bonjour service and its name, never an address.
    let mut advertisement = if !preview && service.config.advertise_lan {
        Some(
            tokio::process::Command::new("/usr/bin/dns-sd")
                .args([
                    "-R",
                    &name,
                    "_torrbaby._tcp",
                    "local.",
                    &service.config.listen_port.to_string(),
                    "api=1",
                    &format!("name={name}"),
                    &format!("id={}", identity["id"].as_str().unwrap_or_default()),
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .context("Advertise Torrbaby on LAN")?,
        )
    } else {
        None
    };
    let configured = (service.config.home_host.clone(), service.config.away_host.clone());
    let routes_task = tokio::spawn(async move {
        loop {
            refresh_routes(configured.0.clone(), configured.1.clone()).await;
            tokio::time::sleep(std::time::Duration::from_secs(600)).await;
        }
    });
    // Off in previews and tests: only the live server holds the connection to bronze.
    let rendezvous_task = (!preview).then(|| service.config.rendezvous.clone()).flatten()
        .map(|config| crate::rendezvous::spawn(config, service.config.listen_port));
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
        let cloud = service.clone();
        workers.push(tokio::spawn(async move { cloud.cloud.work(&cloud).await; }));
        let watching = service.clone();
        workers.push(tokio::spawn(async move { watching.watcher.work(&watching).await; }));
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
        let moves = service.clone();
        workers.push(tokio::spawn(async move {
            loop {
                if let Err(error) = crate::drive_moves::tick(&moves).await { tracing::warn!(%error, "Moving files between drives"); }
                // Files left open for playback would stop a drive from being ejected.
                moves.virtual_media.release_idle(std::time::Duration::from_secs(120));
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }));
        let removals = service.clone();
        workers.push(tokio::spawn(async move {
            loop {
                if let Err(error) = crate::library_files::tick(&removals).await { eprintln!("File management: {error}"); }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
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
    if !preview {
        let artwork_service = service.clone();
        workers.push(tokio::spawn(async move {
            loop {
                if let Err(error) = prefetch_library_artwork(&artwork_service).await {
                    tracing::warn!(%error, "Library artwork preparation failed");
                }
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        }));
        let capacity_service=service.clone();
        workers.push(tokio::spawn(async move {
            loop {
                if let Err(error)=crate::processing::refresh_capacity(&capacity_service) {
                    capacity_service.subtitle_jobs.set_concurrency(0);
                    tracing::warn!(%error,"Processing capacity unavailable");
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }));
        let trials_service=service.clone();
        workers.push(tokio::spawn(async move {
            loop {
                if let Err(error)=crate::trials::tick(&trials_service).await {tracing::warn!(%error,"Torrent trial scheduler failed");}
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
        }));
        let formats = service.clone();
        workers.push(tokio::spawn(async move {
            loop {
                // Brisk while files are waiting to be described, idle otherwise.
                // Languages first: the format pass describes files that have a track list.
                let listed = crate::media_inventory::list_waiting(&formats).await
                    .unwrap_or_else(|error| { tracing::warn!(%error,"Language listing failed"); 0 });
                let pause = match crate::media_format::tick(&formats).await {
                    Ok(0) if listed == 0 => 15,
                    Ok(_) => 1,
                    Err(error) => { tracing::warn!(%error,"Media format pass failed"); 60 }
                };
                tokio::time::sleep(std::time::Duration::from_secs(pause)).await;
            }
        }));
        let background = service.clone();
        workers.push(tokio::spawn(async move {
            loop {
                if let Err(error) = crate::processing::tick(&background).await {
                    tracing::warn!(%error,"Media processing failed");
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
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
    let lan_task = lan_listener.map(|listener| {
        let router = router.clone();
        tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
        })
    });
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Install termination handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
        crate::processing::SHUTTING_DOWN.store(true, std::sync::atomic::Ordering::SeqCst);
    })
    .await?;
    if let Some(task) = lan_task {
        task.abort();
    }
    routes_task.abort();
    if let Some(task) = rendezvous_task { task.abort(); }
    if let Some(child) = &mut advertisement {
        let _ = child.kill().await;
    }
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
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    method: Method,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    if uri.path() == "/v1/lan/register" {
        if method != Method::POST || !crate::lan::trusted(peer.ip()) {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"error":"Automatic connection is available on the trusted LAN only"})),
            )
                .into_response();
        }
        let result = (|| -> Result<Value> {
            s.lan_limiter.admit(peer.ip())?;
            let body: Value = serde_json::from_slice(&bytes)?;
            let mut db =
                s.db.lock()
                    .map_err(|_| anyhow::anyhow!("Database unavailable"))?;
            crate::lan::register(&mut db, &body)
        })();
        return match result {
            Ok(value) => ([("Cache-Control", "no-store")], Json(value)).into_response(),
            Err(error) => (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":error.to_string()})),
            )
                .into_response(),
        };
    }
    if uri.path().starts_with("/v1/hls/") {
        return match hls_response(&s, &uri, &method, &headers).await {
            Ok(response) => response,
            Err(error) => (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":error.to_string()})),
            )
                .into_response(),
        };
    }
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
        .strip_prefix("/v1/subtitle-jobs/")
        .and_then(|p| p.strip_suffix(".vtt"))
    {
        let result = async {
            if method != Method::GET {
                bail!("Unsupported subtitle method");
            }
            let job = s.subtitle_jobs.status(id)?;
            media_file(&s, text(&job, "asset_id")?, 0).await?;
            let content = s.subtitle_jobs.artifact(id)?;
            Ok::<_, anyhow::Error>(
                (
                    [
                        ("Content-Type", "text/vtt; charset=utf-8"),
                        ("Cache-Control", "no-store"),
                    ],
                    content,
                )
                    .into_response(),
            )
        }
        .await;
        return match result {
            Ok(value) => value,
            Err(error) => (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":error.to_string()})),
            )
                .into_response(),
        };
    }
    if let Some((id, track)) = uri
        .path()
        .strip_prefix("/v1/library/assets/")
        .and_then(|p| p.split_once("/subtitles/"))
        .filter(|(_, track)| track.ends_with(".vtt"))
    {
        let result = async {
            if method != Method::GET || !id.starts_with("asset-") || id.contains('/') {
                bail!("Invalid subtitle request");
            }
            let track = track
                .strip_suffix(".vtt")
                .context("Invalid subtitle format")?
                .to_string();
            let media = media_file(&s, id, 0).await?;
            let content =
                tokio::task::spawn_blocking(move || crate::subtitles::read_sidecar(&media, &track))
                    .await??;
            Ok::<_, anyhow::Error>(
                (
                    [
                        ("Content-Type", "text/vtt; charset=utf-8"),
                        ("Cache-Control", "no-store"),
                    ],
                    content,
                )
                    .into_response(),
            )
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
            let parsed=reqwest::Url::parse(&format!("http://localhost{uri}"))?;
            if let Some((_,job))=parsed.query_pairs().find(|(key,_)|key=="virtual") {
                let reader=s.virtual_media.reader(&job,id,&path).await?;
                return virtual_response(reader,&job,&method,&headers).await;
            }
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
    let profile = match headers.get("x-torrbaby-profile") {
        Some(value) => match value.to_str() {
            Ok(value) if !value.is_empty() => value,
            _ => return (StatusCode::BAD_REQUEST, Json(json!({"error":"Invalid profile header"}))).into_response(),
        },
        None => crate::profiles::LEGACY_PROFILE,
    };
    let gzip = headers.get(axum::http::header::ACCEPT_ENCODING).and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|e| e.trim().split(';').next() == Some("gzip")));
    match handle(&s, &method, uri.path(), body, profile).await {
        Ok(value) if gzip => compressed_json(&value),
        Ok(value) => ([("Cache-Control", "no-store")], Json(value)).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":error.to_string()})),
        )
            .into_response(),
    }
}
/// JSON compressed for clients that accept it. The library alone is several megabytes and
/// shrinks to a fraction; over a slow route (a phone on a Tailscale relay) the plain
/// response did not arrive in time and the client showed Offline. Media is never compressed.
fn compressed_json(value: &Value) -> Response {
    let Ok(plain) = serde_json::to_vec(value) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":"Invalid response"}))).into_response();
    };
    let json_headers = [("Cache-Control", "no-store"), ("Content-Type", "application/json")];
    if plain.len() < 1024 { return (json_headers, plain).into_response(); }
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::with_capacity(plain.len() / 6), flate2::Compression::fast());
    match encoder.write_all(&plain).and_then(|_| encoder.finish()) {
        Ok(body) => ([("Cache-Control", "no-store"), ("Content-Type", "application/json"), ("Content-Encoding", "gzip"), ("Vary", "Accept-Encoding")], body).into_response(),
        Err(_) => (json_headers, plain).into_response(),
    }
}
/// Cloud shares for a title, each copy set against the series' episodes when the
/// metadata service knows the series. A failed lookup only leaves the totals out.
pub(crate) async fn cloud_search(s: &Service, query: &str) -> Result<Value> {
    let mut found = s.cloud.search(query).await?;
    let (title, season) = crate::cloud::title_and_season(query);
    let show = match s.metadata.series_episodes(&title).await {
        Ok(show) => show,
        Err(error) => { eprintln!("Cloud search: no episode list for this title: {error}"); Value::Null }
    };
    crate::cloud::cover(&mut found, &show, season);
    s.cloud.attach_measured(&mut found);
    Ok(found)
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .with_context(|| format!("{key} must be text"))
}

async fn handle(s: &Arc<Service>, method: &Method, path: &str, body: Value, profile: &str) -> Result<Value> {
    if method == Method::GET {
        if let Some(id) = path.strip_prefix("/v1/library/assets/").and_then(|p| p.strip_suffix("/manage-files")) {
            return crate::library_files::preview(s, id).await;
        }
    }
    if method == Method::POST {
        if let Some(id) = path.strip_prefix("/v1/library/assets/").and_then(|p|p.strip_suffix("/manage-files")) {
            if s.preview { bail!("File operations are disabled in migration preview mode"); }
            return crate::library_files::submit(s,id,text(&body,"action")?,text(&body,"confirmation_token")?).await;
        }
    }
    if method == Method::GET && path == "/v1/library/operations" {
        let db = s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        return Ok(json!({"operations":db.library_operations()?}));
    }
    if path == "/v1/profiles" {
        let db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
        if method == Method::GET { return Ok(json!({"profiles":db.profiles()?})); }
        if method == Method::POST { return db.write_profile(None, &body); }
    }
    if let Some(id) = path.strip_prefix("/v1/profiles/").filter(|id| !id.contains('/')) {
        if method == Method::PATCH {
            let db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
            return db.write_profile(Some(id), &body);
        }
    }
    if let Some(client) = path.strip_prefix("/v1/devices/").and_then(|p|p.strip_suffix("/profile")) {
        let db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
        if method == Method::GET { return db.device_profile(client); }
        if method == Method::PUT { return db.select_profile(client, text(&body, "profile_id")?); }
    }
    if method == Method::POST && path == "/v1/library/continue-watching" {
        let id=text(&body,"title_id")?;
        let hidden=body["hidden"].as_bool().context("hidden must be boolean")?;
        let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
        db.require_profile(profile)?;
        if hidden {
            // A second download of a movie is listed as <movie>:copy:<torrent>; it shares the movie's record.
            let known=id.split_once(":copy:").map_or(id,|(movie,_)|movie);
            anyhow::ensure!(db.documents("entities")?.iter().any(|v|v["id"]==known),"Title not found");
            db.connection.execute("INSERT INTO profile_hidden_titles(profile_id,title_id,hidden_at) VALUES(?,?,?) ON CONFLICT(profile_id,title_id) DO UPDATE SET hidden_at=excluded.hidden_at",rusqlite::params![profile,id,crate::storage::now()])?;
        } else { db.connection.execute("DELETE FROM profile_hidden_titles WHERE profile_id=? AND title_id=?",[profile,id])?; }
        return Ok(json!({"hidden":hidden}));
    }
    if method==Method::GET && path=="/v1/search/torrents/hashes" {
        let rows=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?.rows("SELECT listing_key,hash FROM search_torrent_hashes",[])?;
        return Ok(json!({"hashes":rows}));
    }
    if path=="/v1/trials" {
        return if method==Method::GET {crate::trials::list(s)} else if method==Method::POST {crate::trials::create(s,&body).await} else {bail!("Unsupported method")};
    }
    if method==Method::POST {
        if let Some(id)=path.strip_prefix("/v1/trials/") {return crate::trials::action(s,id,&body).await;}
    }
    if method == Method::POST && path == "/v1/search/torrents" && body["market"] == "china" {
        // Mainland titles are found as cloud-drive shares, not torrents. Every result has
        // been opened and holds real video; a share holding a series in several qualities
        // is one result for each, so each states its own picture and episodes.
        let query = text(&body, "query")?;
        let found = cloud_search(s, query).await?;
        let (show, _) = crate::cloud::title_and_season(query);
        let year = found["show"]["year"].as_str().and_then(|y| y.parse::<u16>().ok());
        let mut copies: Vec<(&Value, &Value, usize)> = Vec::new();
        for share in found["shares"].as_array().into_iter().flatten() {
            for (index, copy) in share["copies"].as_array().into_iter().flatten().enumerate() {
                // The same release shared by someone else: same picture and episodes, the same size to within 1%.
                let again = copies.iter().any(|(_, other, _)| other["resolution"] == copy["resolution"] && other["fps"] == copy["fps"] && other["episodes"] == copy["episodes"]
                    && other["total_bytes"].as_u64().zip(copy["total_bytes"].as_u64()).is_some_and(|(a, b)| a.abs_diff(b) <= a.max(b) / 100));
                if !again { copies.push((share, copy, index)); }
            }
        }
        copies.sort_by_key(|(_, copy, _)| crate::cloud::preference(copy));
        // The likeliest choices have their tracks read in the background, so their sound is known by the next look.
        let unread: Vec<(String, String)> = copies.iter().filter(|(_, copy, _)| copy["measured"].is_null()).take(5)
            .filter_map(|(share, copy, _)| Some((share["url"].as_str()?.to_owned(), copy["key"].as_str()?.to_owned()))).collect();
        if !unread.is_empty() {
            let service = s.clone();
            tokio::spawn(async move { for (url, copy) in unread { if let Err(error) = service.cloud.probe(&url, &copy).await { eprintln!("Cloud search: could not read a copy's tracks: {error}"); } } });
        }
        let items: Vec<crate::torrent_search::Item> = copies.into_iter().filter_map(|(share, copy, index)| {
            let url = share["url"].as_str()?;
            let id = url.rsplit('/').next()?.split('?').next()?;
            let several = share["copies"].as_array().is_some_and(|all| all.len() > 1);
            let title = share["title"].as_str().filter(|t| !t.is_empty()).unwrap_or(query);
            let folder = copy["folder"].as_str().and_then(|f| f.rsplit('/').next()).map(crate::cloud::visible).filter(|f| !f.is_empty());
            let name = match &folder { Some(folder) if several && folder != title => format!("{title} · {folder}"), _ => title.to_owned() };
            let mut item = crate::torrent_search::cloud_item(name, &if several { format!("{id}#{index}") } else { id.to_owned() }, url, copy["total_bytes"].as_u64(),
                share["posted"].as_str().filter(|p| !p.is_empty()).map(str::to_owned), copy["video_count"].as_u64().unwrap_or(0), &show, copy["key"].as_str());
            // What the files are, not what the title happens to say.
            let episodes: Vec<u16> = copy["episodes"].as_array().into_iter().flatten().filter_map(|n| n.as_u64().map(|n| n as u16)).collect();
            item.hints.year = year.or(item.hints.year);
            item.hints.season = copy["season"].as_u64().map(|n| n as u16); item.hints.season_end = None;
            item.hints.episode = episodes.first().copied();
            item.hints.episode_end = episodes.last().copied().filter(|_| episodes.len() > 1);
            item.hints.episode_count = (episodes.len() > 1).then_some(episodes.len() as u16);
            item.hints.resolution = copy["resolution"].as_str().map(str::to_owned);
            if let Some(codec) = copy["codec"].as_str() { item.hints.video_codec = Some(codec.to_owned()); }
            let mut detail = copy.clone();
            detail["show"] = found["show"]["title"].clone();
            detail["mirrors"] = json!(share["mirrors"].as_array().map_or(0, Vec::len));
            item.cloud = Some(detail);
            Some(item)
        }).collect();
        return Ok(serde_json::to_value(crate::torrent_search::SearchResult {
            query: query.to_owned(), providers: vec![crate::torrent_search::ProviderStatus { provider: "quark".into(), count: items.len(), error: None }], items, cached: false })?);
    }
    if method == Method::POST && path == "/v1/search/torrents" {
        let mut result=s.torrent_search.search(serde_json::from_value(body)?).await?;
        let known=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?.rows("SELECT listing_key,hash FROM search_torrent_hashes",[])?;
        let mut merged:Vec<crate::torrent_search::Item>=Vec::new();
        for mut item in result.items {
            if item.info_hash.is_empty() {
                let key=item.sources.iter().map(|s|format!("{}:{}",s.provider,s.id)).collect::<Vec<_>>().join(",");
                if let Some(row)=known.iter().find(|r|r["listing_key"]==key) {item.info_hash=row["hash"].as_str().unwrap_or("").to_owned();}
            }
            if let Some(existing)=merged.iter_mut().find(|r|!item.info_hash.is_empty() && r.info_hash.eq_ignore_ascii_case(&item.info_hash)) {
                for source in item.sources {if !existing.sources.iter().any(|s|s.provider==source.provider && s.id==source.id){existing.sources.push(source);}}
            } else {merged.push(item);}
        }
        result.items=merged;
        return Ok(serde_json::to_value(result)?);
    }
    if method == Method::POST && path == "/v1/search/torrents/artwork" {
        // Artwork is optional; a slow/unavailable metadata provider must not affect search.
        return Ok(tokio::time::timeout(std::time::Duration::from_secs(18),
            s.metadata.search_artwork(text(&body, "name")?, body["season"].as_u64())).await
            .ok().and_then(Result::ok).unwrap_or_else(|| json!({"data":null})));
    }
    if method == Method::POST && path == "/v1/search/torrents/resolve" {
        return s.torrent_search.resolve(text(&body,"provider")?,text(&body,"id")?,body["attachment"].as_str()).await;
    }
    if method == Method::POST && path == "/v1/search/torrents/summary" {
        return s.torrent_search.summary(text(&body,"provider")?,text(&body,"id")?).await;
    }
    if method == Method::POST && path == "/v1/search/torrents/detail" {
        if body["provider"] == "1lou" && body["explicit"] != true {
            return Ok(json!({"description":"", "files":null, "releases":[], "media_verified":false}));
        }
        return s
            .torrent_search
            .detail(text(&body, "provider")?, text(&body, "id")?)
            .await;
    }

    if method == Method::POST && path == "/v1/diagnostics/startup" {
        anyhow::ensure!(
            body["platform"] == "android" && body["build"].as_u64().is_some(),
            "Invalid startup report"
        );
        let stages = body["stages"]
            .as_array()
            .context("Missing startup stages")?;
        anyhow::ensure!(
            stages.len() <= 40
                && stages.iter().all(|stage| stage["stage"]
                    .as_str()
                    .is_some_and(|name| name.len() <= 64
                        && name.chars().all(|c| c.is_ascii_alphabetic() || c == ' '))
                    && stage["elapsed_ms"]
                        .as_u64()
                        .is_some_and(|ms| ms <= 3_600_000)),
            "Invalid startup stages"
        );
        s.event(
            "client_startup",
            json!({"platform":"android","build":body["build"],"stages":stages}),
        )?;
        return Ok(json!({"accepted":true}));
    }
    if method == Method::GET {
        if let Some(id) = path.strip_prefix("/v1/virtual-jobs/") { return s.virtual_media.status(id); }
    }
    if method == Method::POST {
        if let Some(id) = path.strip_prefix("/v1/library/assets/").and_then(|p| p.strip_suffix("/virtual-mp4")) {
            let input = media_file(s, id, 0).await?;
            // Subtitles made before the file was moved into the library follow it.
            s.subtitle_jobs.relocate(id, &input);
            let mut captions=Vec::new();
            if let Some(ids)=body.get("subtitle_jobs") {
                let ids=ids.as_array().context("Invalid subtitle jobs")?;
                anyhow::ensure!(ids.len()<=8,"Too many subtitles");
                for value in ids { let job=value.as_str().context("Invalid subtitle job")?;let status=s.subtitle_jobs.status(job)?;
                    anyhow::ensure!(status["asset_id"]==id,"Subtitle does not belong to this title");
                    // A subtitle that is no longer usable is left out; it must not stop the title from playing.
                    if status["state"]!="ready" { continue; }
                    captions.push(crate::virtual_captions::Caption {id:job.into(),language:status["target"].as_str().or(status["source"].as_str()).unwrap_or("und").into(),text:s.subtitle_jobs.artifact(job)?,name:caption_name(&body,captions.len(),&status),sdh:caption_sdh(s,id,&status)});
                }
            }
            // By content: Matroska named .mp4 must not be streamed directly to AVFoundation.
            if captions.is_empty() && !crate::container::is_matroska(&input) {
                return Ok(json!({"state":"direct", "id":""}));
            }
            return s.virtual_media.prepare(id, input, captions);
        }
    }
    if method == Method::GET {
        if let Some(id) = path.strip_prefix("/v1/hls-jobs/") {
            return s.hls.status(id);
        }
    }
    if method == Method::POST {
        if let Some(id) = path
            .strip_prefix("/v1/library/assets/")
            .and_then(|p| p.strip_suffix("/hls"))
        {
            let input = media_file(s, id, 0).await?;
            let ids = body["subtitle_jobs"]
                .as_array()
                .context("subtitle_jobs must be an array")?;
            anyhow::ensure!(ids.len() <= 8, "Too many subtitle tracks");
            let mut captions = Vec::new();
            for value in ids {
                let job_id = value.as_str().context("Invalid subtitle job")?;
                let status = s.subtitle_jobs.status(job_id)?;
                anyhow::ensure!(
                    status["asset_id"] == id && status["state"] == "ready",
                    "Subtitles do not belong to this media or are not ready"
                );
                let language = status["target"]
                    .as_str()
                    .or(status["source"].as_str())
                    .unwrap_or("und")
                    .to_owned();
                captions.push(crate::hls::Caption {
                    id: job_id.into(),
                    language,
                    text: s.subtitle_jobs.artifact(job_id)?,
                    name: caption_name(&body, captions.len(), &status),
                    sdh: caption_sdh(s, id, &status),
                });
            }
            return s.hls.prepare(id, input, captions);
        }
    }

    if method == Method::GET {
        if let Some(id) = path.strip_prefix("/v1/subtitle-jobs/") {
            let status = s.subtitle_jobs.status(id)?;
            media_file(s, text(&status, "asset_id")?, 0).await?;
            return Ok(status);
        }
    }
    if method == Method::POST {
        if let Some(id) = path
            .strip_prefix("/v1/library/assets/")
            .and_then(|p| p.strip_suffix("/subtitles/prepare"))
        {
            if !id.starts_with("asset-") || id.contains('/') {
                bail!("Invalid asset ID");
            }
            let input = media_file(s, id, 0).await?;
            let (input, track) = if let Some(sidecar) = body["sidecar"].as_str() {
                let sidecar = sidecar.to_owned();
                (
                    tokio::task::spawn_blocking(move || {
                        crate::subtitles::sidecar_path(&input, &sidecar)
                    })
                    .await??,
                    1,
                )
            } else {
                (
                    input,
                    body["track"].as_u64().context("Choose a subtitle track")?,
                )
            };
            let source = body
                .get("source")
                .map(|value| {
                    value
                        .as_str()
                        .context("Invalid source language")
                        .map(str::to_owned)
                })
                .transpose()?;
            let target = body
                .get("target")
                .map(|value| {
                    value
                        .as_str()
                        .context("Invalid target language")
                        .map(str::to_owned)
                })
                .transpose()?;
            return s.subtitle_jobs.prepare(id, input, track, source, target);
        }
    }
    if method == Method::GET {
        if let Some(id) = path
            .strip_prefix("/v1/library/assets/")
            .and_then(|p| p.strip_suffix("/progress"))
        {
            if !id.starts_with("asset-") || id.contains('/') {
                bail!("Invalid asset ID");
            }
            let db =
                s.db.lock()
                    .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
            return crate::offline::shared_progress(&db, id, profile);
        }
    }
    if method == Method::POST {
        if let Some(id) = path
            .strip_prefix("/v1/library/assets/")
            .and_then(|p| p.strip_suffix("/subtitles/prioritize"))
        {
            let db =
                s.db.lock()
                    .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
            return crate::media_inventory::prioritize(&db, id);
        }
    }
    if method == Method::GET {
        if let Some(id) = path
            .strip_prefix("/v1/library/assets/")
            .and_then(|p| p.strip_suffix("/media-info"))
        {
            let db =
                s.db.lock()
                    .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
            db.asset(id)?;
            if let Some(path)=db.asset(id).ok().and_then(|a|a["path"].as_str().map(PathBuf::from)) { s.subtitle_jobs.relocate(id,&path); }
            return crate::media_inventory::cached_with(&db, id, Some(s.subtitle_jobs.as_ref()));
        }
    }
    if method == Method::POST && path == "/v1/playback/offline" {
        let mut db =
            s.db.lock()
                .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
        return crate::offline::sync(&mut db, body);
    }
    if method == Method::GET {
        if let Some(id) = path
            .strip_prefix("/v1/library/assets/")
            .and_then(|p| p.strip_suffix("/subtitles"))
        {
            if !id.starts_with("asset-") || id.contains('/') {
                bail!("Invalid asset ID");
            }
            let media = media_file(s, id, 0).await?;
            let sidecar_media = media.clone();
            let tracks =
                tokio::task::spawn_blocking(move || crate::subtitles::discover(&sidecar_media))
                    .await??;
            {
                let db =
                    s.db.lock()
                        .map_err(|_| anyhow::anyhow!("Database unavailable"))?;
                crate::media_inventory::update_sidecar_languages(&db, id, &tracks)?;
            }
            let tracks: Vec<_> = tracks
                .into_iter()
                .map(|track| {
                    let mut value =
                        serde_json::to_value(&track).expect("Subtitle track serializes");
                    value["path"] = json!(format!(
                        "/v1/library/assets/{id}/subtitles/{}.vtt",
                        track.id
                    ));
                    value["format"] = json!("webvtt");
                    value
                })
                .collect();
            let (embedded, scanned, error) = match crate::native_subtitles::tracks(&media).await {
                Ok(value) => (value["tracks"].clone(), true, None),
                Err(error) => (json!([]), false, Some(error.to_string())),
            };
            return Ok(json!({"tracks":tracks,"embedded_tracks":embedded,
                "embedded_tracks_scanned":scanned,"embedded_error":error}));
        }
    }
    if method == Method::GET || method == Method::POST {
        if let Some(id) = path
            .strip_prefix("/v1/library/assets/")
            .and_then(|p| p.strip_suffix("/download-info"))
        {
            if !id.starts_with("asset-") || id.contains('/') {
                bail!("Invalid asset ID")
            }
            let path = media_file(s, id, 0).await?;
            if let Some(job)=body.get("virtual_job").and_then(Value::as_str) {
                let reader=s.virtual_media.reader(job,id,&path).await?;
                let name=path.with_extension("mp4").file_name().unwrap_or_default().to_string_lossy().into_owned();
                // A device holding an earlier copy (other subtitles, or none) can keep the
                // video and fetch only what follows `prefix`. The probes let it confirm
                // that its copy really is the same source before it touches the file.
                let reuse=tokio::task::spawn_blocking(move ||->Result<Value> {
                    let Some((prefix,mask))=reader.reusable_prefix() else {return Ok(json!({"size":reader.len(),"reuse":null}))};
                    let length=prefix.min(65_536);
                    let mut probes=Vec::new();
                    for at in [0,prefix/2,prefix-length] {
                        let mut data=vec![0u8;length as usize];
                        reader.read_at(at,&mut data)?;
                        use sha2::Digest;
                        let digest:String=sha2::Sha256::digest(&data).iter().map(|b|format!("{b:02x}")).collect();
                        probes.push(json!({"at":at,"length":length,"sha256":digest}));
                    }
                    Ok(json!({"size":reader.len(),"reuse":{"prefix":prefix,"patch_at":mask,"probes":probes}}))
                }).await??;
                return Ok(json!({"asset_id":id,"name":name,"size":reuse["size"],"etag":format!("\"{job}\""),"content_type":"video/mp4","representation":"virtual-mp4","reuse":reuse["reuse"]}));
            }
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
    // Drive changes only touch the settings in this server's database and each drive's marker.
    if let Some(result) = crate::drive_moves::route(s, method.as_str(), path, &body).await {
        return result;
    }
    if let Some(result) = crate::drive_settings::route(s, method.as_str(), path, &body).await {
        return result;
    }
    if path == "/v1/settings" && (method == Method::GET || method == Method::PATCH) {
        let _guard = s.mutation_lock.lock().await;
        let mut config = serde_json::to_value(s.effective_config()?)?;
        if method == Method::PATCH {
            let patch = body.as_object().context("Expected settings object")?;
            if patch.is_empty()
                || patch.keys().any(|k| {
                    ![
                        "paused",
                        "poll_seconds",
                        "source",
                        "movies",
                        "tv",
                        "background_subtitles",
                    ]
                    .contains(&k.as_str())
                })
            {
                bail!(
                    "Allowed settings: paused, poll_seconds, source, movies, tv, background_subtitles"
                );
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
            json!({"background_subtitles":config["background_subtitles"],"paused":config["paused"],"poll_seconds":config["poll_seconds"],"source":config["source"],"movies":config["movies"],"tv":config["tv"]}),
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
                profile,
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
                        profile,
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
                "prioritize",
                "unprioritize",
            ]
            .contains(&action)
            {
                let affected = if action == "rename" {
                    let torrent=crate::organizer::torrent(s,hash).await?;
                    let root=std::path::Path::new(text(&torrent,"download_dir")?).join(text(&body,"path")?);
                    s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?.documents("assets")?
                        .into_iter().filter(|a|a["torrent_hash"]==hash && a["path"].as_str().is_some_and(|p|std::path::Path::new(p).starts_with(&root)))
                        .filter_map(|a|a["id"].as_str().map(str::to_owned)).collect::<Vec<_>>()
                } else { Vec::new() };
                let result=crate::organizer::action(s, hash, action, &body).await?;
                if action == "rename" {
                    {
                        let db=s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?;
                        for id in affected {
                            let mut asset=db.asset(&id)?;
                            let name=text(&body,"name")?;
                            asset["metadata_query"]=json!(if crate::scanner::video(std::path::Path::new(name)) {
                                std::path::Path::new(name).file_stem().and_then(|s|s.to_str()).unwrap_or(name)
                            } else {name});
                            asset["entity_id"]=Value::Null;
                            asset["confirmed"]=json!(false);
                            asset["status"]=json!("pending");
                            asset["matched_at"]=json!(0);
                            asset["error"]=Value::Null;
                            asset["candidates"]=json!([]);
                            db.connection.execute("UPDATE library_assets SET document=? WHERE id=?",rusqlite::params![asset.to_string(),id])?;
                        }
                    }
                    let service=s.clone();
                    tokio::spawn(async move {
                        let refresh=async {
                            scan_library(&service).await?;
                            collect_metadata(&service).await?;
                            crate::organizer::tick(&service).await?;
                            scan_library(&service).await
                        }.await;
                        if let Err(error)=refresh {eprintln!("Refresh after rename: {error}");}
                    });
                }
                return Ok(result);
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
        // An explicit retry should not be delayed by the automatic failure backoff.
        s.db.lock().map_err(|_|anyhow::anyhow!("Database unavailable"))?.connection.execute(
            "UPDATE library_assets SET document=json_set(document,'$.matched_at',0) WHERE COALESCE(json_extract(document,'$.status'),'')!='matched' AND COALESCE(json_extract(document,'$.confirmed'),0)!=1",[])?;
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
    if path == "/v1/cloud/quark" {
        // The member's Quark login. It can be set or removed, never read back.
        if method == Method::PUT { s.cloud.set_cookie(text(&body, "cookie")?)?; }
        else if method == Method::DELETE { s.cloud.clear_cookie()?; }
        else if method != Method::GET { bail!("Unsupported method") }
        return s.cloud.status().await;
    }
    if method == Method::POST && path == "/v1/cloud/search" { return cloud_search(s, text(&body, "query")?).await; }
    // Shows watched for new episodes.
    if path == "/v1/watch" {
        if method == Method::POST { s.watcher.add(s, text(&body, "id")?, body["market"].as_str())?; }
        return Ok(s.watcher.list(s));
    }
    if method == Method::POST && path == "/v1/watch/check" { return Ok(s.watcher.check_now()); }
    if let Some(id) = path.strip_prefix("/v1/watch/") {
        if method == Method::DELETE { s.watcher.remove(id)?; return Ok(s.watcher.list(s)); }
        if method == Method::POST { s.watcher.add(s, id, body["market"].as_str())?; return Ok(s.watcher.list(s)); }
    }
    if method == Method::POST && path == "/v1/cloud/probe" { return s.cloud.probe(text(&body, "url")?, body["copy"].as_str().unwrap_or("")).await; }
    if method == Method::POST && path == "/v1/cloud/add" { return s.cloud.add(s, &body).await; }
    if method == Method::GET && path == "/v1/cloud/jobs" {
        // A download is named in the language it was found in; the library's English
        // title is given beside it once the library knows the show, or the metadata service does.
        let mut listed = s.cloud.jobs();
        let entities = s.db.lock().ok().and_then(|db| db.documents("entities").ok()).unwrap_or_default();
        for job in listed["jobs"].as_array_mut().into_iter().flatten() {
            let Some(title) = job["title"].as_str().map(str::to_owned) else { continue };
            let known = entities.iter().find(|e| e["original_title"] == title.as_str()).and_then(|e| e["title"].as_str()).map(str::to_owned);
            let english = match known { Some(found) => Some(found), None => s.metadata.series_english_title(&title).await };
            if let Some(english) = english.filter(|english| *english != title) { job["english"] = json!(english); }
        }
        return Ok(listed);
    }
    if let Some(job) = path.strip_prefix("/v1/cloud/jobs/") {
        if method == Method::DELETE { return Ok(s.cloud.dismiss(job)); }
        if let (true, Some(job)) = (method == Method::POST, job.strip_suffix("/retry")) { return Ok(s.cloud.retry(job)); }
        if let (true, Some(job)) = (method == Method::POST, job.strip_suffix("/next")) { return Ok(s.cloud.next(job)); }
    }
    if path == "/v1/private-site" {
        // The member's private site. The passkey can be set or removed, never read back.
        let file = s.torrent_search.private_site_path().context("Private sites are unavailable")?.to_owned();
        if method == Method::PUT {
            // An empty passkey keeps the saved one, so the minimums can be changed without re-entering it.
            let existing = s.torrent_search.private_site();
            let passkey = match body["passkey"].as_str().filter(|p| !p.trim().is_empty()) {
                Some(passkey) => passkey.to_owned(),
                None => existing.as_ref().map(|site| site.passkey().to_owned()).context("Enter your passkey")?,
            };
            let usual = crate::torrent_search::PrivateSite::new(text(&body, "base_url")?, &passkey)?;
            let hours = body["min_seed_hours"].as_f64().unwrap_or(existing.as_ref().map_or(usual.min_seed_hours, |site| site.min_seed_hours));
            let ratio = body["min_ratio"].as_f64().unwrap_or(existing.as_ref().map_or(usual.min_ratio, |site| site.min_ratio));
            usual.with_minimums(hours, ratio)?.save(&file)?;
            s.torrent_search.private_site_changed();
        } else if method == Method::DELETE {
            match std::fs::remove_file(&file) { Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()), _ => {} }
            s.torrent_search.private_site_changed();
        } else if method != Method::GET { bail!("Unsupported method") }
        let site = s.torrent_search.private_site();
        let usual = crate::torrent_search::PrivateSite::new("https://example.org", "0000000000000000")?;
        let shown = site.as_ref().unwrap_or(&usual);
        let (seconds, ratio) = shown.seeding_targets();
        return Ok(json!({"configured": site.is_some(), "base_url": site.as_ref().map(|s| s.base_url.clone()),
            "min_seed_hours": shown.min_seed_hours, "min_ratio": shown.min_ratio, "seed_hours": seconds / 3600.0, "seed_ratio": ratio}));
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
            return playback_link(s, id, &body, profile).await;
        }
        if let Some(hash) = path
            .strip_prefix("/v1/torrents/")
            .and_then(|s| s.strip_suffix("/play"))
        {
            return playback_link(s, hash, &body, profile).await;
        }
    }
    if method == Method::GET && path == "/v1/torrents" {
        let cached = s
            .torrents
            .lock()
            .map_err(|_| anyhow::anyhow!("Torrent cache lock failed"))?
            .as_ref()
            .filter(|(updated, _)| updated.elapsed().as_secs() < 15)
            .map(|(_, torrents)| json!(torrents));
        if let Some(torrents) = cached {
            let db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
            let operations = db.library_operations()?;
            return Ok(library_issues(crate::library_files::annotate_torrents(torrents, &operations), &db.state()?));
        }
        let torrents = s
            .rpc
            .call("torrent_get", json!({"fields":crate::transmission::FIELDS}))
            .await?["torrents"]
            .take();
        let db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
        let operations = db.library_operations()?;
        return Ok(library_issues(crate::library_files::annotate_torrents(torrents, &operations), &db.state()?));
    }
    let effective = if path == "/v1/status" || path == "/v1/storage" || path.starts_with("/v1/attention") {
        Some(s.effective_config()?)
    } else {
        None
    };
    // No lock is held across an await. SQLite operations remain short and transactional.
    let mut db =
        s.db.lock()
            .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
    if method == Method::GET {
        if let Some(id)=path.strip_prefix("/v1/library/assets/").and_then(|p|p.strip_suffix("/credits")) {
            return crate::credits::cached(&db,id);
        }
        if let Some(hash) = path
            .strip_prefix("/v1/torrents/")
            .and_then(|s| s.strip_suffix("/rates"))
        {
            return Ok(json!(db.torrent_rates(hash)?));
        }
    }
    if let Some(client) = path
        .strip_prefix("/v1/devices/")
        .and_then(|p| p.strip_suffix("/subtitle-preferences"))
    {
        return match method.as_str() {
            "GET" => db.device_subtitle_preferences(client, None),
            "PUT" => db.device_subtitle_preferences(client, Some(body)),
            _ => bail!("Unsupported subtitle preferences method"),
        };
    }
    if method == Method::GET {
        if let Some(title) = path.strip_prefix("/v1/library/titles/").and_then(|p| p.strip_suffix("/missing-episodes")) {
            return db.missing_episodes(title);
        }
        if let Some(id) = path.strip_prefix("/v1/punch/") {
            return crate::punch::TESTS.status(id);
        }
        if let Some(id) = path.strip_prefix("/v1/direct/") {
            return crate::direct::DIRECT.status(id);
        }
        if let Some(title) = path.strip_prefix("/v1/library/titles/").and_then(|p| p.strip_suffix("/details")) {
            return db.catalog_details(profile, title);
        }
    }
    if let Some(title) = path
        .strip_prefix("/v1/library/titles/")
        .and_then(|p| p.strip_suffix("/subtitle-preferences"))
    {
        return match method.as_str() {
            "GET" => db.subtitle_preferences(Some(title)),
            "PUT" => db.save_subtitle_preferences(Some(title), body),
            "DELETE" => db.reset_subtitle_preferences(title),
            _ => bail!("Unsupported subtitle preferences method"),
        };
    }
    match (method.as_str(),path) {
        ("GET","/v1/subtitle-preferences") => db.subtitle_preferences(None),
        ("PUT","/v1/subtitle-preferences") => db.save_subtitle_preferences(None, body),
        ("GET","/v1/storage") => {
            let config = effective.as_ref().context("Missing settings")?;
            Ok(crate::drives::Drives::of(config).report())
        },
        ("GET","/v1/library/revision") => Ok(json!({"revision":db.catalog_revision()?,"content":db.part_revision("content")?,"progress":db.part_revision("progress")?})),
        ("GET","/v1/library/list") => db.catalog_list(profile),
        ("GET","/v1/library/progress") => db.catalog_progress(profile),
        ("GET","/v1/processing") => crate::processing::summary(&db),
        ("GET","/v1/processing/failures") => Ok(json!({
            "failures":db.rows("SELECT * FROM processing_failures ORDER BY resolved_at IS NULL DESC,last_seen DESC",[])?,
            "patterns":db.rows("SELECT stage,error,COUNT(*) AS affected_files,SUM(occurrences) AS occurrences FROM processing_failures WHERE resolved_at IS NULL GROUP BY stage,error ORDER BY affected_files DESC",[])?
        })),
        ("POST","/v1/processing/rescan") => crate::processing::rescan(&db,&body),
        ("GET","/v1/attention") => {
            let mut list = crate::processing::attention(&db)?;
            if let (Some(config), Some(items)) = (effective.as_ref(), list["items"].as_array_mut()) {
                // A missing drive is one item, ahead of everything else.
                let drives = crate::drives::Drives::of(config).attention();
                items.splice(0..0, drives);
            }
            Ok(list)
        },
        ("POST","/v1/attention/dismiss") => {
            if let Some(config) = effective.as_ref() {
                let ids: Vec<i64> = body["ids"].as_array().into_iter().flatten().filter_map(Value::as_i64).collect();
                crate::drives::Drives::of(config).dismiss(&ids, body["all"] == true);
            }
            crate::processing::dismiss_attention(&db,&body)
        },
        ("GET","/v1/credits") => crate::credits::summary(&db),
        ("GET","/v1/library") => {
            let mut catalog = db.catalog(profile)?;
            catalog["revision"] = json!(db.catalog_revision()?);
            Ok(catalog)
        },
        ("GET","/v1/library/matches") => Ok(json!(db.visible_assets()?.into_iter().filter(|a|a["available"]==true && a["status"]!="matched" && a["role"]!="extra").collect::<Vec<_>>())),
        // The online check: who the server is and where it is reached, a few hundred bytes.
        // /v1/status (with the whole processing queue) is for the screens that show it.
        ("GET","/v1/ping") => {
            let state = db.state()?;
            Ok(json!({"version":env!("CARGO_PKG_VERSION"),"server":{"name":state["server"]["name"],"id":state["server"]["id"],"routes":ROUTES.read().map(|r|r.clone()).unwrap_or_default(),"relay":crate::rendezvous::advertised()}}))
        }
        ("GET","/v1/status") => {
            let state = db.state()?;
            let config = effective.as_ref().context("Missing settings")?;
            Ok(json!({"version":env!("CARGO_PKG_VERSION"),"server":{"name":state["server"]["name"],"id":state["server"]["id"],"routes":ROUTES.read().map(|r|r.clone()).unwrap_or_default(),"relay":crate::rendezvous::advertised()},"uptime_seconds":s.started.elapsed().as_secs(),"rendezvous":crate::rendezvous::status(),
                "settings":{"background_subtitles":config.background_subtitles,"source":config.source,"movies":config.movies,"tv":config.tv,"poll_seconds":config.poll_seconds,"paused":s.preview || config.paused},
                "storage":crate::drives::Drives::of(&config).report(),
                "credits_analysis":crate::credits::summary(&db)?,"subtitle_preparation":crate::media_inventory::summary(&db,&s.subtitle_jobs)?,"last_poll":state["last_poll"],"last_error":if s.preview { json!("Migration preview: organizer not enabled") } else {state["last_error"].clone()},"pending_scans":if state["library_scan"]["scanning"] == true {1} else {0},"scan_error":state["library_scan"]["error"],"jobs":state["jobs"]}))
        }
        ("PUT","/v1/server") => {
            // The name is the owner's to choose; the id never changes, so devices keep
            // everything they have across a rename.
            let name = body["name"].as_str().unwrap_or_default().trim().to_owned();
            anyhow::ensure!(!name.is_empty() && name.chars().count() <= 64 && !name.chars().any(char::is_control), "Choose a name of up to 64 characters");
            let mut state = db.state()?;
            state["server"]["name"] = json!(name);
            db.save_state(&state)?;
            Ok(json!({"name":state["server"]["name"],"id":state["server"]["id"]}))
        }
        ("POST","/v1/punch") => { drop(db); return crate::punch::TESTS.start(&body).await; }
        ("POST","/v1/direct") => { drop(db); return crate::direct::DIRECT.start(&body, s.config.listen_port).await; }
        ("GET","/v1/events") => Ok(db.state()?["events"].take()),
        ("GET","/v1/clients") => Ok(json!(db.rows("SELECT clients.*, EXISTS(SELECT 1 FROM device_credentials WHERE client_id=clients.id) AS paired_credential FROM clients ORDER BY last_seen DESC",[])?)),
        ("POST","/v1/clients") => db.register(text(&body,"client_id")?,text(&body,"name")?,text(&body,"platform")?),
        ("GET","/v1/playback") => Ok(json!(db.history(profile)?)),
        ("POST","/v1/playback/end") => {
            let session=text(&body,"session_id")?;
            let tx=db.connection.transaction()?;
            tx.execute("UPDATE playback_sessions SET ended=1 WHERE id=?",[session])?;
            tx.execute("UPDATE profile_playback SET active_session=NULL WHERE active_session=?",[session])?;
            tx.commit()?;
            Ok(json!({"accepted":true}))
        },
        ("POST","/v1/playback/progress") => db.progress_with_evidence(text(&body,"session_id")?,body["sequence"].as_i64().context("Invalid sequence")?,body["position"].as_f64().context("Invalid position")?,body["duration"].as_f64().context("Invalid duration")?, match body.get("ended") {None => false,Some(v)=>v.as_bool().context("ended must be boolean")?}, body.get("watched_eligible").map(|v|v.as_bool().context("watched_eligible must be boolean")).transpose()?.unwrap_or(true), body.get("reached_end").map(|v|v.as_bool().context("reached_end must be boolean")).transpose()?.unwrap_or(false)),
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

/// The file to prepare for an asset: as `media_file`, or the finished copy of a
/// file whose package is still downloading. Never used to serve playback.
pub(crate) async fn processing_file(s: &Service, id: &str) -> Result<PathBuf> {
    let asset = s.db.lock().map_err(|_| anyhow::anyhow!("Database lock failed"))?.asset(id)?;
    if asset["complete"] == true { return media_file(s, id, 0).await; }
    let path = crate::scanner::work_path(&asset).context("Download is not complete")?;
    let drives = crate::drives::Drives::of(&s.effective_config()?);
    let root = drives.roots(crate::drives::Role::Downloads).find(|root| path.starts_with(root)).context("Media is outside managed roots")?;
    let relative = path.strip_prefix(root).ok().and_then(|r| r.to_str()).context("Media is outside managed roots")?;
    let path = checked_path(root, relative)?;
    anyhow::ensure!(path.metadata().is_ok_and(|m| m.is_file() && Some(m.len()) == asset["size"].as_u64()), "Media is unavailable or changed");
    Ok(path)
}
pub(crate) async fn media_file(s: &Service, hash: &str, index: u64) -> Result<PathBuf> {
    {
        let db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
        if hash.starts_with("asset-") {
            if db.hidden_asset(&db.asset(hash)?)? { bail!("This title has been removed from the library"); }
        } else if db.hidden_package(hash)? { bail!("This package has been removed from the library"); }
    }

    if hash.starts_with("asset-") {
        if index != 0 {
            bail!("Invalid asset file index");
        }
        let asset =
            s.db.lock()
                .map_err(|_| anyhow::anyhow!("Database lock failed"))?
                .asset(hash)?;
        // Asked of the drive now: the mark is only as fresh as the last scan, and a drive
        // that has just come back plays at once.
        if asset["offline"] == true && (asset["removed_drive"].is_string() || asset["path"].as_str().is_none_or(|path| {
            let drives = crate::drives::Drives::of(&s.effective_config().unwrap_or_else(|_| s.config.clone()));
            drives.offline(std::path::Path::new(path), &drives.online())
        })) {
            bail!("This title is on a drive that is not connected");
        }
        if asset["complete"] != true {
            bail!("Download is not complete");
        }
        let path = PathBuf::from(text(&asset, "path")?);
        let mut allowed = None;
        let drives = crate::drives::Drives::of(&s.effective_config()?);
        if let Some((_, _, root)) = drives.root_of(&path) {
            if let Ok(relative) = path.strip_prefix(root) {
                allowed = Some(checked_path(
                    root,
                    relative.to_str().context("Invalid media path")?,
                )?);
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
    if !crate::drives::Drives::of(&s.effective_config()?).is_root(root) {
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

async fn playback_link(s: &Service, hash: &str, body: &Value, profile: &str) -> Result<Value> {
    let _mutation = s.mutation_lock.lock().await;
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
    if let Some(id) = body.get("hls_job") {
        let id = id.as_str().context("Invalid HLS job")?;
        anyhow::ensure!(
            s.hls.asset(id)? == hash && s.hls.status(id)?["state"] == "ready",
            "HLS package is not ready for this asset"
        );
        let expires = s.hls.status(id)?["expires"]
            .as_u64()
            .context("Invalid HLS expiry")?;
        result["path"] = json!(format!(
            "/v1/hls/{id}/media.m3u8?expires={expires}&sig={}",
            signature(&s.token, &format!("hls-{id}"), 0, expires)
        ));
        result["expires"] = json!(expires);
    }
    if let Some(id) = body.get("virtual_job") {
        let id = id.as_str().context("Invalid virtual media job")?;
        anyhow::ensure!(body.get("hls_job").is_none(), "Choose one playback container");
        let _ = s.virtual_media.reader(id, hash, &path).await?;
        let key = format!("virtual:{hash}:{id}");
        result["path"] = json!(format!("/v1/stream/{hash}/{index}?virtual={id}&expires={expires}&sig={}", signature(&s.token, &key, index, expires)));
        result["container"] = json!("mp4");
    }
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
            profile,
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
    let signed_key = if let Some(id) = query.get("virtual") { format!("virtual:{}:{id}", parts[2]) } else {parts[2].to_owned()};
    mac.update(format!("{signed_key}:{index}:{expires}").as_bytes());
    mac.verify_slice(&decoded?)
        .map_err(|_| anyhow::anyhow!("Invalid playback signature"))?;
    let path = media_file(s, parts[2], index).await?;
    if let Some(id) = query.get("virtual") {
        let reader = s.virtual_media.reader(id, parts[2], &path).await?;
        return virtual_response(reader, id, method, headers).await;
    }
    file_response(&path, method, headers).await
}

async fn hls_response(
    s: &Service,
    uri: &axum::http::Uri,
    method: &Method,
    headers: &HeaderMap,
) -> Result<Response> {
    use hmac::{Hmac, Mac};
    anyhow::ensure!(
        method == Method::GET || method == Method::HEAD,
        "Invalid HLS method"
    );
    let tail = uri
        .path()
        .strip_prefix("/v1/hls/")
        .context("Invalid HLS route")?;
    let (id, name) = tail.split_once('/').context("Invalid HLS path")?;
    let parsed = reqwest::Url::parse(&format!("http://localhost{uri}"))?;
    let query: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
    let expires: u64 = query.get("expires").context("Missing expiry")?.parse()?;
    let sig = query.get("sig").context("Missing signature")?;
    anyhow::ensure!(expires > now() as u64 && sig.len() == 64, "Expired HLS URL");
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
    mac.update(format!("hls-{id}:0:{expires}").as_bytes());
    mac.verify_slice(&decoded?)
        .map_err(|_| anyhow::anyhow!("Invalid HLS signature"))?;
    let asset = s.hls.asset(id)?;
    media_file(s, &asset, 0).await?;
    let path = s.hls.file(id, name)?;
    if path.extension().is_some_and(|s| s == "m3u8") {
        let text = tokio::fs::read_to_string(path).await?;
        let text = crate::hls::signed_playlist(&text, &format!("expires={expires}&sig={sig}"));
        return Ok(Response::builder()
            .header("Content-Type", "application/vnd.apple.mpegurl")
            .header("Cache-Control", "private, no-store")
            .header("Content-Length", text.len())
            .body(if method == Method::HEAD {
                axum::body::Body::empty()
            } else {
                axum::body::Body::from(text)
            })?);
    }
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
    if let Some(sniffed) = crate::container::media_type(path) {
        return sniffed;
    }
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "mp4" | "m4v" | "m4s" => "video/mp4",
        "vtt" => "text/vtt; charset=utf-8",
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

/// The track name for the `index`th requested subtitle job: the client's
/// localized `subtitle_names` entry when it sent one, otherwise the server's
/// English short name ("English Text", "Chinese Translated").
/// SDH by its text (the job) or by its source track's flag or name (inventory).
fn caption_sdh(s: &Service, asset: &str, status: &Value) -> bool {
    status["sdh"] == true || s.db.lock().ok().and_then(|db| crate::media_inventory::cached(&db, asset).ok()).is_some_and(|info| {
        info["prepared"].as_array().is_some_and(|p| p.iter().any(|j| j["id"] == status["id"] && j["sdh"] == true))
    })
}
fn caption_name(body: &Value, index: usize, status: &Value) -> String {
    let supplied = body["subtitle_names"].get(index).and_then(Value::as_str)
        .map(|n| n.trim().chars().filter(|c| !c.is_control() && *c != '"').take(64).collect::<String>())
        .filter(|n| !n.is_empty());
    supplied.unwrap_or_else(|| status["name"].as_str().unwrap_or("").to_owned())
}
pub(crate) async fn virtual_response(
    reader: Arc<crate::virtual_captions::Reader>,
    id: &str,
    method: &Method,
    headers: &HeaderMap,
) -> Result<Response> {
    let size = reader.len();
    let revision = format!("\"{id}\"");
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
    let mut response = Response::builder()
        .status(if range.is_some() { 206 } else { 200 })
        .header("Accept-Ranges", "bytes")
        .header("X-Torrbaby-Source", "virtual-mp4")
        .header("Cache-Control", "private, no-store")
        .header("Content-Length", end - start + 1)
        .header("ETag", &revision)
        .header("Content-Type", "video/mp4");
    if range.is_some() {
        response = response.header("Content-Range", format!("bytes {start}-{end}/{size}"));
    }
    let body = if method == Method::HEAD {
        axum::body::Body::empty()
    } else {
        let chunks = futures_util::stream::try_unfold((reader, start, end + 1), |(reader, at, end)| async move {
            if at == end { return Ok::<_, std::io::Error>(None); }
            let count = (end - at).min(256 * 1024) as usize;
            let source = reader.clone();
            let data = tokio::task::spawn_blocking(move || {
                let mut data = vec![0; count];
                let n = source.read_at(at, &mut data)?;
                if n != count { return Err(std::io::Error::other("Virtual media ended early")); }
                Ok::<_, std::io::Error>(data)
            }).await.map_err(std::io::Error::other)??;
            Ok(Some((data, (reader, at + count as u64, end))))
        });
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

/// Why a finished download has not been filed into the library, for the
/// clients' "Needs attention" list (for example, a title waiting to be identified).
fn library_issues(mut torrents: Value, state: &Value) -> Value {
    if let Some(rows) = torrents.as_array_mut() {
        for torrent in rows {
            let job = &state["jobs"][torrent["hash_string"].as_str().unwrap_or("")];
            if job["phase"] == "blocked" && torrent["percent_done"].as_f64().is_some_and(|done| done >= 1.0) {
                if let Some(reason) = job["error"].as_str().filter(|reason| !reason.is_empty()) {
                    torrent["library_issue"] = json!(reason);
                }
            }
        }
    }
    torrents
}

async fn artwork(s: &Service, id: &str, variant: &str) -> Result<Value> {
    use base64::Engine;
    if !["poster", "backdrop", "logo", "episode"].contains(&variant) {
        bail!("Unknown artwork variant");
    }
    // A second download of a movie (<movie>:copy:<torrent>) uses the movie's artwork.
    let id = id.split_once(":copy:").map_or(id, |(movie, _)| movie);
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
    if url.starts_with('/') { return Ok(json!({"data":null})); }
    let bytes = cached_artwork(s, &url).await?;
    Ok(json!({"data":base64::engine::general_purpose::STANDARD.encode(bytes)}))
}

fn artwork_key(url: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(url.as_bytes()).iter().map(|b|format!("{b:02x}")).collect()
}

async fn cached_artwork(s: &Service, url: &str) -> Result<Vec<u8>> {
    let key = artwork_key(url);
    let cache = s.data_dir.join("artwork");
    tokio::fs::create_dir_all(&cache).await?;
    let path = cache.join(format!("{key}.image"));
    let bytes = if path.exists() {
        tokio::fs::read(&path).await?
    } else {
        let parsed = reqwest::Url::parse(url)?;
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
    Ok(bytes)
}

fn library_artwork_urls(assets: &[Value], entities: &[Value]) -> std::collections::BTreeSet<String> {
    let mut urls = std::collections::BTreeSet::new();
    for asset in assets.iter().filter(|a| a["available"] == true || a["offline"] == true || a["torrent_present"] == true) {
        let Some(entity) = entities.iter().find(|e|e["id"] == asset["entity_id"] && e["id"].is_string()) else { continue; };
        for field in ["thumbnail", "backdrop", "logo"] {
            if let Some(url) = entity[field].as_str().filter(|url|url.starts_with("https://")) { urls.insert(url.to_owned()); }
        }
        if let Some(episode) = entity["episodes"].as_array().and_then(|episodes|episodes.iter().find(|e|
            e["season"] == asset["season"] && e["episode"] == asset["episode"] && e["episode"].is_number())) {
            if let Some(url) = episode["thumbnail"].as_str().filter(|url|url.starts_with("https://")) { urls.insert(url.to_owned()); }
        }
    }
    urls
}

fn artwork_retry_delay(attempts: u64) -> f64 { (60u64 * (1u64 << attempts.min(6))).min(3600) as f64 }

#[cfg(test)]
mod artwork_preparation_tests {
    use super::*;
    #[test]
    fn pending_assets_prepare_shared_artwork_and_only_their_episode_images() {
        let assets = vec![json!({"entity_id":"show","available":false,"torrent_present":true,"complete":false,"season":1,"episode":1}),
            json!({"entity_id":"show","available":true,"season":1,"episode":2}),
            json!({"entity_id":"gone","available":false,"torrent_present":false})];
        let entities = vec![json!({"id":"show","thumbnail":"https://image.tmdb.org/poster",
            "backdrop":"https://image.tmdb.org/backdrop","logo":"https://image.tmdb.org/logo",
            "episodes":[{"season":1,"episode":1,"thumbnail":"https://image.tmdb.org/one"},
                {"season":1,"episode":2,"thumbnail":"https://image.tmdb.org/two"},
                {"season":1,"episode":3,"thumbnail":"https://image.tmdb.org/not-downloaded"}]}),
            json!({"id":"gone","thumbnail":"https://image.tmdb.org/gone"})];
        let urls = library_artwork_urls(&assets,&entities);
        assert_eq!(urls.len(),5);
        assert!(urls.contains("https://image.tmdb.org/one"));
        assert!(urls.contains("https://image.tmdb.org/two"));
        assert!(!urls.contains("https://image.tmdb.org/not-downloaded"));
        assert!(!urls.contains("https://image.tmdb.org/gone"));
        assert_eq!(artwork_retry_delay(1),120.0);
        assert_eq!(artwork_retry_delay(u64::MAX),3600.0);
    }
}

async fn prefetch_library_artwork(s: &Service) -> Result<()> {
    use futures_util::{stream, StreamExt};
    let urls = {
        let db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
        // visible_assets excludes removal intent; it deliberately includes pending downloads.
        let mut urls = library_artwork_urls(&db.visible_assets()?, &db.documents("entities")?);
        for record in db.documents("metadata")? {
            if db.hidden_package(record["hash"].as_str().unwrap_or(""))? { continue; }
            if let Some(url) = record["selected"]["thumbnail"].as_str().filter(|u|u.starts_with("https://")) {
                urls.insert(url.to_owned());
            }
        }
        urls
    };
    let cache = s.data_dir.join("artwork");
    tokio::fs::create_dir_all(&cache).await?;
    stream::iter(urls).map(|url| {
        let cache = &cache;
        async move {
            let key = artwork_key(&url);
            if tokio::fs::try_exists(cache.join(format!("{key}.image"))).await? { return Ok::<(),anyhow::Error>(()); }
            let state_path = cache.join(format!("{key}.fetch.json"));
            let previous: Value = match tokio::fs::read(&state_path).await {
                Ok(data) => serde_json::from_slice(&data).unwrap_or(Value::Null),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Null,
                Err(error) => return Err(error.into()),
            };
            if previous["retry_at"].as_f64().unwrap_or(0.0) > now() { return Ok(()); }
            match cached_artwork(s, &url).await {
                Ok(_) => { let _ = tokio::fs::remove_file(state_path).await; }
                Err(error) => {
                    let attempts = previous["attempts"].as_u64().unwrap_or(0).saturating_add(1);
                    let state = json!({"url":url,"attempts":attempts,"retry_at":now()+artwork_retry_delay(attempts),"error":error.to_string()});
                    let temporary = cache.join(format!("{key}.{}.tmp", uuid::Uuid::new_v4()));
                    tokio::fs::write(&temporary, serde_json::to_vec(&state)?).await?;
                    tokio::fs::rename(&temporary, &state_path).await?;
                    tracing::debug!(%url,%error,"Artwork will retry in background");
                }
            }
            Ok(())
        }
    }).buffer_unordered(4).for_each(|result| async move {
        if let Err(error) = result { tracing::warn!(%error,"Could not persist artwork preparation state"); }
    }).await;
    Ok(())
}

pub(crate) async fn scan_library(s: &Service) -> Result<()> {
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
    // Files in Transmission's incomplete directory belong to torrents being downloaded.
    let incomplete = s.rpc.call("session_get", json!({"fields":["incomplete_dir","incomplete_dir_enabled"]})).await.ok()
        .filter(|session| session["incomplete_dir_enabled"] == true)
        .and_then(|session| session["incomplete_dir"].as_str().map(std::path::PathBuf::from));
    let config = s.effective_config()?;
    let result =
        tokio::task::spawn_blocking(move || crate::scanner::inventory_with(&config, old, torrents, incomplete.as_deref()))
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
                        "metadata_query",
                        "error",
                    ] {
                        if let Some(value) = current.get(key) {
                            asset[key] = value.clone();
                        }
                    }
                    // Metadata matching can finish while the filesystem scan is running.
                    // Keep explicit mappings and episode titles resolved against the provider.
                    if current["confirmed"] == true
                        || (current["episode_title"].is_string()
                            && current["entity_id"].is_string()
                            && current["season"].is_number())
                    {
                        for key in ["season", "episode"] {
                            if let Some(value) = current.get(key) {
                                asset[key] = value.clone();
                            }
                        }
                    }
                }
                tx.execute(
                    "INSERT OR REPLACE INTO library_assets VALUES(?,?)",
                    rusqlite::params![id, asset.to_string()],
                )?;
            }
            for id in &inventory.superseded {
                drop_superseded_asset(&tx, id)?;
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

/// Remove an untracked duplicate of a torrent file (Transmission's incomplete-directory
/// copy) together with its per-asset caches. The torrent-tracked record keeps the file's
/// match, playback and processing. A record that has gained an explicit match, a library
/// file operation or playback history of its own is kept.
fn drop_superseded_asset(tx: &rusqlite::Transaction, id: &str) -> Result<bool> {
    use rusqlite::OptionalExtension;
    let Some(document) = tx.query_row("SELECT document FROM library_assets WHERE id=?", [id], |r| r.get::<_, String>(0)).optional()? else {
        return Ok(false);
    };
    let asset: Value = serde_json::from_str(&document)?;
    if !asset["torrent_hash"].is_null() || asset["confirmed"] == true {
        return Ok(false);
    }
    let media = format!("{id}:{}", asset["file_index"].as_u64().unwrap_or(0));
    let referenced: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM library_operation_assets WHERE asset_id=?1)
            OR EXISTS(SELECT 1 FROM profile_playback WHERE media_id=?2)
            OR EXISTS(SELECT 1 FROM playback WHERE media_id=?2)
            OR EXISTS(SELECT 1 FROM playback_sessions WHERE media_id=?2)",
        rusqlite::params![id, media], |r| r.get(0))?;
    if referenced {
        return Ok(false);
    }
    for table in ["media_inventory", "credits_analysis", "media_processing", "processing_failures", "subtitle_priorities"] {
        tx.execute(&format!("DELETE FROM {table} WHERE asset_id=?"), [id])?;
    }
    tx.execute("DELETE FROM library_assets WHERE id=?", [id])?;
    Ok(true)
}

/// Fields filled in later by background backfills. A saved match snapshot taken
/// before they existed must not erase them when it is written again.
const BACKFILLED_ENTITY_FIELDS: [&str; 3] = ["series_status", "last_air_date", "cast"];
fn keep_backfilled(connection: &rusqlite::Connection, id: &str, mut entity: Value) -> Result<Value> {
    use rusqlite::OptionalExtension;
    let stored: Option<String> = connection
        .query_row("SELECT document FROM library_entities WHERE id=?", [id], |r| r.get(0))
        .optional()?;
    if let Some(stored) = stored.and_then(|text: String| serde_json::from_str::<Value>(&text).ok()) {
        for field in BACKFILLED_ENTITY_FIELDS {
            let missing = entity[field].is_null() || entity[field].as_str() == Some("");
            let known = !(stored[field].is_null() || stored[field].as_str() == Some(""));
            if missing && known && entity.is_object() { entity[field] = stored[field].clone(); }
        }
    }
    Ok(entity)
}
fn save_match(s: &Service, id: &str, entity: Value, confirmed: bool) -> Result<()> {
    let mut db =
        s.db.lock()
            .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
    let mut asset = db.asset(id)?;
    if asset["confirmed"] == true && !confirmed {
        return Ok(());
    }
    crate::scanner::resolve_episode_title(&mut asset, &entity);
    let entity_id = entity["id"].as_str().context("Invalid metadata identity")?.to_owned();
    let entity_id = entity_id.as_str();
    asset["entity_id"] = json!(entity_id);
    asset["matched_at"] = json!(now());
    asset["status"] = json!("matched");
    asset["match_confidence"] = entity["match_confidence"].clone();
    asset["error"] = Value::Null;
    if confirmed {
        asset["confirmed"] = json!(true);
    }
    let tx = db.connection.transaction()?;
    let entity = keep_backfilled(&tx, entity_id, entity)?;
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
    let entities=s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?.documents("entities")?;
    // Background backfills are capped per pass and back off per entity after a
    // failure, so a title TMDB cannot serve is not re-requested every minute.
    let mut lookups=0;
    for entity in entities.iter().filter(|entity| entity["provider"]=="TMDB" && !entity["cast"].is_array()) {
        let key=entity["id"].as_str().unwrap_or_default();
        if lookups>=3 || s.metadata.recently_failed(key).await {continue;}
        lookups+=1;
        match s.metadata.cast(entity).await {
            Ok(cast) => {
                let id=text(entity,"id")?;
                let db=s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
                if let Some(row)=db.rows("SELECT document FROM library_entities WHERE id=?",[id])?.first() {
                    let mut current:Value=serde_json::from_str(text(row,"document")?)?;
                    current["cast"]=cast;
                    db.connection.execute("UPDATE library_entities SET document=? WHERE id=?",[current.to_string(),id.to_string()])?;
                }
            }
            Err(error)=>{eprintln!("Cast metadata: {error}");s.metadata.record_failure(key).await;}
        }
    }
    // Series saved before their run was recorded pick it up here, one request each
    // (an empty status from earlier builds counts as missing).
    let mut lookups=0;
    for entity in entities.iter().filter(|entity| entity["provider"]=="TMDB" && entity["kind"]=="tv" && entity["series_status"].as_str().is_none_or(str::is_empty)) {
        let key=entity["id"].as_str().unwrap_or_default();
        if lookups>=3 || s.metadata.recently_failed(key).await {continue;}
        lookups+=1;
        match s.metadata.series_run(entity).await {
            Ok(run) => {
                let id=text(entity,"id")?;
                let db=s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
                if let Some(row)=db.rows("SELECT document FROM library_entities WHERE id=?",[id])?.first() {
                    let mut current:Value=serde_json::from_str(text(row,"document")?)?;
                    current["series_status"]=run["series_status"].clone();
                    current["last_air_date"]=run["last_air_date"].clone();
                    db.connection.execute("UPDATE library_entities SET document=? WHERE id=?",[current.to_string(),id.to_string()])?;
                }
            }
            Err(error)=>{eprintln!("Series run metadata: {error}");s.metadata.record_failure(key).await;}
        }
    }
    if let Ok(torrents) = crate::organizer::torrents(s).await {
        for torrent in torrents {
            let hash = text(&torrent, "hash_string")?;
            if s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?.hidden_package(hash)? { continue; }
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
        }
    }
    let assets =
        s.db.lock()
            .map_err(|_| anyhow::anyhow!("Database lock failed"))?
            .visible_assets()?;
    let config = s.effective_config()?;
    for asset in &assets {
        if (asset["available"] != true && asset["torrent_present"] != true) || asset["role"] == "extra" || asset["confirmed"] == true {
            continue;
        }
        if let Some(hash) = asset["torrent_hash"].as_str().filter(|_|!asset["metadata_query"].is_string()) {
            if let Some(record) = torrent_metadata(s, hash)?
                .filter(|r| r["selected"].is_object() && (r["confirmed"] == true
                    || (r["status"] == "matched" && r["selected"]["kind"] == "tv"
                        && matches!(r["selected"]["match_confidence"].as_str(),
                            Some("exact_title_year" | "exact_unique_title")))))
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
                    // One asset that vanished or failed must not abort matching for the rest.
                    if let Err(error) = save_match(s, text(&asset, "id")?, record["selected"].clone(), record["confirmed"] == true) {
                        eprintln!("Metadata match skipped: {error}");
                    }
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
        let mut name = asset["metadata_query"].as_str().or(asset["series_query"].as_str())
            .or(asset["series_name"].as_str())
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
        // A file's own folder often names the film plainly (`11.Moonraker.1979/`) when the
        // file name carries a franchise or site prefix. Library roots are not titles.
        let folder = path.parent().filter(|parent| {
            asset["metadata_query"].is_null() && asset["series_query"].is_null() && asset["series_name"].is_null()
                && !crate::drives::Drives::of(&config).is_root(parent)
        }).and_then(|parent| parent.file_name()).map(|folder| folder.to_string_lossy().into_owned());
        let result = s.metadata.search_in(&name, folder.as_deref()).await;
        match result {
            Ok(result) => {
                if result["selected"].is_object() {
                    if let Err(error) = save_match(s, id, result["selected"].clone(), false) {
                        eprintln!("Metadata match skipped: {error}");
                    }
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
        self.config.with_settings(&state["settings"])
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
        let entity = keep_backfilled(&tx, id, record["selected"].clone())?;
        tx.execute(
            "INSERT OR REPLACE INTO library_entities VALUES(?,?)",
            rusqlite::params![id, entity.to_string()],
        )?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod backfill_tests {
    use super::*;
    #[test]
    fn superseded_duplicates_are_removed_unless_they_carry_history_or_a_choice() -> Result<()> {
        let root = std::env::temp_dir().join(format!("torrbaby-superseded-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let mut db = crate::storage::Database::open(&root.join("state.sqlite3"),None)?;
        for (id, extra) in [("asset-stale", json!({})), ("asset-watched", json!({})), ("asset-chosen", json!({"confirmed":true})),
                            ("asset-tracked", json!({"torrent_hash":"bond","file_index":0}))] {
            let mut asset = json!({"id":id,"path":"/Downloads/Incomplete/007.Set/Moonraker.mp4","status":"review","available":false});
            asset.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            db.connection.execute("INSERT INTO library_assets VALUES(?,?)",rusqlite::params![id,asset.to_string()])?;
            db.connection.execute("INSERT INTO media_inventory VALUES(?,'r','{}',0)",[id])?;
        }
        db.connection.execute("INSERT INTO media VALUES('asset-watched:0','asset-watched',0,'Moonraker',100)",[])?;
        db.connection.execute("INSERT INTO profile_playback(profile_id,media_id,position,duration,watched) VALUES('profile-jon','asset-watched:0',50,100,0)",[])?;
        let tx = db.connection.transaction()?;
        let dropped: Vec<bool> = ["asset-stale","asset-watched","asset-chosen","asset-tracked","asset-missing"].iter()
            .map(|id| drop_superseded_asset(&tx, id)).collect::<Result<_>>()?;
        tx.commit()?;
        assert_eq!(dropped, [true, false, false, false, false]);
        let remaining: Vec<String> = db.rows("SELECT id FROM library_assets ORDER BY id",[])?.iter().map(|r|r["id"].as_str().unwrap().to_owned()).collect();
        assert_eq!(remaining, ["asset-chosen","asset-tracked","asset-watched"]);
        let caches: i64 = db.connection.query_row("SELECT COUNT(*) FROM media_inventory WHERE asset_id='asset-stale'",[],|r|r.get(0))?;
        assert_eq!(caches, 0);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn rewriting_a_stale_match_snapshot_keeps_backfilled_fields() -> Result<()> {
        let connection = rusqlite::Connection::open_in_memory()?;
        connection.execute("CREATE TABLE library_entities (id TEXT PRIMARY KEY, document TEXT NOT NULL)", [])?;
        connection.execute("INSERT INTO library_entities VALUES('tmdb:tv:1437',?)",
            [json!({"id":"tmdb:tv:1437","title":"Firefly","series_status":"Canceled","last_air_date":"2002-12-20","cast":[{"name":"Nathan Fillion"}]}).to_string()])?;
        let stale = json!({"id":"tmdb:tv:1437","title":"Firefly","year":"2002"});
        let merged = keep_backfilled(&connection, "tmdb:tv:1437", stale)?;
        assert_eq!(merged["series_status"], "Canceled");
        assert_eq!(merged["last_air_date"], "2002-12-20");
        assert_eq!(merged["cast"][0]["name"], "Nathan Fillion");
        assert_eq!(merged["year"], "2002");
        // A fresh snapshot with its own values wins.
        let fresh = json!({"id":"tmdb:tv:1437","series_status":"Ended","last_air_date":"2003-01-01","cast":[]});
        assert_eq!(keep_backfilled(&connection, "tmdb:tv:1437", fresh)?["series_status"], "Ended");
        assert_eq!(keep_backfilled(&connection, "unknown", json!({"id":"unknown"}))?["series_status"], Value::Null);
        Ok(())
    }
}
