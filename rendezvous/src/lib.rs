//! Torrbaby's rendezvous, running on bronze (a cloud server with a fixed public address in
//! Shenzhen). Diamond sits behind China Telecom's carrier NAT and nothing can reach it from
//! outside, so it keeps one standing outbound connection here. Devices away from home reach
//! Diamond through this service when they cannot punch a direct path; once they can talk to
//! Diamond they set up the direct tunnel (server/direct.rs) and move their traffic onto it.
//!
//! Three jobs, on two UDP ports:
//! - STUN (3478): Binding responses, so devices and Diamond learn their public address and
//!   port for punching.
//! - QUIC (7443), ALPN `tbhome`: Diamond's standing connection. Its first stream proves the
//!   shared secret; after that this side opens one stream per relayed device stream, and
//!   Diamond pipes each to its own HTTP port.
//! - QUIC (7443), ALPN `tbrelay`: a device's connection. Each stream it opens is spliced to a
//!   new stream on Diamond's connection. Requests stay ordinary Torrbaby requests carrying the
//!   device's own credentials, which Diamond checks; this side holds no client credentials.
//!   A stream that starts with `TBINFO1\n` is answered here instead: whether Diamond is
//!   connected, the device's address as seen from here, and Diamond's public IP (a device
//!   with the same public IP is behind the same NAT: at home). One that is `TBNAME1 <client
//!   id>\n` names the connection's device (its Torrbaby client id; Diamond knows the names).
//!
//! Usage, for Diamond's Connection page: the devices connected now (client id, address, since,
//! bytes relayed) and bytes relayed per day (China time), kept in usage.json in the state
//! directory. Diamond asks over its own connection with a `TBUSAGE1\n` stream.
//!
//! The certificate is self-signed, made once and kept in the state directory; devices pin its
//! SHA-256 fingerprint, which reaches them through pairing or Diamond's status.
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::watch;

pub const ALPN_HOME: &[u8] = b"tbhome";
pub const ALPN_RELAY: &[u8] = b"tbrelay";
pub const HOME_HELLO: &str = "TBHOME1 ";
pub const INFO: &[u8] = b"TBINFO1\n";
pub const NAME: &[u8] = b"TBNAME1 ";
pub const USAGE: &[u8] = b"TBUSAGE1\n";
/// Stream reset code when Diamond is not connected.
pub const HOME_OFFLINE: u32 = 2;
const MAGIC: u32 = 0x2112A442;
/// Devices connected at once; each is a phone, tablet or TV away from home.
const MAX_DEVICES: usize = 256;

pub mod stun {
    use super::*;

    /// The Binding success response to a STUN Binding request from `from`, or None when the
    /// datagram is not one (anything else is ignored, so this port reflects nothing else).
    pub fn answer(request: &[u8], from: SocketAddr) -> Option<Vec<u8>> {
        if request.len() < 20 || request[0] & 0xC0 != 0 { return None }
        if u16::from_be_bytes([request[0], request[1]]) != 0x0001 { return None }
        if u32::from_be_bytes(request[4..8].try_into().ok()?) != MAGIC { return None }
        let declared = u16::from_be_bytes([request[2], request[3]]) as usize;
        if declared % 4 != 0 || 20 + declared != request.len() { return None }
        let txn = &request[8..20];
        let port = from.port();
        let mut xor = Vec::new();
        let mut plain = Vec::new();
        match from.ip() {
            IpAddr::V4(ip) => {
                let ip = u32::from(ip);
                xor.extend_from_slice(&[0, 1]);
                xor.extend_from_slice(&(port ^ (MAGIC >> 16) as u16).to_be_bytes());
                xor.extend_from_slice(&(ip ^ MAGIC).to_be_bytes());
                plain.extend_from_slice(&[0, 1]);
                plain.extend_from_slice(&port.to_be_bytes());
                plain.extend_from_slice(&ip.to_be_bytes());
            }
            IpAddr::V6(ip) => {
                let mut key = [0u8; 16];
                key[..4].copy_from_slice(&MAGIC.to_be_bytes());
                key[4..].copy_from_slice(txn);
                xor.extend_from_slice(&[0, 2]);
                xor.extend_from_slice(&(port ^ (MAGIC >> 16) as u16).to_be_bytes());
                xor.extend(ip.octets().iter().zip(key).map(|(a, b)| a ^ b));
                plain.extend_from_slice(&[0, 2]);
                plain.extend_from_slice(&port.to_be_bytes());
                plain.extend_from_slice(&ip.octets());
            }
        }
        let mut body = Vec::new();
        for (kind, value) in [(0x0020u16, &xor), (0x0001u16, &plain)] {
            body.extend_from_slice(&kind.to_be_bytes());
            body.extend_from_slice(&(value.len() as u16).to_be_bytes());
            body.extend_from_slice(value);
        }
        let mut response = Vec::with_capacity(20 + body.len());
        response.extend_from_slice(&0x0101u16.to_be_bytes());
        response.extend_from_slice(&(body.len() as u16).to_be_bytes());
        response.extend_from_slice(&MAGIC.to_be_bytes());
        response.extend_from_slice(txn);
        response.extend_from_slice(&body);
        Some(response)
    }

    /// Answer Binding requests on `socket` forever.
    pub async fn serve(socket: UdpSocket) {
        let mut buffer = [0u8; 1500];
        loop {
            let Ok((len, from)) = socket.recv_from(&mut buffer).await else { continue };
            if let Some(response) = answer(&buffer[..len], from) { let _ = socket.send_to(&response, from).await; }
        }
    }
}

pub struct Identity {
    pub cert: rustls::pki_types::CertificateDer<'static>,
    pub key: Vec<u8>,
    pub fingerprint: String,
}

pub fn fingerprint(cert: &[u8]) -> String {
    Sha256::digest(cert).iter().map(|b| format!("{b:02x}")).collect()
}

impl Identity {
    /// The certificate in `dir` (cert.der, key.der), made on first use. It never changes on
    /// its own: devices pin its fingerprint.
    pub fn load_or_create(dir: &Path) -> Result<Identity> {
        let (cert_path, key_path) = (dir.join("cert.der"), dir.join("key.der"));
        // The certificate is written last, so a key without one is a half-finished first run.
        if !cert_path.exists() {
            let _ = std::fs::remove_file(&key_path);
            let made = rcgen::generate_simple_self_signed(vec!["torrbaby-rendezvous".to_owned()])?;
            write_private(&key_path, &made.key_pair.serialize_der())?;
            std::fs::write(&cert_path, made.cert.der())?;
        }
        let cert = std::fs::read(&cert_path).with_context(|| format!("Reading {}", cert_path.display()))?;
        let key = std::fs::read(&key_path).with_context(|| format!("Reading {}", key_path.display()))?;
        Ok(Identity { fingerprint: fingerprint(&cert), cert: cert.into(), key })
    }
}

/// Diamond's shared secret in `dir` (home.secret), made on first use: 32 random bytes as hex.
pub fn load_or_create_secret(dir: &Path) -> Result<String> {
    let path = dir.join("home.secret");
    if !path.exists() {
        let mut bytes = [0u8; 32];
        std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom")?, &mut bytes)?;
        write_private(&path, bytes.iter().map(|b| format!("{b:02x}")).collect::<String>().as_bytes())?;
    }
    let secret = std::fs::read_to_string(&path)?.trim().to_owned();
    if secret.len() < 32 { bail!("{} is too short", path.display()) }
    Ok(secret)
}

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
        .with_context(|| format!("Creating {}", path.display()))?;
    std::io::Write::write_all(&mut file, data)?;
    Ok(())
}

fn transport() -> quinn::TransportConfig {
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(10)));
    transport.max_idle_timeout(Some(Duration::from_secs(45).try_into().expect("45 s fits")));
    transport.max_concurrent_bidi_streams(1024u32.into());
    transport.max_concurrent_uni_streams(0u32.into());
    // Room for a 4K stream across a longer device round trip.
    transport.stream_receive_window((8u32 << 20).into());
    transport.receive_window((32u32 << 20).into());
    transport.send_window(32 << 20);
    // BBR, not Cubic: Cubic fills a bottleneck's queue until packets are lost (on CT 5G the
    // round trip climbed 100 ms → 3 s with almost no loss while video streamed, and the player
    // stalled); BBR paces to the measured bottleneck and keeps the queue short.
    transport.congestion_controller_factory(std::sync::Arc::new(quinn::congestion::BbrConfig::default()));

    transport
}

pub fn endpoint(socket: std::net::UdpSocket, identity: &Identity) -> Result<quinn::Endpoint> {
    let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(vec![identity.cert.clone()], rustls::pki_types::PrivateKeyDer::Pkcs8(identity.key.clone().into()))?;
    tls.alpn_protocols = vec![ALPN_HOME.to_vec(), ALPN_RELAY.to_vec()];
    let mut server = quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls)?));
    server.transport_config(Arc::new(transport()));
    socket.set_nonblocking(true)?;
    Ok(quinn::Endpoint::new(quinn::EndpointConfig::default(), Some(server), socket, Arc::new(quinn::TokioRuntime))?)
}

/// Diamond's current connection, shared with every device connection.
pub struct Hub {
    secret: [u8; 32],
    home: watch::Sender<Option<quinn::Connection>>,
    devices: AtomicU64,
    streams: AtomicU64,
    /// The devices connected now, by connection.
    connected: std::sync::Mutex<std::collections::BTreeMap<usize, Arc<Device>>>,
    /// Bytes relayed since start (down: Diamond to devices; up: devices to Diamond), and the
    /// per-day totals they are folded into each minute.
    down: AtomicU64,
    up: AtomicU64,
    days: std::sync::Mutex<Days>,
}

/// One connected device.
struct Device {
    client: std::sync::Mutex<String>,
    address: std::sync::Mutex<SocketAddr>,
    since: u64,
    down: AtomicU64,
    up: AtomicU64,
    streams: AtomicU64,
}

/// Bytes relayed per day ("2026-11-14", China time): (down, up). Folded from the running
/// counters; `folded` is how much of them is already counted.
#[derive(Default)]
struct Days { days: std::collections::BTreeMap<String, (u64, u64)>, folded: (u64, u64), file: Option<std::path::PathBuf> }

fn unix_now() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

/// The date in China (UTC+8, no daylight saving) at `unix` seconds.
pub fn china_day(unix: u64) -> String {
    let days = (unix + 8 * 3600) / 86400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

impl Hub {
    pub fn new(secret: &str) -> Arc<Hub> {
        Arc::new(Hub { secret: Sha256::digest(secret.as_bytes()).into(), home: watch::channel(None).0, devices: AtomicU64::new(0), streams: AtomicU64::new(0),
            connected: Default::default(), down: AtomicU64::new(0), up: AtomicU64::new(0), days: Default::default() })
    }

    /// Keep the per-day totals in `dir`/usage.json: read now, written each minute while
    /// traffic flows (the last 400 days are kept).
    pub fn keep_usage(self: &Arc<Self>, dir: &Path) {
        let file = dir.join("usage.json");
        if let Ok(mut days) = self.days.lock() {
            if let Some(saved) = std::fs::read(&file).ok().and_then(|d| serde_json::from_slice::<serde_json::Value>(&d).ok()) {
                for (day, totals) in saved["days"].as_object().into_iter().flatten() {
                    days.days.insert(day.clone(), (totals["down"].as_u64().unwrap_or(0), totals["up"].as_u64().unwrap_or(0)));
                }
            }
            days.file = Some(file);
        }
        let hub = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                hub.fold(true);
            }
        });
    }

    /// Write the totals now (on shutdown).
    pub fn save_usage(&self) { self.fold(true) }

    /// Fold the running counters into today's total; written to the file when asked and changed.
    fn fold(&self, write: bool) {
        let (down, up) = (self.down.load(Ordering::Relaxed), self.up.load(Ordering::Relaxed));
        let Ok(mut days) = self.days.lock() else { return };
        let (delta_down, delta_up) = (down - days.folded.0, up - days.folded.1);
        if delta_down == 0 && delta_up == 0 { return }
        days.folded = (down, up);
        let today = days.days.entry(china_day(unix_now())).or_default();
        today.0 += delta_down; today.1 += delta_up;
        while days.days.len() > 400 { let first = days.days.keys().next().cloned().unwrap_or_default(); days.days.remove(&first); }
        if let (true, Some(file)) = (write, days.file.clone()) {
            let json = serde_json::json!({"days": days.days.iter().map(|(day, (down, up))| (day.clone(), serde_json::json!({"down": down, "up": up}))).collect::<serde_json::Map<_, _>>()});
            let temporary = file.with_extension("json.tmp");
            if std::fs::write(&temporary, json.to_string()).is_ok() { let _ = std::fs::rename(&temporary, &file); }
        }
    }

    /// What Diamond's Connection page shows: the devices connected now and bytes per day.
    pub fn usage(&self) -> serde_json::Value {
        self.fold(false);
        let devices: Vec<_> = self.connected.lock().map(|connected| connected.values().map(|device| serde_json::json!({
            "client": *device.client.lock().unwrap_or_else(|e| e.into_inner()),
            "address": device.address.lock().map(|a| a.to_string()).unwrap_or_default(),
            "since": device.since, "down": device.down.load(Ordering::Relaxed), "up": device.up.load(Ordering::Relaxed),
            "streams": device.streams.load(Ordering::Relaxed),
        })).collect()).unwrap_or_default();
        let days: serde_json::Map<_, _> = self.days.lock().map(|days| days.days.iter().map(|(day, (down, up))| (day.clone(), serde_json::json!({"down": down, "up": up}))).collect()).unwrap_or_default();
        serde_json::json!({"devices": devices, "days": days, "today": china_day(unix_now()), "home_online": self.home_online()})
    }

    /// Diamond's connection, waiting up to `wait` for one (it reconnects within seconds).
    async fn home(&self, wait: Duration) -> Option<quinn::Connection> {
        let mut seen = self.home.subscribe();
        let live = |c: &Option<quinn::Connection>| c.as_ref().is_some_and(|c| c.close_reason().is_none());
        let found = tokio::time::timeout(wait, seen.wait_for(live)).await.ok()?.ok()?.clone();
        found
    }

    fn home_online(&self) -> bool {
        self.home.borrow().as_ref().is_some_and(|c| c.close_reason().is_none())
    }

    /// Diamond's public IP, as its standing connection comes from.
    fn home_ip(&self) -> Option<std::net::IpAddr> {
        self.home.borrow().as_ref().filter(|c| c.close_reason().is_none()).map(|c| c.remote_address().ip())
    }
}

/// Accept connections on `endpoint` until it closes.
pub async fn serve(endpoint: quinn::Endpoint, hub: Arc<Hub>) {
    while let Some(incoming) = endpoint.accept().await {
        if hub.devices.load(Ordering::Relaxed) as usize >= MAX_DEVICES { incoming.refuse(); continue }
        let hub = hub.clone();
        tokio::spawn(async move {
            let from = incoming.remote_address();
            let connection = match incoming.await {
                Ok(connection) => connection,
                Err(error) => { eprintln!("{from}: handshake failed: {error}"); return }
            };
            let alpn = connection.handshake_data()
                .and_then(|d| d.downcast::<quinn::crypto::rustls::HandshakeData>().ok())
                .and_then(|d| d.protocol);
            match alpn.as_deref() {
                Some(ALPN_HOME) => home(connection, hub).await,
                Some(ALPN_RELAY) => device(connection, hub).await,
                _ => connection.close(1u32.into(), b"protocol"),
            }
        });
    }
}

async fn home(connection: quinn::Connection, hub: Arc<Hub>) {
    let from = connection.remote_address();
    let proved = async {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let mut line = Vec::new();
        while !line.ends_with(b"\n") && line.len() < 256 {
            let mut byte = [0u8; 1];
            recv.read_exact(&mut byte).await?;
            line.push(byte[0]);
        }
        let text = String::from_utf8_lossy(&line);
        let offered = text.trim_end().strip_prefix(HOME_HELLO).unwrap_or("");
        let digest: [u8; 32] = Sha256::digest(offered.as_bytes()).into();
        // Compare digests in full rather than stopping at the first difference.
        if digest.iter().zip(hub.secret).fold(0u8, |acc, (a, b)| acc | (a ^ b)) != 0 { bail!("wrong secret") }
        send.write_all(b"OK\n").await?;
        send.finish()?;
        anyhow::Ok(())
    };
    match tokio::time::timeout(Duration::from_secs(10), proved).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => { eprintln!("Home {from}: refused: {error}"); connection.close(3u32.into(), b"refused"); return }
        Err(_) => { eprintln!("Home {from}: refused: no hello"); connection.close(3u32.into(), b"refused"); return }
    }
    let previous = hub.home.send_replace(Some(connection.clone()));
    if let Some(previous) = previous { previous.close(4u32.into(), b"replaced") }
    eprintln!("Home {from}: connected");
    // Diamond's own questions on its connection (it is proved): usage, for its Connection page.
    let asking = connection.clone();
    let asked = hub.clone();
    tokio::spawn(async move {
        while let Ok((mut send, mut recv)) = asking.accept_bi().await {
            let hub = asked.clone();
            tokio::spawn(async move {
                let Ok(request) = recv.read_to_end(64).await else { return };
                if request == USAGE { let _ = send.write_all(hub.usage().to_string().as_bytes()).await; }
                let _ = send.finish();
            });
        }
    });
    let started = Instant::now();
    let reason = connection.closed().await;
    hub.home.send_if_modified(|held| {
        if held.as_ref().is_some_and(|c| c.stable_id() == connection.stable_id()) { *held = None; true } else { false }
    });
    eprintln!("Home {from}: closed after {} s ({reason})", started.elapsed().as_secs());
}

async fn device(connection: quinn::Connection, hub: Arc<Hub>) {
    let from = connection.remote_address();
    let id = connection.stable_id();
    hub.devices.fetch_add(1, Ordering::Relaxed);
    eprintln!("Device {id} {from}: connected");
    let record = Arc::new(Device { client: Default::default(), address: std::sync::Mutex::new(from), since: unix_now(), down: AtomicU64::new(0), up: AtomicU64::new(0), streams: AtomicU64::new(0) });
    if let Ok(mut connected) = hub.connected.lock() { connected.insert(id, record.clone()); }
    let started = Instant::now();
    let mut streams = 0u64;
    while let Ok((send, recv)) = connection.accept_bi().await {
        streams += 1;
        hub.streams.fetch_add(1, Ordering::Relaxed);
        record.streams.fetch_add(1, Ordering::Relaxed);
        // The address now, not at connect: a device that moved networks migrated its connection.
        let address = connection.remote_address();
        if let Ok(mut held) = record.address.lock() { *held = address }
        tokio::spawn(splice(send, recv, hub.clone(), address, id, record.clone()));
    }
    if let Ok(mut connected) = hub.connected.lock() { connected.remove(&id); }
    hub.devices.fetch_sub(1, Ordering::Relaxed);
    eprintln!("Device {id} {from}: closed after {} s, {streams} streams ({})", started.elapsed().as_secs(),
        connection.close_reason().map(|r| r.to_string()).unwrap_or_default());
}

/// One device stream onto one new stream to Diamond, both ways. The first request line
/// (method and path, never the query, which can carry signatures) goes to the log.
async fn splice(mut device_send: quinn::SendStream, mut device_recv: quinn::RecvStream, hub: Arc<Hub>, from: SocketAddr, id: usize, record: Arc<Device>) {
    let started = Instant::now();
    let mut first = vec![0u8; 4096];
    let n = match device_recv.read(&mut first).await { Ok(Some(n)) => n, _ => 0 };
    if first[..n].starts_with(INFO) {
        // Older clients read only the first two lines; "homeip" is on its own line after them.
        let mut reply = format!("home {}\nyou {from}\n", if hub.home_online() { "online" } else { "offline" });
        if let Some(ip) = hub.home_ip() { reply.push_str(&format!("homeip {ip}\n")) }
        let _ = device_send.write_all(reply.as_bytes()).await;
        let _ = device_send.finish();
        return;
    }
    if first[..n].starts_with(NAME) {
        let client = String::from_utf8_lossy(&first[NAME.len()..n]).trim().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(64).collect::<String>();
        if let Ok(mut held) = record.client.lock() { *held = client }
        let _ = device_send.finish();
        return;
    }
    let Some(home) = hub.home(Duration::from_secs(5)).await else {
        let _ = device_send.reset(HOME_OFFLINE.into());
        eprintln!("Device {id}: stream refused, Diamond is not connected");
        return;
    };
    let Ok((mut home_send, mut home_recv)) = home.open_bi().await else { let _ = device_send.reset(HOME_OFFLINE.into()); return };
    // Encrypted end to end (a TLS handshake): nothing here can be read.
    let line = if first.first() == Some(&0x16) { "(end to end)".to_owned() } else {
        String::from_utf8_lossy(&first[..n]).lines().next().unwrap_or("").split('?').next().unwrap_or("").to_owned()
    };
    if n > 0 && home_send.write_all(&first[..n]).await.is_err() { let _ = device_send.reset(HOME_OFFLINE.into()); return }
    hub.up.fetch_add(n as u64, Ordering::Relaxed); record.up.fetch_add(n as u64, Ordering::Relaxed);
    let up = async {
        let sent = counted_copy(&mut device_recv, &mut home_send, [&hub.up, &record.up]).await;
        match sent { Ok(_) => { let _ = home_send.finish(); } Err(_) => { let _ = home_send.reset(0u32.into()); } }
    };
    let down = async {
        let got = counted_copy(&mut home_recv, &mut device_send, [&hub.down, &record.down]).await;
        match got { Ok(bytes) => { let _ = device_send.finish(); bytes } Err(_) => { let _ = device_send.reset(0u32.into()); 0 } }
    };
    let (_, down_bytes) = tokio::join!(up, down);
    eprintln!("Device {id}: {line} → {down_bytes} bytes in {} ms", started.elapsed().as_millis());
}

/// Copy until the end, adding each chunk to `counters` as it goes (so a long video counts as
/// it plays, not when it ends).
async fn counted_copy<R: tokio::io::AsyncRead + Unpin, W: tokio::io::AsyncWrite + Unpin>(from: &mut R, to: &mut W, counters: [&AtomicU64; 2]) -> std::io::Result<u64> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = from.read(&mut buffer).await?;
        if n == 0 { return Ok(total) }
        to.write_all(&buffer[..n]).await?;
        total += n as u64;
        for counter in counters { counter.fetch_add(n as u64, Ordering::Relaxed); }
    }
}
