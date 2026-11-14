//! Torrbaby's own direct connection, for a device away from home when Tailscale can only
//! relay (both ends behind carrier NAT). It grew out of the feasibility test in punch.rs.
//!
//! The device asks over its normal connection (POST /v1/direct) with its public address and
//! the ports a STUN server saw for its sockets. The server opens a UDP socket, learns its own
//! public address, and sprays the device's likely ports so China Telecom's filter lets the
//! device in, while the device's sockets fire at it. The first device packet that arrives is
//! echoed; the device then speaks QUIC from the winning socket's port, and this same socket
//! becomes a QUIC endpoint that accepts that device's address only. Each QUIC stream is piped
//! to the server's own HTTP port, so requests through it are ordinary Torrbaby requests with
//! the device's own credentials. The certificate is the server's own; its fingerprint reaches
//! the device over the normal (trusted) connection. Keepalives hold both NATs open; when the
//! device goes quiet the endpoint closes and the device falls back to Tailscale.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, net::{IpAddr, SocketAddr}, sync::{Arc, Mutex}, time::{Duration, Instant}};
use tokio::{io::{AsyncReadExt as _, AsyncWriteExt}, net::{TcpStream, UdpSocket}};

const SPRAY_SECONDS: u64 = 25;
const MAX_WINDOW: u32 = 8192;
/// Ports sprayed when the device's NAT gives each destination its own unpredictable port
/// (China Mobile): random ports across the whole range. With the device firing from ~200
/// sockets, a match is nearly certain (1 − e^(−200·2048/64512) ≈ 99.8%), and the number of
/// destinations stays near the predicted window's, so China Telecom's NAT is not flooded.
const RANDOM_PORTS: usize = 2048;

/// `count` distinct random ports in 1024–65535.
fn random_ports(count: usize) -> Vec<u16> {
    let mut chosen = std::collections::BTreeSet::new();
    while chosen.len() < count {
        for pair in uuid::Uuid::new_v4().as_bytes().chunks(2) {
            let port = u16::from_be_bytes([pair[0], pair[1]]);
            if port >= 1024 && chosen.len() < count { chosen.insert(port); }
        }
    }
    chosen.into_iter().collect()
}
const ALPN: &[u8] = b"tbdirect";

pub static DIRECT: std::sync::LazyLock<Direct> = std::sync::LazyLock::new(Direct::default);

/// Every punch's outcome, one JSON line each in the data directory, for the punch results in
/// /v1/status and Settings → Connection: how reliably each carrier punches, and how.
pub mod results {
    use serde_json::{Value, json};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    fn file(data: &Path) -> PathBuf { data.join("direct-punches.jsonl") }

    /// A carrier's name from the public IP's first two parts, where known.
    pub fn carrier(ip: &str) -> String {
        let prefix: String = ip.split('.').take(2).collect::<Vec<_>>().join(".");
        let name = match prefix.as_str() {
            "223.104" | "223.105" | "117.136" | "112.96" | "111.0" => "China Mobile",
            "183.39" | "113.84" | "14.155" | "116.30" | "59.37" => "China Telecom",
            "112.97" | "120.229" | "58.250" => "China Unicom",
            "172.56" | "172.58" | "172.59" => "T-Mobile US",
            _ => "",
        };
        if name.is_empty() { format!("{prefix}.x") } else { format!("{name} ({prefix}.x)") }
    }

    pub fn record(data: &Path, ip: &str, kind: &str, spread: &str, punched_ms: Option<u64>) {
        if !file(data).exists() { backfill(data) }
        let line = json!({"at": crate::storage::now(), "carrier": carrier(ip), "kind": kind, "spread": spread, "ok": punched_ms.is_some(), "ms": punched_ms});
        if let Ok(mut out) = std::fs::OpenOptions::new().create(true).append(true).open(file(data)) { let _ = writeln!(out, "{line}"); }
    }

    /// Per carrier and connection kind over the last 30 days: punched, failed, median time, and
    /// the failures by spray method. The first call fills the file from the server log's
    /// earlier "Direct …" lines (kind unknown) so the history starts with what is already known.
    pub fn summary(data: &Path) -> Value {
        if !file(data).exists() { backfill(data) }
        let text = std::fs::read_to_string(file(data)).unwrap_or_default();
        let since = crate::storage::now() - 30.0 * 86400.0;
        let mut groups: std::collections::BTreeMap<(String, String), (u64, u64, Vec<u64>, std::collections::BTreeMap<String, u64>)> = Default::default();
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            if v["at"].as_f64().unwrap_or(0.0) < since && v["at"].as_f64().unwrap_or(0.0) > 0.0 { continue }
            let entry = groups.entry((v["carrier"].as_str().unwrap_or("?").into(), v["kind"].as_str().unwrap_or("unknown").into())).or_default();
            if v["ok"] == true { entry.0 += 1; if let Some(ms) = v["ms"].as_u64() { entry.2.push(ms) } }
            else { entry.1 += 1; *entry.3.entry(v["spread"].as_str().unwrap_or("?").into()).or_default() += 1 }
        }
        Value::Array(groups.into_iter().map(|((carrier, kind), (ok, failed, mut ms, by))| {
            ms.sort_unstable();
            json!({"carrier": carrier, "kind": kind, "punched": ok, "failed": failed, "median_ms": ms.get(ms.len() / 2), "failed_by_spread": by})
        }).collect())
    }

    #[cfg(test)]
    #[test]
    fn carriers_are_named_and_results_are_summed() {
        let dir = std::env::temp_dir().join(format!("torrbaby-punch-results-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(carrier("223.104.80.68"), "China Mobile (223.104.x)");
        assert_eq!(carrier("8.8.8.8"), "8.8.x");
        record(&dir, "223.104.80.68", "cellular", "random", Some(70));
        record(&dir, "223.104.1.2", "cellular", "random", Some(90));
        record(&dir, "223.104.1.2", "wifi", "random", None);
        let summary = summary(&dir);
        let rows = summary.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        let cellular = rows.iter().find(|r| r["kind"] == "cellular").unwrap();
        assert_eq!((cellular["punched"].as_u64(), cellular["failed"].as_u64(), cellular["median_ms"].as_u64()), (Some(2), Some(0), Some(90)));
        let wifi = rows.iter().find(|r| r["kind"] == "wifi").unwrap();
        assert_eq!(wifi["failed_by_spread"]["random"], 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn backfill(data: &Path) {
        let Ok(log) = std::fs::read_to_string(data.join("rust-server.log")) else { return };
        let mut sessions: Vec<(String, String, String, Option<u64>)> = Vec::new();
        let mut index = std::collections::HashMap::new();
        for line in log.lines() {
            let Some(rest) = line.strip_prefix("Direct ") else { continue };
            let Some((id, text)) = rest.split_once(": ") else { continue };
            if let Some(after) = text.strip_prefix("requested by ") {
                let ip = after.split(',').next().unwrap_or("").to_owned();
                let spread = if text.contains("random ports") { "random" } else if text.contains("known ports") { "exact" } else { "window" };
                index.insert(id.to_owned(), sessions.len());
                sessions.push((id.to_owned(), ip, spread.into(), None));
            } else if let Some(after) = text.strip_prefix("punched from ") {
                let ms = after.split(" after ").nth(1).and_then(|t| t.split(" ms").next()).and_then(|t| t.parse().ok());
                if let Some(&i) = index.get(id) { sessions[i].3 = ms }
            }
        }
        let Ok(mut out) = std::fs::OpenOptions::new().create(true).append(true).open(file(data)) else { return };
        for (_, ip, spread, ms) in sessions {
            let _ = writeln!(out, "{}", json!({"at": 0, "carrier": carrier(&ip), "kind": "unknown", "spread": spread, "ok": ms.is_some(), "ms": ms}));
        }
    }
}

pub(crate) struct Identity { cert: rustls::pki_types::CertificateDer<'static>, key: Vec<u8>, pub(crate) fingerprint: String }

/// Diamond's own identity: one certificate, kept, for the direct path's QUIC and for the TLS
/// that devices wrap each relayed stream in. Devices learn its fingerprint only from pairing
/// or from Diamond on the home network, never through bronze, so bronze (which splices relay
/// streams and passes the direct path's signalling) can neither read the traffic nor stand in
/// for Diamond.
static IDENTITY: std::sync::OnceLock<Arc<Identity>> = std::sync::OnceLock::new();

/// Load Diamond's identity from `data_dir` (identity.der and identity.key), creating it once.
pub fn load_identity(data_dir: &std::path::Path) -> Result<()> {
    let (cert_path, key_path) = (data_dir.join("identity.der"), data_dir.join("identity.key"));
    let (cert, key) = match (std::fs::read(&cert_path), std::fs::read(&key_path)) {
        (Ok(cert), Ok(key)) => (cert, key),
        _ => {
            let made = rcgen::generate_simple_self_signed(vec!["torrbaby-diamond".to_owned()])?;
            let (cert, key) = (made.cert.der().to_vec(), made.key_pair.serialize_der());
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&key_path)?;
            std::io::Write::write_all(&mut file, &key)?;
            std::fs::write(&cert_path, &cert)?;
            (cert, key)
        }
    };
    let fingerprint = Sha256::digest(&cert).iter().map(|b| format!("{b:02x}")).collect();
    let _ = IDENTITY.set(Arc::new(Identity { cert: rustls::pki_types::CertificateDer::from(cert), key, fingerprint }));
    Ok(())
}

/// The fingerprint devices pin (null before the identity is loaded).
pub fn fingerprint() -> Option<String> { IDENTITY.get().map(|identity| identity.fingerprint.clone()) }

#[derive(Default)]
pub struct Direct {
    identity: Mutex<Option<Arc<Identity>>>,
    sessions: Mutex<HashMap<String, Arc<Mutex<Value>>>>,
}

impl Direct {
    fn identity(&self) -> Result<Arc<Identity>> {
        let mut held = self.identity.lock().map_err(|_| anyhow::anyhow!("Direct connection unavailable"))?;
        if let Some(identity) = held.as_ref() { return Ok(identity.clone()) }
        if let Some(identity) = IDENTITY.get() { *held = Some(identity.clone()); return Ok(identity.clone()) }
        let made = rcgen::generate_simple_self_signed(vec!["torrbaby-direct".to_owned()])?;
        let cert = made.cert.der().clone();
        let fingerprint = Sha256::digest(cert.as_ref()).iter().map(|b| format!("{b:02x}")).collect();
        let identity = Arc::new(Identity { cert, key: made.key_pair.serialize_der(), fingerprint });
        *held = Some(identity.clone());
        Ok(identity)
    }

    /// Start a direct connection for the device at `ip`, spraying ports `low..=high`.
    pub async fn start(&self, body: &Value, http_port: u16, data: &std::path::Path) -> Result<Value> {
        let ip: IpAddr = body["ip"].as_str().context("ip is required")?.parse().context("ip must be an address")?;
        let low = body["low"].as_u64().context("low is required")? as u32;
        let high = body["high"].as_u64().context("high is required")? as u32;
        ensure!((1..=65535).contains(&low) && (low..=65535).contains(&high), "ports must be 1–65535, low ≤ high");
        let high = high.min(low + MAX_WINDOW - 1);
        let random = body["spread"] == "random";
        // The device's public ports as STUN saw them. Where its NAT keeps one port per socket
        // they are exactly the ports to aim at, so they go first; "exact" sprays only them (the
        // NAT hands ports out far apart, as T-Mobile US does, so no window predicts them).
        let known: Vec<u16> = body["ports"].as_array().into_iter().flatten()
            .filter_map(|p| p.as_u64()).filter(|p| (1..=65535).contains(p)).map(|p| p as u16).take(64).collect();
        let exact = body["spread"] == "exact" && !known.is_empty();
        let mut list = known.clone();
        if !exact {
            let rest: Vec<u16> = if random { random_ports(RANDOM_PORTS) } else { (low..=high).map(|p| p as u16).collect() };
            list.extend(rest.into_iter().filter(|p| !known.contains(p)));
        }
        let ports: Arc<Vec<u16>> = Arc::new(list);
        let spread = if exact { "exact" } else if random { "random" } else { "window" };
        let identity = self.identity()?;
        let std_socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
        std_socket.set_nonblocking(true)?;
        let socket = UdpSocket::from_std(std_socket)?;
        let public = crate::punch::stun(&socket).await.context("Diamond could not learn its public address")?;
        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_owned();
        if exact { eprintln!("Direct {id}: requested by {ip}, spraying its {} known ports from {public} ({})", known.len(), body["diagnosis"].as_str().unwrap_or("")) }
        else if random { eprintln!("Direct {id}: requested by {ip}, spraying {RANDOM_PORTS} random ports from {public} ({})", body["diagnosis"].as_str().unwrap_or("")) }
        else { eprintln!("Direct {id}: requested by {ip}, spraying {low}–{high} from {public} ({})", body["diagnosis"].as_str().unwrap_or("")) }
        let state = Arc::new(Mutex::new(json!({
            "id": id, "state": "punching", "server": public.to_string(), "device_ip": ip.to_string(),
            "low": low, "high": high, "spread": spread, "device": null, "punched_after_ms": null, "streams": 0, "other_sources": [],
        })));
        {
            let mut sessions = self.sessions.lock().map_err(|_| anyhow::anyhow!("Direct connection unavailable"))?;
            if sessions.len() > 20 { sessions.retain(|_, s| s.lock().map(|s| s["state"] == "open").unwrap_or(false)); }
            sessions.insert(id.clone(), state.clone());
        }
        let fingerprint = identity.fingerprint.clone();
        let session = id.clone();
        // The outcome goes to the punch results once the punch is decided (in run, or here on failure).
        let outcome = (data.to_owned(), ip.to_string(), body["kind"].as_str().unwrap_or("unknown").to_owned(), spread.to_owned());
        tokio::spawn(async move {
            let logged = session.clone();
            if let Err(error) = run(socket, ip, ports, session, identity, http_port, state.clone(), outcome.clone()).await {
                eprintln!("Direct {logged}: {error}");
                let punched = state.lock().map(|s| s["punched_after_ms"].is_u64()).unwrap_or(true);
                if !punched { results::record(&outcome.0, &outcome.1, &outcome.2, &outcome.3, None) }
                if let Ok(mut s) = state.lock() { s["state"] = json!("failed"); s["error"] = json!(error.to_string()); }
            }
        });
        Ok(json!({"id": id, "server": public.to_string(), "fingerprint": fingerprint, "low": low, "high": high, "spread": spread}))
    }

    pub fn status(&self, id: &str) -> Result<Value> {
        let sessions = self.sessions.lock().map_err(|_| anyhow::anyhow!("Direct connection unavailable"))?;
        Ok(sessions.get(id).context("No such direct connection")?.lock().map_err(|_| anyhow::anyhow!("Direct connection unavailable"))?.clone())
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(socket: UdpSocket, ip: IpAddr, ports: Arc<Vec<u16>>, id: String, identity: Arc<Identity>, http_port: u16, state: Arc<Mutex<Value>>, outcome: (std::path::PathBuf, String, String, String)) -> Result<()> {
    // Punch: open China Telecom's filter for the device's likely ports until one gets through.
    let started = Instant::now();
    let spray = format!("TBDIRECT1 {id} spray").into_bytes();
    let prefix = format!("TBDIRECT1 {id} ");
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut buffer = [0u8; 1500];
    let device = loop {
        ensure!(started.elapsed() < Duration::from_secs(SPRAY_SECONDS), "No packet from the device got through");
        tokio::select! {
            _ = tick.tick() => {
                for &port in ports.iter() { let _ = socket.send_to(&spray, SocketAddr::new(ip, port)).await; }
            }
            received = socket.recv_from(&mut buffer) => {
                let Ok((len, from)) = received else { continue };
                let Ok(text) = std::str::from_utf8(&buffer[..len]) else { continue };
                if !text.starts_with(&prefix) { continue }
                // The device's packet from an address other than the one STUN saw: its NAT uses
                // a different public address for each destination. Logged, and accepted.
                if from.ip() != ip {
                    eprintln!("Direct {id}: punch packet from another address {from} (STUN saw {ip})");
                    if let Ok(mut s) = state.lock() { if let Some(list) = s["other_sources"].as_array_mut() { list.push(json!(from.to_string())) } }
                }
                // Echo a few times: the device switches this socket's port to QUIC on the first.
                for _ in 0..3 { let _ = socket.send_to(format!("TBACK {text}").as_bytes(), from).await; }
                break from;
            }
        }
    };
    // Watch for the device's QUIC (anything that is not a punch packet) and log where it comes
    // from; QUIC resends its first packet, so handing over then loses nothing.
    // Until the device's QUIC shows up (up to 12 s), keep spraying and echoing its punch
    // packets: it starts QUIC only once it has seen an echo.
    let watch_until = Instant::now() + Duration::from_secs(12);
    while Instant::now() < watch_until {
        tokio::select! {
            _ = tick.tick() => {
                for &port in ports.iter() { let _ = socket.send_to(&spray, SocketAddr::new(ip, port)).await; }
            }
            received = tokio::time::timeout(watch_until.saturating_duration_since(Instant::now()), socket.recv_from(&mut buffer)) => {
                let Ok(Ok((len, from))) = received else { break };
                // Keep echoing the device's punch packets, as the test does: three echoes at
                // the first one were lost on a hotspot and the device never saw it get through.
                if buffer[..len].starts_with(prefix.as_bytes()) {
                    let echo = [b"TBACK ".as_slice(), &buffer[..len]].concat();
                    let _ = socket.send_to(&echo, from).await;
                    continue;
                }
                if buffer[..len].starts_with(b"TBDIRECT1") { continue }
                eprintln!("Direct {id}: first non-test packet from {from} ({len} bytes{}) after {} ms",
                    if from.ip() == ip { "" } else { ", another address" }, started.elapsed().as_millis());
                if let Ok(mut s) = state.lock() { s["first_quic_from"] = json!(from.to_string()) }
                break;
            }
        }
    }
    if let Ok(mut s) = state.lock() {
        s["state"] = json!("punched");
        s["device"] = json!(device.to_string());
        results::record(&outcome.0, &outcome.1, &outcome.2, &outcome.3, Some(started.elapsed().as_millis() as u64));
        s["punched_after_ms"] = json!(started.elapsed().as_millis() as u64);
    }
    // The same socket, and so the same hole, becomes a QUIC endpoint for that device only.
    let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(vec![identity.cert.clone()], rustls::pki_types::PrivateKeyDer::Pkcs8(identity.key.clone().into()))?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let mut server = quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls)?));
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(10)));
    transport.max_idle_timeout(Some(Duration::from_secs(45).try_into()?));
    transport.max_concurrent_bidi_streams(256u32.into());
    // BBR, not Cubic: Cubic fills a bottleneck's queue until packets are lost (on CT 5G the
    // round trip climbed 100 ms → 3 s with almost no loss while video streamed, and the player
    // stalled); BBR paces to the measured bottleneck and keeps the queue short.
    transport.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
    server.transport_config(Arc::new(transport));
    let std_socket = socket.into_std()?;
    // Keep spraying until QUIC connects: iOS may start QUIC from a fresh port rather than the
    // punched one, and the device's NAT gives each new destination port the next port along,
    // so the window must stay open for it. A duplicate handle sends while QUIC owns the socket.
    let sprayer = UdpSocket::from_std(std_socket.try_clone()?)?;
    let connected = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let spraying = connected.clone();
    let spray_until = Instant::now() + Duration::from_secs(20);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        while !spraying.load(std::sync::atomic::Ordering::Relaxed) && Instant::now() < spray_until {
            tick.tick().await;
            for &port in ports.iter() { let _ = sprayer.send_to(&spray, SocketAddr::new(ip, port)).await; }
        }
    });
    let endpoint = quinn::Endpoint::new(quinn::EndpointConfig::default(), Some(server), std_socket, Arc::new(quinn::TokioRuntime))?;
    if let Ok(mut s) = state.lock() { s["state"] = json!("waiting") }
    eprintln!("Direct {id}: punched from {device} after {} ms; waiting for QUIC", started.elapsed().as_millis());
    // One device: the first connection from its address; then serve until it goes quiet.
    let connection = loop {
        let Ok(accepted) = tokio::time::timeout(Duration::from_secs(20), endpoint.accept()).await else {
            connected.store(true, std::sync::atomic::Ordering::Relaxed);
            eprintln!("Direct {id}: no QUIC from the device in 20 s");
            anyhow::bail!("The device did not start QUIC");
        };
        let incoming = accepted.context("Endpoint closed")?;
        let from = incoming.remote_address();
        if from.ip() != device.ip() { incoming.refuse(); continue }
        match incoming.await {
            Ok(connection) => { eprintln!("Direct {id}: QUIC open from {from}"); break connection }
            Err(error) => { eprintln!("Direct {id}: QUIC from {from} failed: {error}"); continue }
        }
    };
    connected.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Ok(mut s) = state.lock() { s["state"] = json!("open"); s["opened_after_ms"] = json!(started.elapsed().as_millis() as u64); }
    while let Ok((send, recv)) = connection.accept_bi().await {
        if let Ok(mut s) = state.lock() { s["streams"] = json!(s["streams"].as_u64().unwrap_or(0) + 1) }
        tokio::spawn(pipe(send, recv, http_port, format!("Direct {id}")));
    }
    endpoint.close(0u32.into(), b"done");
    eprintln!("Direct {id}: closed after {} s ({})", started.elapsed().as_secs(), connection.close_reason().map(|r| r.to_string()).unwrap_or_default());
    if let Ok(mut s) = state.lock() { s["state"] = json!("closed"); s["closed_after_ms"] = json!(started.elapsed().as_millis() as u64); }
    Ok(())
}

/// ALPN of the TLS a device wraps a relayed stream in (end to end, to Diamond's identity).
pub(crate) const E2E_ALPN: &[u8] = b"tbe2e";

/// One stream is one connection to the server's own HTTP port, both ways. A stream that opens
/// with a TLS handshake is a device's end-to-end encryption through bronze: it is decrypted
/// here with Diamond's identity. Others (older clients, and the direct path, already QUIC to
/// this identity) are plain. The first request line (method and path, never the query, which
/// can carry signatures) goes to the log.
pub(crate) async fn pipe(mut send: quinn::SendStream, mut recv: quinn::RecvStream, http_port: u16, label: String) {
    let mut first = vec![0u8; 2048];
    let n = match recv.read(&mut first).await { Ok(Some(n)) => n, _ => 0 };
    first.truncate(n);
    // 0x16: a TLS handshake record. No HTTP request starts with it.
    if first.first() == Some(&0x16) {
        let Some(identity) = IDENTITY.get().cloned() else { let _ = send.reset(1u32.into()); return };
        let stream = tokio::io::join(std::io::Cursor::new(first).chain(recv), send);
        let Ok(acceptor) = acceptor(&identity) else { return };
        let Ok(Ok(mut tls)) = tokio::time::timeout(Duration::from_secs(20), acceptor.accept(stream)).await else { return };
        let mut first = vec![0u8; 2048];
        let n = tls.read(&mut first).await.unwrap_or(0);
        first.truncate(n);
        serve(tls, first, http_port, label + " (end to end)").await;
    } else {
        serve(tokio::io::join(recv, send), first, http_port, label).await;
    }
}

fn acceptor(identity: &Identity) -> Result<tokio_rustls::TlsAcceptor> {
    let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(vec![identity.cert.clone()], rustls::pki_types::PrivateKeyDer::Pkcs8(identity.key.clone().into()))?;
    tls.alpn_protocols = vec![E2E_ALPN.to_vec()];
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(tls)))
}

/// Pipe `stream` (whose first bytes, already read, are `first`) to the HTTP port and back.
async fn serve<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(stream: S, first: Vec<u8>, http_port: u16, label: String) {
    let (mut from_device, mut to_device) = tokio::io::split(stream);
    let Ok(tcp) = TcpStream::connect(("127.0.0.1", http_port)).await else { return };
    let _ = tcp.set_nodelay(true);
    let started = Instant::now();
    let (mut from_server, mut to_server) = tcp.into_split();
    let line = String::from_utf8_lossy(&first).lines().next().unwrap_or("").split('?').next().unwrap_or("").to_owned();
    if !first.is_empty() && to_server.write_all(&first).await.is_err() { return }
    let up = async { let sent = tokio::io::copy(&mut from_device, &mut to_server).await.unwrap_or(0); let _ = to_server.shutdown().await; sent };
    let down = async { let got = tokio::io::copy(&mut from_server, &mut to_device).await.unwrap_or(0); let _ = to_device.shutdown().await; got };
    let (_, down_bytes) = tokio::join!(up, down);
    eprintln!("{label}: stream {line} → {down_bytes} bytes in {} ms", started.elapsed().as_millis());
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A device punches, then speaks QUIC from the same port, and a stream reaches the
    /// server's HTTP port and back.
    #[tokio::test]
    async fn a_punched_socket_carries_streams_to_the_http_port() -> Result<()> {
        // A stand-in for the server's HTTP port.
        let http = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let http_port = http.local_addr()?.port();
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = http.accept().await {
                tokio::spawn(async move {
                    let mut request = [0u8; 256];
                    let n = tcp.read(&mut request).await.unwrap_or(0);
                    let _ = tcp.write_all(format!("HTTP/1.1 200 OK\r\n\r\nheard {}", String::from_utf8_lossy(&request[..n]).lines().next().unwrap_or("")).as_bytes()).await;
                });
            }
        });
        let direct = Direct::default();
        let identity = direct.identity()?;
        let server = UdpSocket::bind("127.0.0.1:0").await?;
        let server_address = server.local_addr()?;
        // The device's socket; the server sprays a window that holds its port.
        let device = UdpSocket::bind("127.0.0.1:0").await?;
        let device_port = device.local_addr()?.port() as u32;
        let state = Arc::new(Mutex::new(json!({})));
        let run_state = state.clone();
        let run_identity = identity.clone();
        tokio::spawn(async move {
            run(server, "127.0.0.1".parse().unwrap(), Arc::new((device_port.saturating_sub(5).max(1)..=device_port + 5).map(|p| p as u16).collect()), "t".into(), run_identity, http_port, run_state, (std::env::temp_dir(), "127.0.0.1".into(), "test".into(), "window".into())).await
        });
        device.send_to(b"TBDIRECT1 t 0 hello", server_address).await?;
        let mut buffer = [0u8; 256];
        loop {
            let (n, _) = tokio::time::timeout(Duration::from_secs(5), device.recv_from(&mut buffer)).await??;
            if buffer[..n].starts_with(b"TBACK") { break }
        }
        // QUIC from the same port, trusting the fingerprinted certificate.
        let device = device.into_std()?;
        let mut roots = rustls::RootCertStore::empty();
        roots.add(identity.cert.clone())?;
        let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])?.with_root_certificates(roots).with_no_client_auth();
        tls.alpn_protocols = vec![ALPN.to_vec()];
        let mut client = quinn::Endpoint::new(quinn::EndpointConfig::default(), None, device, Arc::new(quinn::TokioRuntime))?;
        client.set_default_client_config(quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls)?)));
        let connection = tokio::time::timeout(Duration::from_secs(5), client.connect(server_address, "torrbaby-direct")?.into_future()).await??;
        let (mut send, mut recv) = connection.open_bi().await?;
        send.write_all(b"GET /v1/ping HTTP/1.1\r\n\r\n").await?;
        send.finish()?;
        let reply = tokio::time::timeout(Duration::from_secs(5), recv.read_to_end(4096)).await??;
        assert_eq!(String::from_utf8_lossy(&reply), "HTTP/1.1 200 OK\r\n\r\nheard GET /v1/ping HTTP/1.1");
        assert_eq!(state.lock().unwrap()["state"], "open");
        Ok(())
    }

    /// Through bronze: a device's stream wrapped in TLS to Diamond's identity reaches the HTTP
    /// port decrypted, and a plain stream (an older client) still works beside it.
    #[tokio::test]
    async fn a_relayed_stream_is_decrypted_end_to_end_and_plain_ones_still_work() -> Result<()> {
        let http = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let http_port = http.local_addr()?.port();
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = http.accept().await {
                tokio::spawn(async move {
                    let mut request = [0u8; 256];
                    let n = tcp.read(&mut request).await.unwrap_or(0);
                    let _ = tcp.write_all(format!("HTTP/1.1 200 OK\r\n\r\nheard {}", String::from_utf8_lossy(&request[..n]).lines().next().unwrap_or("")).as_bytes()).await;
                });
            }
        });
        let dir = std::env::temp_dir().join(format!("tb-identity-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        load_identity(&dir)?;
        let pinned = fingerprint().unwrap();
        // bronze's leg, as a QUIC connection whose streams are piped here.
        let made = rcgen::generate_simple_self_signed(vec!["bronze".into()])?;
        let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])?.with_no_client_auth()
            .with_single_cert(vec![made.cert.der().clone()], rustls::pki_types::PrivateKeyDer::Pkcs8(made.key_pair.serialize_der().into()))?;
        tls.alpn_protocols = vec![b"t".to_vec()];
        let server = quinn::Endpoint::server(quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls)?)), "127.0.0.1:0".parse()?)?;
        let address = server.local_addr()?;
        tokio::spawn(async move {
            let connection = server.accept().await.unwrap().await.unwrap();
            while let Ok((send, recv)) = connection.accept_bi().await { tokio::spawn(pipe(send, recv, http_port, "Relay".into())); }
        });
        let mut roots = rustls::RootCertStore::empty();
        roots.add(made.cert.der().clone())?;
        let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])?.with_root_certificates(roots).with_no_client_auth();
        tls.alpn_protocols = vec![b"t".to_vec()];
        let mut client = quinn::Endpoint::client("127.0.0.1:0".parse()?)?;
        client.set_default_client_config(quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls)?)));
        let connection = client.connect(address, "bronze")?.await?;

        // End to end: TLS inside the stream, Diamond's certificate checked by its fingerprint.
        let (send, recv) = connection.open_bi().await?;
        let mut e2e = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .dangerous().with_custom_certificate_verifier(Arc::new(FingerprintOnly(pinned, Arc::new(rustls::crypto::ring::default_provider())))).with_no_client_auth();
        e2e.alpn_protocols = vec![E2E_ALPN.to_vec()];
        let connector = tokio_rustls::TlsConnector::from(Arc::new(e2e));
        let mut stream = connector.connect(rustls::pki_types::ServerName::try_from("torrbaby-diamond")?, tokio::io::join(recv, send)).await?;
        stream.write_all(b"GET /v1/ping HTTP/1.1\r\n\r\n").await?;
        let mut reply = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut reply)).await??;
        assert_eq!(String::from_utf8_lossy(&reply), "HTTP/1.1 200 OK\r\n\r\nheard GET /v1/ping HTTP/1.1");

        // Plain, as before.
        let (mut send, mut recv) = connection.open_bi().await?;
        send.write_all(b"GET /v1/status HTTP/1.1\r\n\r\n").await?;
        send.finish()?;
        let reply = tokio::time::timeout(Duration::from_secs(5), recv.read_to_end(4096)).await??;
        assert_eq!(String::from_utf8_lossy(&reply), "HTTP/1.1 200 OK\r\n\r\nheard GET /v1/status HTTP/1.1");
        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[derive(Debug)]
    struct FingerprintOnly(String, Arc<rustls::crypto::CryptoProvider>);
    impl rustls::client::danger::ServerCertVerifier for FingerprintOnly {
        fn verify_server_cert(&self, cert: &rustls::pki_types::CertificateDer<'_>, _: &[rustls::pki_types::CertificateDer<'_>], _: &rustls::pki_types::ServerName<'_>, _: &[u8], _: rustls::pki_types::UnixTime) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            let digest: String = Sha256::digest(cert.as_ref()).iter().map(|b| format!("{b:02x}")).collect();
            if digest == self.0 { Ok(rustls::client::danger::ServerCertVerified::assertion()) } else { Err(rustls::Error::General("not Diamond".into())) }
        }
        fn verify_tls12_signature(&self, m: &[u8], c: &rustls::pki_types::CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> { rustls::crypto::verify_tls12_signature(m, c, d, &self.1.signature_verification_algorithms) }
        fn verify_tls13_signature(&self, m: &[u8], c: &rustls::pki_types::CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> { rustls::crypto::verify_tls13_signature(m, c, d, &self.1.signature_verification_algorithms) }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> { self.1.signature_verification_algorithms.supported_schemes() }
    }
}
