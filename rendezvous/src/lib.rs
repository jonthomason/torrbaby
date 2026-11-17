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
//! - TCP (loopback 7444, behind bronze's web server on 443): the relay over TCP, for networks
//!   that block UDP; see `serve_tcp`.
//!
//! Homes: Diamond is the home every existing stream goes to. Other homes (another machine of
//! the user's, such as the Mac's ABT host agent) each have their own secret in
//! `homes/<name>.secret` in the state directory, read again whenever a hello matches no known
//! secret. A home connects as Diamond does (QUIC, ALPN `tbhome`, `TBHOME1 <secret>\n`: the
//! secret says which home it is), or over TCP 443 by keeping idle upgraded connections here
//! (`serve_tcp`): bronze puts one device stream on each. A device names its home with a first
//! line `TBTO1 <name>\n` on a stream (or `X-Torrbaby-Home` on the upgrade); without one it is
//! Diamond, so nothing changes for Diamond or its devices. Diamond's Connection page counts
//! only Diamond's streams; other homes' bytes are counted per home.
//!
//! Punch hints, shared by every app and home: a home (Diamond, the Mac's ABT host) reports each
//! punch outcome on its proved connection with a stream `TBPUNCH1 <json>` (the device's public
//! address, its kind, the spray used, whether it got through, and how long it took). Bronze keeps
//! them (punches.jsonl in the state directory, 30 days) by the device's network: its IPv4 /24 or
//! IPv6 /56, since a hotel or carrier NAT hands out addresses from a pool. A device asks with a
//! `TBHINTS1\n` stream and gets the record for the network bronze sees it on; a home asks with
//! `TBHINTS1 <address>\n`. TBINFO's reply carries `spread random` when a predicted spray (a port
//! window, or STUN's exact ports) failed on that network and none got through after it.
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
pub const PUNCH: &[u8] = b"TBPUNCH1 ";
pub const HINTS: &[u8] = b"TBHINTS1";
/// A device stream that starts with `TBTO1 <home>\n` goes to that home instead of Diamond.
pub const TO: &[u8] = b"TBTO1 ";
/// After `TBHOME1 <secret>\n` and `OK\n`, an idle home connection over TCP gets this every
/// 20 s, and `CLAIMED` once when a device stream is put on it (raw bytes from then on).
pub const KEEPALIVE: u8 = b'.';
pub const CLAIMED: u8 = b'!';
/// Homes' idle connections over TCP, at most, each.
const MAX_POOL: usize = 16;
/// The path the relay over TCP is asked for, through bronze's web server.
pub const TCP_PATH: &str = "/tbrelay";
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

/// The homes' connections, shared with every device connection.
pub struct Hub {
    diamond: Arc<Home>,
    /// Every home by name, Diamond ("diamond") included.
    homes: std::sync::Mutex<std::collections::BTreeMap<String, Arc<Home>>>,
    /// Where the other homes' secrets are (`homes/` in the state directory).
    homes_dir: std::sync::Mutex<Option<std::path::PathBuf>>,
    devices: AtomicU64,
    streams: AtomicU64,
    /// The devices connected now, by connection.
    connected: std::sync::Mutex<std::collections::BTreeMap<usize, Arc<Device>>>,
    /// Bytes relayed since start (down: Diamond to devices; up: devices to Diamond), and the
    /// per-day totals they are folded into each minute.
    down: AtomicU64,
    up: AtomicU64,
    days: std::sync::Mutex<Days>,
    hints: std::sync::Mutex<hints::Store>,
}

/// Punch outcomes by the device's network, shared by every app and home (see the top).
pub mod hints {
    use serde_json::{Value, json};
    use std::net::IpAddr;
    use std::path::PathBuf;

    /// Outcomes are kept this long, and at most this many.
    const KEEP_SECONDS: u64 = 30 * 86400;
    const KEEP: usize = 20_000;

    /// The network an address belongs to: its IPv4 /24 or IPv6 /56.
    pub fn prefix(ip: IpAddr) -> String {
        match ip {
            IpAddr::V4(v4) => { let o = v4.octets(); format!("{}.{}.{}.0/24", o[0], o[1], o[2]) }
            IpAddr::V6(v6) => {
                let s = v6.segments();
                format!("{:x}:{:x}:{:x}:{:x}00::/56", s[0], s[1], s[2], s[3] >> 8)
            }
        }
    }

    #[derive(Default)]
    pub struct Store { pub(crate) outcomes: Vec<Value>, pub(crate) file: Option<PathBuf> }

    impl Store {
        /// Read `file` (dropping what is too old) and keep appending to it.
        pub fn load(&mut self, file: PathBuf, now: u64) {
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            self.outcomes = text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .filter(|v| v["at"].as_u64().is_some_and(|at| at + KEEP_SECONDS >= now)).collect();
            let start = self.outcomes.len().saturating_sub(KEEP);
            self.outcomes.drain(..start);
            // Rewritten without the old lines.
            let body: String = self.outcomes.iter().map(|v| format!("{v}\n")).collect();
            let temporary = file.with_extension("jsonl.tmp");
            if std::fs::write(&temporary, body).is_ok() { let _ = std::fs::rename(&temporary, &file); }
            self.file = Some(file);
        }

        /// One reported outcome from home `host`, cleaned to the fields kept. None when it is not one.
        pub fn add(&mut self, report: &Value, host: &str, now: u64) -> Option<Value> {
            let ip: IpAddr = report["address"].as_str()?.trim().parse().ok()?;
            let word = |key: &str, allowed: &[&str]| report[key].as_str().filter(|v| allowed.contains(v)).map(str::to_owned);
            let spread = word("spread", &["window", "exact", "random", "ipv6", "lan"])?;
            let ok = report["ok"].as_bool()?;
            let ms = |key: &str| report[key].as_u64().filter(|v| *v < 600_000);
            let app: String = report["app"].as_str().unwrap_or("").chars().filter(|c| c.is_ascii_alphanumeric()).take(16).collect();
            let outcome = json!({"prefix": prefix(ip), "kind": word("kind", &["wifi", "cellular", "wired", "unknown"]).unwrap_or_else(|| "unknown".into()),
                "spread": spread, "ok": ok, "punch_ms": ms("punch_ms"), "quic_ms": ms("quic_ms"), "host": host, "app": app, "at": now});
            if let Some(file) = &self.file {
                use std::io::Write;
                if let Ok(mut out) = std::fs::OpenOptions::new().create(true).append(true).open(file) { let _ = writeln!(out, "{outcome}"); }
            }
            self.outcomes.push(outcome.clone());
            if self.outcomes.len() > KEEP { let extra = self.outcomes.len() - KEEP; self.outcomes.drain(..extra); }
            Some(outcome)
        }

        /// What is known of `ip`'s network: the spread to start with, and its recent outcomes.
        pub fn for_address(&self, ip: IpAddr, now: u64) -> Value {
            let prefix = prefix(ip);
            let recent: Vec<&Value> = self.outcomes.iter().filter(|v| v["prefix"] == prefix.as_str() && v["at"].as_u64().is_some_and(|at| at + KEEP_SECONDS >= now)).collect();
            json!({"prefix": prefix, "spread": spread(&recent), "outcomes": recent.iter().rev().take(20).collect::<Vec<_>>()})
        }
    }

    /// "random" when a predicted spray (window or exact) failed on the network and no predicted
    /// spray got through after it; else null (nothing learned: the device decides as before).
    pub fn spread(recent: &[&Value]) -> Value {
        let predicted = |v: &&&Value| matches!(v["spread"].as_str(), Some("window" | "exact"));
        match recent.iter().filter(predicted).last() {
            Some(last) if last["ok"] == false => json!("random"),
            _ => Value::Null,
        }
    }
}

/// A machine devices are relayed to: Diamond, or another home.
pub struct Home {
    name: String,
    secret: [u8; 32],
    quic: watch::Sender<Option<quinn::Connection>>,
    /// Idle connections over TCP, newest last.
    pool: std::sync::Mutex<Vec<Pooled>>,
    pooled: tokio::sync::Notify,
    /// Bytes relayed since start (down: home to devices).
    down: AtomicU64,
    up: AtomicU64,
}

/// An idle home connection over TCP: claimed by sending where to put the stream.
struct Pooled { id: u64, from: SocketAddr, claim: tokio::sync::oneshot::Sender<tokio::sync::oneshot::Sender<tokio::net::TcpStream>> }

/// Where a device stream goes: a new stream on a home's QUIC connection, or one of its idle
/// TCP connections.
enum Upstream { Quic(quinn::SendStream, quinn::RecvStream), Tcp(tokio::net::TcpStream) }

static POOLED_IDS: AtomicU64 = AtomicU64::new(0);

impl Home {
    fn new(name: &str, secret: &str) -> Arc<Home> {
        Arc::new(Home { name: name.to_owned(), secret: Sha256::digest(secret.as_bytes()).into(), quic: watch::channel(None).0,
            pool: Default::default(), pooled: tokio::sync::Notify::new(), down: AtomicU64::new(0), up: AtomicU64::new(0) })
    }

    fn quic_live(&self) -> Option<quinn::Connection> {
        self.quic.borrow().as_ref().filter(|c| c.close_reason().is_none()).cloned()
    }

    fn online(&self) -> bool {
        self.quic_live().is_some() || self.pool.lock().is_ok_and(|pool| !pool.is_empty())
    }

    /// The home's public IP, as its standing connection comes from.
    fn ip(&self) -> Option<IpAddr> {
        self.quic_live().map(|c| c.remote_address().ip())
            .or_else(|| self.pool.lock().ok().and_then(|pool| pool.last().map(|p| p.from.ip())))
    }

    /// A way to the home for one stream, waiting up to `wait` for one (homes reconnect
    /// within seconds). QUIC first; else an idle connection over TCP.
    async fn open(&self, wait: Duration) -> Option<Upstream> {
        let deadline = tokio::time::Instant::now() + wait;
        let mut seen = self.quic.subscribe();
        loop {
            if let Some(connection) = self.quic_live() {
                return connection.open_bi().await.ok().map(|(send, recv)| Upstream::Quic(send, recv));
            }
            while let Some(idle) = self.pool.lock().ok().and_then(|mut pool| pool.pop()) {
                let (reply, stream) = tokio::sync::oneshot::channel();
                if idle.claim.send(reply).is_err() { continue }
                if let Ok(Ok(stream)) = tokio::time::timeout(Duration::from_secs(3), stream).await { return Some(Upstream::Tcp(stream)) }
            }
            tokio::select! {
                _ = seen.changed() => {}
                _ = self.pooled.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(250)) => {}
            }
            if tokio::time::Instant::now() >= deadline { return None }
        }
    }
}

/// Compare digests in full rather than stopping at the first difference.
fn same(a: &[u8; 32], b: &[u8; 32]) -> bool { a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0 }

/// A home's name: lowercase letters, digits and '-', up to 32.
fn home_name(text: &str) -> Option<String> {
    let name = text.trim();
    (!name.is_empty() && name.len() <= 32 && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')).then(|| name.to_owned())
}

/// Make the secret for home `name` in `dir`/homes (32 random bytes as hex) unless it exists;
/// returns its path.
pub fn create_home_secret(dir: &Path, name: &str) -> Result<std::path::PathBuf> {
    let Some(name) = home_name(name).filter(|n| n != "diamond") else { bail!("Not a home name: {name}") };
    let homes = dir.join("homes");
    std::fs::create_dir_all(&homes)?;
    let path = homes.join(format!("{name}.secret"));
    if !path.exists() {
        let mut bytes = [0u8; 32];
        std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom")?, &mut bytes)?;
        write_private(&path, bytes.iter().map(|b| format!("{b:02x}")).collect::<String>().as_bytes())?;
    }
    Ok(path)
}

/// One connected device.
struct Device {
    client: std::sync::Mutex<String>,
    address: std::sync::Mutex<SocketAddr>,
    since: u64,
    down: AtomicU64,
    up: AtomicU64,
    streams: AtomicU64,
    /// Streams open now (the relay over TCP: the record goes when the last one closes).
    open: AtomicU64,
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
        let diamond = Home::new("diamond", secret);
        let homes = std::sync::Mutex::new(std::collections::BTreeMap::from([("diamond".to_owned(), diamond.clone())]));
        Arc::new(Hub { diamond, homes, homes_dir: Default::default(), devices: AtomicU64::new(0), streams: AtomicU64::new(0),
            connected: Default::default(), down: AtomicU64::new(0), up: AtomicU64::new(0), days: Default::default(), hints: Default::default() })
    }

    /// Keep punch outcomes in `dir`/punches.jsonl (see `hints`).
    pub fn keep_hints(&self, dir: &Path) {
        if let Ok(mut hints) = self.hints.lock() { hints.load(dir.join("punches.jsonl"), unix_now()) }
    }

    /// What is known of the network `ip` is on (see `hints`).
    pub fn hints_for(&self, ip: IpAddr) -> serde_json::Value {
        self.hints.lock().map(|h| h.for_address(ip, unix_now())).unwrap_or_default()
    }

    /// Home `host` reports a punch outcome.
    pub fn report_punch(&self, report: &serde_json::Value, host: &str) -> bool {
        self.hints.lock().ok().and_then(|mut h| h.add(report, host, unix_now())).is_some()
    }

    /// The other homes' secrets are in `dir`/homes/<name>.secret: read now, and again when a
    /// hello matches no known secret (a home added while running).
    pub fn load_homes(&self, dir: &Path) {
        if let Ok(mut held) = self.homes_dir.lock() { *held = Some(dir.join("homes")) }
        self.rescan_homes();
    }

    fn rescan_homes(&self) {
        let Some(dir) = self.homes_dir.lock().ok().and_then(|d| d.clone()) else { return };
        let Ok(entries) = std::fs::read_dir(&dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("secret") { continue }
            let Some(name) = path.file_stem().and_then(|s| s.to_str()).and_then(home_name).filter(|n| n != "diamond") else { continue };
            let Ok(secret) = std::fs::read_to_string(&path) else { continue };
            let secret = secret.trim();
            if secret.len() < 32 { continue }
            let digest: [u8; 32] = Sha256::digest(secret.as_bytes()).into();
            let Ok(mut homes) = self.homes.lock() else { return };
            // A changed secret is a new home record: the old one's connections stay until they close.
            if homes.get(&name).is_some_and(|home| same(&home.secret, &digest)) { continue }
            homes.insert(name.clone(), Home::new(&name, secret));
            eprintln!("Home {name}: known");
        }
    }

    /// The home whose secret was offered, if any.
    fn home_by_secret(&self, offered: &str) -> Option<Arc<Home>> {
        let digest: [u8; 32] = Sha256::digest(offered.as_bytes()).into();
        let find = || self.homes.lock().ok().and_then(|homes| homes.values().find(|home| same(&home.secret, &digest)).cloned());
        find().or_else(|| { self.rescan_homes(); find() })
    }

    /// The home a stream names; none named: Diamond.
    fn named(&self, name: Option<&str>) -> Option<Arc<Home>> {
        match name {
            None | Some("diamond") => Some(self.diamond.clone()),
            Some(name) => self.homes.lock().ok().and_then(|homes| homes.get(name).cloned()),
        }
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

    fn home_online(&self) -> bool { self.diamond.online() }
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
        let Some(found) = hub.home_by_secret(offered) else { bail!("wrong secret") };
        send.write_all(b"OK\n").await?;
        send.finish()?;
        anyhow::Ok(Some(found))
    };
    let home = match tokio::time::timeout(Duration::from_secs(10), proved).await {
        Ok(Ok(Some(home))) => home,
        Ok(Ok(None)) => return,
        Ok(Err(error)) => { eprintln!("Home {from}: refused: {error}"); connection.close(3u32.into(), b"refused"); return }
        Err(_) => { eprintln!("Home {from}: refused: no hello"); connection.close(3u32.into(), b"refused"); return }
    };
    let previous = home.quic.send_replace(Some(connection.clone()));
    if let Some(previous) = previous { previous.close(4u32.into(), b"replaced") }
    let diamond = Arc::ptr_eq(&home, &hub.diamond);
    // Diamond's log lines stay as they were; another home is named.
    let label = if diamond { format!("Home {from}") } else { format!("Home {} {from}", home.name) };
    eprintln!("{label}: connected");
    // Diamond's own questions on its connection (it is proved): usage, for its Connection page.
    // Another home asking gets its own totals.
    let asking = connection.clone();
    let asked = hub.clone();
    let asker = home.clone();
    tokio::spawn(async move {
        while let Ok((mut send, mut recv)) = asking.accept_bi().await {
            let (hub, home) = (asked.clone(), asker.clone());
            tokio::spawn(async move {
                let Ok(request) = recv.read_to_end(4096).await else { return };
                if request.starts_with(PUNCH) {
                    let stored = serde_json::from_slice::<serde_json::Value>(&request[PUNCH.len()..]).ok().is_some_and(|report| hub.report_punch(&report, &home.name));
                    let _ = send.write_all(if stored { b"OK\n".as_slice() } else { b"NO\n".as_slice() }).await;
                } else if request.starts_with(HINTS) {
                    let asked = String::from_utf8_lossy(&request[HINTS.len()..]).trim().parse::<IpAddr>();
                    let reply = asked.map(|ip| hub.hints_for(ip)).unwrap_or_else(|_| serde_json::json!({"error": "TBHINTS1 <address>"}));
                    let _ = send.write_all(reply.to_string().as_bytes()).await;
                } else if request == USAGE {
                    let usage = if diamond { hub.usage() } else {
                        serde_json::json!({"home": home.name, "down": home.down.load(Ordering::Relaxed), "up": home.up.load(Ordering::Relaxed)})
                    };
                    let _ = send.write_all(usage.to_string().as_bytes()).await;
                }
                let _ = send.finish();
            });
        }
    });
    let started = Instant::now();
    let reason = connection.closed().await;
    home.quic.send_if_modified(|held| {
        if held.as_ref().is_some_and(|c| c.stable_id() == connection.stable_id()) { *held = None; true } else { false }
    });
    eprintln!("{label}: closed after {} s ({reason})", started.elapsed().as_secs());
}

async fn device(connection: quinn::Connection, hub: Arc<Hub>) {
    let from = connection.remote_address();
    let id = connection.stable_id();
    hub.devices.fetch_add(1, Ordering::Relaxed);
    eprintln!("Device {id} {from}: connected");
    let record = Arc::new(Device { client: Default::default(), address: std::sync::Mutex::new(from), since: unix_now(), down: AtomicU64::new(0), up: AtomicU64::new(0), streams: AtomicU64::new(0), open: AtomicU64::new(0) });
    let started = Instant::now();
    let mut streams = 0u64;
    while let Ok((send, recv)) = connection.accept_bi().await {
        streams += 1;
        hub.streams.fetch_add(1, Ordering::Relaxed);
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

/// One device stream onto one new stream to its home, both ways (a QUIC relay stream).
async fn splice(device_send: quinn::SendStream, device_recv: quinn::RecvStream, hub: Arc<Hub>, from: SocketAddr, id: usize, record: Arc<Device>) {
    let (mut device_send, mut device_recv) = (device_send, device_recv);
    let mut first = vec![0u8; 4096];
    let n = match device_recv.read(&mut first).await { Ok(Some(n)) => n, _ => 0 };
    first.truncate(n);
    // Listed on Diamond's Connection page once it names itself or uses Diamond (a device of
    // another home's is not Diamond's to see).
    let listing = hub.clone();
    let listed = record.clone();
    let list = move || {
        if let Ok(mut connected) = listing.connected.lock() { connected.entry(id).or_insert_with(|| listed.clone()); }
        listed.streams.fetch_add(1, Ordering::Relaxed);
        listed
    };
    if let Err(Refused) = relay(first, device_recv, &mut device_send, hub, from, &format!("Device {id}"), None, list).await {
        let _ = device_send.reset(HOME_OFFLINE.into());
    }
}

/// The home was not connected: the device's stream is refused.
struct Refused;

/// One device stream (any byte stream; `first` is what was read of it so far) onto one new
/// stream to its home, both ways, or bronze's own answer to TBINFO and TBNAME. The home is
/// named by a first line `TBTO1 <name>\n` (else `home`, else Diamond). `record` gives the
/// device's record on Diamond's Connection page, made when first needed: Diamond's streams
/// only (and TBNAME). The first request line (method and path, never the query, which can
/// carry signatures) goes to the log; streams encrypted end to end show as such.
#[allow(clippy::too_many_arguments)]
async fn relay<R, W>(mut first: Vec<u8>, mut device_recv: R, device_send: &mut W, hub: Arc<Hub>, from: SocketAddr, who: &str, home: Option<String>, record: impl FnOnce() -> Arc<Device>) -> Result<(), Refused>
where R: tokio::io::AsyncRead + Unpin, W: tokio::io::AsyncWrite + Unpin {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let started = Instant::now();
    let mut named = home;
    if first.starts_with(TO) {
        let mut more = [0u8; 4096];
        while !first.contains(&b'\n') && first.len() < 128 {
            let m = device_recv.read(&mut more).await.unwrap_or(0);
            if m == 0 { break }
            first.extend_from_slice(&more[..m]);
        }
        let Some(end) = first.iter().position(|b| *b == b'\n') else { return Err(Refused) };
        named = Some(String::from_utf8_lossy(&first[TO.len()..end]).trim().to_owned());
        first.drain(..=end);
        if first.is_empty() {
            let m = device_recv.read(&mut more).await.unwrap_or(0);
            first.extend_from_slice(&more[..m]);
        }
    }
    let n = first.len();
    let Some(home) = hub.named(named.as_deref()) else {
        eprintln!("{who}: stream refused, no home {}", named.unwrap_or_default());
        return Err(Refused);
    };
    let diamond = Arc::ptr_eq(&home, &hub.diamond);
    if first.starts_with(INFO) {
        // Older clients read only the first two lines; "homeip" is on its own line after them.
        let mut reply = format!("home {}\nyou {from}\n", if home.online() { "online" } else { "offline" });
        if let Some(ip) = home.ip() { reply.push_str(&format!("homeip {ip}\n")) }
        if let Some(spread) = hub.hints_for(from.ip())["spread"].as_str() { reply.push_str(&format!("spread {spread}\n")) }
        let _ = device_send.write_all(reply.as_bytes()).await;
        let _ = device_send.shutdown().await;
        return Ok(());
    }
    if first.starts_with(HINTS) {
        let _ = device_send.write_all(hub.hints_for(from.ip()).to_string().as_bytes()).await;
        let _ = device_send.shutdown().await;
        return Ok(());
    }
    if first.starts_with(NAME) {
        let client = String::from_utf8_lossy(&first[NAME.len()..n]).trim().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(64).collect::<String>();
        if let Ok(mut held) = record().client.lock() { *held = client }
        let _ = device_send.shutdown().await;
        return Ok(());
    }
    let Some(upstream) = home.open(Duration::from_secs(5)).await else {
        if diamond { eprintln!("{who}: stream refused, Diamond is not connected") } else { eprintln!("{who}: stream refused, home {} is not connected", home.name) }
        return Err(Refused);
    };
    // Diamond's streams count for its Connection page; another home's for that home only.
    let none = AtomicU64::new(0);
    let device = diamond.then(record);
    let (up_counters, down_counters): ([&AtomicU64; 2], [&AtomicU64; 2]) = match &device {
        Some(device) => ([&hub.up, &device.up], [&hub.down, &device.down]),
        None => ([&home.up, &none], [&home.down, &none]),
    };
    // Encrypted end to end (a TLS handshake): nothing here can be read.
    let line = if first.first() == Some(&0x16) { "(end to end)".to_owned() } else {
        String::from_utf8_lossy(&first[..n]).lines().next().unwrap_or("").split('?').next().unwrap_or("").to_owned()
    };
    let down_bytes = match upstream {
        Upstream::Quic(mut home_send, mut home_recv) => {
            if n > 0 && home_send.write_all(&first[..n]).await.is_err() { return Err(Refused) }
            for counter in up_counters { counter.fetch_add(n as u64, Ordering::Relaxed); }
            let up = async {
                let sent = counted_copy(&mut device_recv, &mut home_send, up_counters).await;
                match sent { Ok(_) => { let _ = home_send.finish(); } Err(_) => { let _ = home_send.reset(0u32.into()); } }
            };
            let down = async {
                let got = counted_copy(&mut home_recv, device_send, down_counters).await;
                let _ = device_send.shutdown().await;
                got.unwrap_or(0)
            };
            tokio::join!(up, down).1
        }
        Upstream::Tcp(tcp) => {
            let _ = tcp.set_nodelay(true);
            let (mut home_recv, mut home_send) = tcp.into_split();
            if n > 0 && home_send.write_all(&first[..n]).await.is_err() { return Err(Refused) }
            for counter in up_counters { counter.fetch_add(n as u64, Ordering::Relaxed); }
            let up = async {
                let _ = counted_copy(&mut device_recv, &mut home_send, up_counters).await;
                let _ = home_send.shutdown().await;
            };
            let down = async {
                let got = counted_copy(&mut home_recv, device_send, down_counters).await;
                let _ = device_send.shutdown().await;
                got.unwrap_or(0)
            };
            tokio::join!(up, down).1
        }
    };
    if diamond { eprintln!("{who}: {line} → {down_bytes} bytes in {} ms", started.elapsed().as_millis()) }
    else { eprintln!("{who} → home {}: {line} → {down_bytes} bytes in {} ms", home.name, started.elapsed().as_millis()) }
    Ok(())
}

/// The relay over TCP, for networks that block UDP: bronze's web server (Caddy, which owns
/// TCP 443 with a Let's Encrypt certificate for bronze's IP) hands each `GET /tbrelay` that
/// asks to upgrade to this loopback listener. After the `101` the connection is one relayed
/// stream, spliced exactly as a QUIC relay stream. The device's address comes from Caddy's
/// X-Forwarded-For, its client id from X-Torrbaby-Client. Streams from one device share one
/// record on the Connection page while any is open.
pub async fn serve_tcp(listener: tokio::net::TcpListener, hub: Arc<Hub>) {
    while let Ok((tcp, _)) = listener.accept().await {
        let hub = hub.clone();
        tokio::spawn(async move { let _ = tcp_stream(tcp, hub).await; });
    }
}

/// Relayed streams open over TCP now, and at most (each is a socket here and one at Caddy).
static TCP_OPEN: AtomicU64 = AtomicU64::new(0);
const MAX_TCP_STREAMS: usize = 4096;

/// Key of the Connection page's records for devices on the relay over TCP (QUIC connections
/// use their stable id, which counts up from 0).
fn tcp_key(client: &str, ip: &str) -> usize {
    let digest = Sha256::digest(format!("tcp {client} {ip}").as_bytes());
    usize::MAX / 2 + (u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) as usize)
}

async fn tcp_stream(mut tcp: tokio::net::TcpStream, hub: Arc<Hub>) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _ = tcp.set_nodelay(true);
    // The request's head, a byte at a time so nothing after it is taken.
    let mut head = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !head.ends_with(b"\r\n\r\n") {
            if head.len() > 8192 { bail!("request head too long") }
            head.push(tcp.read_u8().await?);
        }
        anyhow::Ok(())
    }).await??;
    let head = String::from_utf8_lossy(&head).into_owned();
    let mut lines = head.split("\r\n");
    let request = lines.next().unwrap_or("");
    let header = |name: &str| head.split("\r\n").skip(1).find_map(|line| line.split_once(':').filter(|(key, _)| key.trim().eq_ignore_ascii_case(name)).map(|(_, value)| value.trim().to_owned()));
    if !request.starts_with(&format!("GET {TCP_PATH} ")) || header("Upgrade").is_none() {
        tcp.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?;
        return Ok(());
    }
    // Caddy appends the address it saw last.
    let ip = header("X-Forwarded-For").and_then(|f| f.rsplit(',').next().map(|ip| ip.trim().to_owned())).unwrap_or_default();
    let from = ip.parse::<IpAddr>().map(|ip| SocketAddr::new(ip, 0)).unwrap_or_else(|_| tcp.peer_addr().unwrap_or(SocketAddr::from(([0, 0, 0, 0], 0))));
    let client = header("X-Torrbaby-Client").unwrap_or_default().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(64).collect::<String>();
    if TCP_OPEN.load(Ordering::Relaxed) as usize >= MAX_TCP_STREAMS {
        tcp.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?;
        return Ok(());
    }
    tcp.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n").await?;
    let mut first = vec![0u8; 4096];
    let n = tokio::time::timeout(Duration::from_secs(30), tcp.read(&mut first)).await.unwrap_or(Ok(0)).unwrap_or(0);
    first.truncate(n);
    // A home keeping an idle connection here, not a device.
    if first.starts_with(HOME_HELLO.as_bytes()) { return pooled_home(tcp, first, hub, from).await }
    let home = header("X-Torrbaby-Home");
    hub.streams.fetch_add(1, Ordering::Relaxed);
    TCP_OPEN.fetch_add(1, Ordering::Relaxed);
    // One record per device while any of its Diamond streams is open.
    let key = tcp_key(&client, &ip);
    let made: Arc<std::sync::Mutex<Option<Arc<Device>>>> = Default::default();
    let (listing, holder, named) = (hub.clone(), made.clone(), client.clone());
    let record = move || {
        let record = listing.connected.lock().map(|mut connected| connected.entry(key).or_insert_with(|| Arc::new(Device { client: std::sync::Mutex::new(named.clone()), address: std::sync::Mutex::new(from), since: unix_now(),
            down: AtomicU64::new(0), up: AtomicU64::new(0), streams: AtomicU64::new(0), open: AtomicU64::new(0) })).clone())
            .unwrap_or_else(|_| Arc::new(Device { client: Default::default(), address: std::sync::Mutex::new(from), since: unix_now(), down: AtomicU64::new(0), up: AtomicU64::new(0), streams: AtomicU64::new(0), open: AtomicU64::new(0) }));
        record.open.fetch_add(1, Ordering::Relaxed);
        record.streams.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut held) = holder.lock() { *held = Some(record.clone()) }
        record
    };
    let (recv, mut send) = tcp.into_split();
    let _ = relay(first, recv, &mut send, hub.clone(), from, &format!("Device tcp {}", if client.is_empty() { ip.as_str() } else { client.as_str() }), home, record).await;
    TCP_OPEN.fetch_sub(1, Ordering::Relaxed);
    let record = made.lock().ok().and_then(|mut held| held.take());
    if let Some(record) = record && record.open.fetch_sub(1, Ordering::Relaxed) == 1 {
        if let Ok(mut connected) = hub.connected.lock() {
            if connected.get(&key).is_some_and(|held| held.open.load(Ordering::Relaxed) == 0) { connected.remove(&key); }
        }
    }
    Ok(())
}

/// A home's idle connection over TCP: after `TBHOME1 <secret>\n` it gets `OK\n`, then
/// KEEPALIVE every 20 s while idle, and CLAIMED once a device stream is put on it (raw bytes
/// both ways from then on, the device's first). The home opens another as each is claimed.
/// Anything the home sends while idle, or its closing, ends it.
async fn pooled_home(tcp: tokio::net::TcpStream, mut first: Vec<u8>, hub: Arc<Hub>, from: SocketAddr) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut tcp = tcp;
    while !first.contains(&b'\n') {
        if first.len() > 256 { bail!("hello too long") }
        first.push(tokio::time::timeout(Duration::from_secs(10), tcp.read_u8()).await??);
    }
    let end = first.iter().position(|b| *b == b'\n').unwrap_or(first.len());
    if end + 1 != first.len() { bail!("data after the hello") }
    let text = String::from_utf8_lossy(&first[..end]).into_owned();
    let Some(home) = hub.home_by_secret(text.trim_end().strip_prefix(HOME_HELLO).unwrap_or("")) else {
        eprintln!("Home tcp {from}: refused: wrong secret");
        return Ok(());
    };
    let (claim, mut claimed) = tokio::sync::oneshot::channel();
    let id = POOLED_IDS.fetch_add(1, Ordering::Relaxed);
    let full = match home.pool.lock() {
        Ok(mut pool) if pool.len() < MAX_POOL => { pool.push(Pooled { id, from, claim }); false }
        _ => true,
    };
    if full { tcp.write_all(b"FULL\n").await?; return Ok(()) }
    if tcp.write_all(b"OK\n").await.is_err() { home.pool.lock().map_err(|_| anyhow::anyhow!("lock"))?.retain(|p| p.id != id); return Ok(()) }
    home.pooled.notify_one();
    let (mut reader, mut writer) = tcp.into_split();
    let mut byte = [0u8; 1];
    let ended = loop {
        tokio::select! {
            asked = &mut claimed => {
                let Ok(reply) = asked else { break "dropped" };
                if writer.write_all(&[CLAIMED]).await.is_err() { break "closed" }
                if let Ok(tcp) = reader.reunite(writer) { let _ = reply.send(tcp); }
                return Ok(());
            }
            _ = tokio::time::sleep(Duration::from_secs(20)) => { if writer.write_all(&[KEEPALIVE]).await.is_err() { break "closed" } }
            _ = reader.read(&mut byte) => break "closed by the home",
        }
    };
    if let Ok(mut pool) = home.pool.lock() { pool.retain(|p| p.id != id) }
    let _ = ended;
    Ok(())
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
