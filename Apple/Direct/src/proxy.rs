//! Away from home without a packet tunnel (the Android TV app): one loopback port whose every
//! connection goes to Diamond by the best route open, and a maintainer that keeps the routes
//! open. The app's base URL (http://127.0.0.1:<port>) stays the same whatever happens beneath
//! it: the relay opening, a direct path punched or lost, a new network, dormancy.
//!
//! Routing is the packet tunnel's (packet::route with crate::carrier/superseded): the direct
//! path while it is open, else the relay through bronze (each stream TLS end to end to
//! Diamond's pinned identity); a connection on the relay makes way once the direct path opens.
//!
//! The maintainer is PacketTunnelProvider.maintain() (iOS, Swift) in Rust, so other clients
//! share it:
//! - the relay is kept open; every 6 s bronze is asked (TBINFO) for this device's public
//!   address and Diamond's. The same address means this device is behind Diamond's own NAT,
//!   where a punch can only hairpin: relay only, no punching. (At home the app itself finds
//!   Diamond by Bonjour and stops the proxy.)
//! - a direct path is punched (punch.rs: STUN on bronze, POST /v1/direct through the relay,
//!   QUIC to Diamond's pinned identity on the winning socket) when there is none. After a
//!   failure: 5 s, 15 s, 30 s, then every minute where punching has worked on this network or
//!   on this carrier before; else 1, 2, 5, then 15 min, with a gentler spray. A predicted window
//!   that failed from a public address is followed by random ports from there.
//! - a new public address under an open direct path (a carrier switch behind a hotspot):
//!   the relay is first reconnected over the current network to compare like with like, then
//!   the path is punched again.
//! - a network change (told by the app, `network`) that holds for 3 s drops the direct path
//!   and moves the relay onto a socket on the new network (QUIC migration).
//! - nothing open for `idle` (2 min): both paths are let go (no keepalives) until a connection
//!   waits, which wakes them within about a second.
use crate::{RELAY, TUNNEL, alive, packet, punch};
use std::collections::{HashSet, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How the proxy reaches bronze and Diamond.
#[derive(Clone)]
pub struct Config {
    /// bronze's relay ("ip:port") and its certificate fingerprint.
    pub relay: String,
    pub relay_fingerprint: String,
    /// bronze's STUN server ("ip:3478"); empty: the relay's host on 3478. The second STUN port:
    /// none, the same host on 3479.
    pub stun: String,
    pub stun2: Option<String>,
    /// This device's token for Diamond, which punching asks with (POST /v1/direct). Empty:
    /// relay only (a TV claiming its setup has no token yet).
    pub token: String,
    /// Where carriers that have punched are remembered across restarts; none: in memory.
    pub state_dir: Option<PathBuf>,
    /// Nothing open this long: the paths are let go until the next connection.
    pub idle: Duration,
    /// How long `start` waits for the relay to open before giving up.
    pub open_wait: Duration,
}

impl Config {
    pub fn new(relay: &str, relay_fingerprint: &str) -> Config {
        Config { relay: relay.into(), relay_fingerprint: relay_fingerprint.trim().to_lowercase(), stun: String::new(), stun2: None,
            token: String::new(), state_dir: None, idle: Duration::from_secs(120), open_wait: Duration::from_secs(7) }
    }
    fn stun(&self) -> String {
        if !self.stun.is_empty() { return self.stun.clone() }
        self.relay.rsplit_once(':').map(|(host, _)| format!("{host}:3478")).unwrap_or_default()
    }
}

/// What the status line shows, kept by the maintainer.
#[derive(Default)]
struct Status {
    state: String,
    /// How the open direct path was made.
    punched: String,
    last_punch: String,
    next_punch: Option<Instant>,
    punching: bool,
    dormant: bool,
    public_ip: String,
}

struct Shared {
    config: Mutex<Config>,
    status: Mutex<Status>,
    stopped: AtomicBool,
}

struct Proxy {
    runtime: tokio::runtime::Runtime,
    port: u16,
    shared: Arc<Shared>,
}

static PROXY: Mutex<Option<Proxy>> = Mutex::new(None);
/// The network the app says it is on: a signature that changes with the network, and the
/// interface's name. Kept whether or not the proxy runs.
static NETWORK: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));
static NOTES: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
static LOGGER: Mutex<Option<fn(&str)>> = Mutex::new(None);

/// Where each line of what the maintainer does goes (Android: logcat), besides `notes`.
pub fn set_logger(logger: fn(&str)) { if let Ok(mut held) = LOGGER.lock() { *held = Some(logger) } }

fn note(line: &str) {
    if let Ok(mut notes) = NOTES.lock() {
        notes.push_back(line.to_owned());
        while notes.len() > 64 { notes.pop_front(); }
    }
    let logger = LOGGER.lock().ok().and_then(|held| *held);
    if let Some(logger) = logger { logger(line) }
}

/// The maintainer's recent lines, oldest first.
pub fn notes() -> Vec<String> { NOTES.lock().map(|n| n.iter().cloned().collect()).unwrap_or_default() }

/// Start the proxy (or, already running for this relay, take the new token and STUN) and
/// return its loopback port; 0 when the relay did not open within `open_wait`. Blocks until
/// then: call it off the main thread, and not from inside an async task.
pub fn start(config: Config) -> u16 {
    if let Ok(held) = PROXY.lock() && let Some(proxy) = held.as_ref() {
        let mut current = proxy.shared.config.lock().unwrap_or_else(|e| e.into_inner());
        if current.relay == config.relay && current.relay_fingerprint == config.relay_fingerprint {
            *current = config;
            return proxy.port;
        }
    }
    stop();
    let Ok(relay) = config.relay.parse::<SocketAddr>() else { note(&format!("proxy: bronze's address is not valid: {}", config.relay)); return 0 };
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("tbproxy").enable_all().build() else { return 0 };
    let Ok(listener) = runtime.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")) else { return 0 };
    let Ok(port) = listener.local_addr().map(|a| a.port()) else { return 0 };
    let open_wait = config.open_wait;
    let shared = Arc::new(Shared { config: Mutex::new(config), status: Mutex::new(Status { state: "starting".into(), ..Status::default() }), stopped: AtomicBool::new(false) });
    runtime.spawn(async move {
        let carrier: Arc<packet::Carrier> = Arc::new(Box::new(crate::carrier));
        let superseded: Arc<packet::Superseded> = Arc::new(Box::new(crate::superseded));
        loop {
            match listener.accept().await {
                Ok((tcp, _)) => {
                    let _ = tcp.set_nodelay(true);
                    // Up to 20 s for a route: a dormant proxy wakes and opens the relay meanwhile.
                    tokio::spawn(packet::route(tcp, carrier.clone(), superseded.clone(), Duration::from_secs(20)));
                }
                // Out of descriptors, say: a moment, then on.
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    });
    runtime.spawn(Keeper::new(shared.clone(), relay).run());
    if let Ok(mut held) = PROXY.lock() { *held = Some(Proxy { runtime, port, shared }) }
    note(&format!("proxy: loopback port {port}, relay {relay}"));
    let deadline = Instant::now() + open_wait;
    while Instant::now() < deadline {
        if alive(&RELAY) { return port }
        std::thread::sleep(Duration::from_millis(50));
    }
    note(&format!("proxy: the relay at {relay} did not open; stopped"));
    stop();
    0
}

/// Whether the proxy is running.
pub fn running() -> bool { PROXY.lock().ok().is_some_and(|held| held.is_some()) }

/// Stop the proxy, the direct path and the relay.
pub fn stop() {
    let proxy = PROXY.lock().ok().and_then(|mut held| held.take());
    if let Some(proxy) = proxy {
        proxy.shared.stopped.store(true, Ordering::SeqCst);
        proxy.runtime.shutdown_background();
        crate::stop(&TUNNEL);
        crate::stop(&RELAY);
        note("proxy: stopped");
    }
}

/// The network changed (or was first seen): `signature` identifies it (interface, addresses,
/// gateway), `interface` is its name ("wlan0", "eth0", "rmnet0"). A change that holds for 3 s
/// drops the direct path and moves the relay.
pub fn network(signature: &str, interface: &str) {
    let changed = NETWORK.lock().map(|mut held| {
        let changed = held.0 != signature;
        *held = (signature.to_owned(), interface.to_owned());
        changed
    }).unwrap_or(false);
    if changed { note(&format!("network: {signature}")) }
}

/// One line for the log: the routes, what the maintainer is doing, the last punch, and what
/// the proxy carried.
pub fn status() -> String {
    use packet::stats::*;
    let held = PROXY.lock().ok().and_then(|held| held.as_ref().map(|p| (p.port, p.shared.clone())));
    let Some((port, shared)) = held else { return "proxy stopped".into() };
    let s = shared.status.lock().unwrap_or_else(|e| e.into_inner());
    let direct = if alive(&TUNNEL) { format!("direct open ({})", s.punched) } else { "direct closed".into() };
    let relay = if alive(&RELAY) { "relay open" } else { "relay closed" };
    let e2e = if crate::e2e::pinned().is_some() { "end to end to Diamond" } else { "not end to end yet" };
    let mut line = format!("port {port}: {direct}, {relay} ({e2e}){}; {}", if s.dormant { ", dormant" } else { "" }, s.state);
    if s.punching { line.push_str("; punching now") }
    if !s.last_punch.is_empty() { line.push_str(&format!("; last punch {}", s.last_punch)) }
    if let Some(next) = s.next_punch && next > Instant::now() && !alive(&TUNNEL) {
        line.push_str(&format!("; next punch in {} s", (next - Instant::now()).as_secs()))
    }
    if !s.public_ip.is_empty() { line.push_str(&format!("; public address {}", s.public_ip)) }
    let network = NETWORK.lock().map(|n| n.0.clone()).unwrap_or_default();
    if !network.is_empty() { line.push_str(&format!("; network {network}")) }
    line.push_str(&format!("; connections open {} (opened {}, waiting {}, moved to the direct path {}, uncarried {}, abandoned {}), down {} KB, up {} KB",
        get(&FLOWS_OPEN), get(&FLOWS_OPENED), get(&FLOWS_WAITING), get(&FLOWS_MOVED), get(&FLOWS_UNCARRIED), get(&FLOWS_ABANDONED),
        get(&STREAM_DOWN) / 1024, get(&STREAM_UP) / 1024));
    line.push_str(&crate::paths_line());
    line
}

/// A punch's outcome, from its blocking thread.
enum Outcome {
    Opened { summary: String, public_address: String, quic_ms: u64 },
    Failed(punch::Failure),
    /// The network changed (or the proxy stopped) while it ran: not used, not counted.
    Stale,
}

struct Keeper {
    shared: Arc<Shared>,
    relay: SocketAddr,
    /// The relay's own loopback port (plain relay streams): signalling goes over it.
    relay_port: u16,
    dormant: bool,
    last_busy: Instant,
    last_info: Option<Instant>,
    /// The public IP the direct path was punched from.
    punched_from: Option<String>,
    /// bronze saw another address than the direct path's, and the relay was reconnected over
    /// the current network to compare like with like.
    relay_reopened_for_mismatch: bool,
    /// Failed punches on this network, and when the next may start.
    punch_failures: usize,
    next_punch: Instant,
    punched_on_this_network: bool,
    public_ip: Option<String>,
    /// Same public address as Diamond: relay only.
    behind_diamonds_nat: bool,
    /// Public addresses a predicted spray failed from: random ports from there next time.
    window_failed_from: HashSet<String>,
    /// The network acted on, and a different one waiting to hold for 3 s.
    network: String,
    pending: Option<(String, Instant)>,
    /// Counts network changes: a punch begun on an older network is not used.
    epoch: Arc<AtomicU64>,
    punch: Option<tokio::task::JoinHandle<Outcome>>,
}

impl Keeper {
    fn new(shared: Arc<Shared>, relay: SocketAddr) -> Keeper {
        Keeper { shared, relay, relay_port: 0, dormant: false, last_busy: Instant::now(), last_info: None, punched_from: None,
            relay_reopened_for_mismatch: false, punch_failures: 0, next_punch: Instant::now(), punched_on_this_network: false,
            public_ip: None, behind_diamonds_nat: false, window_failed_from: HashSet::new(), network: String::new(), pending: None,
            epoch: Arc::new(AtomicU64::new(0)), punch: None }
    }

    fn state(&self, text: &str) { if let Ok(mut s) = self.shared.status.lock() { s.state = text.into() } }

    /// Each second: network changes and a finished punch; the paths every 3 s, and at once
    /// while a connection waits for one.
    async fn run(mut self) {
        let mut ticks: u64 = 0;
        loop {
            self.watch_network();
            self.finish_punch().await;
            if ticks % 3 == 0 || packet::stats::get(&packet::stats::FLOWS_WAITING) > 0 { self.maintain().await }
            ticks += 1;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    fn watch_network(&mut self) {
        let signature = NETWORK.lock().map(|n| n.0.clone()).unwrap_or_default();
        if signature.is_empty() { return }
        if self.network.is_empty() { self.network = signature; return }
        if signature == self.network { self.pending = None; return }
        match &self.pending {
            Some((waiting, since)) if *waiting == signature => if since.elapsed() >= Duration::from_secs(3) { self.network_changed(signature) },
            _ => self.pending = Some((signature, Instant::now())),
        }
    }

    /// A different network has held for 3 s: the direct path's hole was for the old one. The
    /// relay moves with QUIC to a socket on the new network (bronze has a public address and
    /// simply answers the new one); only if it cannot move is it opened again.
    fn network_changed(&mut self, signature: String) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        crate::stop(&TUNNEL);
        let moved = !self.dormant && alive(&RELAY) && std::net::UdpSocket::bind("0.0.0.0:0").is_ok_and(crate::relay_rebind);
        if !moved { crate::stop(&RELAY) }
        note(&format!("network changed ({} → {signature}): direct path dropped, relay {}", self.network,
            if moved { "moved to the new network" } else if self.dormant { "stays closed (dormant)" } else { "reopening" }));
        self.network = signature;
        self.pending = None;
        self.punched_from = None; self.behind_diamonds_nat = false; self.relay_reopened_for_mismatch = false;
        self.punch_failures = 0; self.next_punch = Instant::now(); self.punched_on_this_network = false; self.public_ip = None;
        self.last_info = None;
        if let Ok(mut s) = self.shared.status.lock() { s.next_punch = None; s.public_ip.clear() }
    }

    async fn maintain(&mut self) {
        // A punch is under way (up to about 30 s): its outcome is taken by finish_punch.
        if self.punch.is_some() { return }
        let open = packet::stats::get(&packet::stats::FLOWS_OPEN);
        let waiting = packet::stats::get(&packet::stats::FLOWS_WAITING);
        if open > 0 || waiting > 0 { self.last_busy = Instant::now() }
        let idle = self.shared.config.lock().map(|c| c.idle).unwrap_or(Duration::from_secs(120));
        if self.dormant {
            if waiting == 0 { return }
            self.dormant = false;
            self.last_busy = Instant::now();
            if let Ok(mut s) = self.shared.status.lock() { s.dormant = false }
            note("a connection is waiting: waking (opening the relay)");
        } else if self.last_busy.elapsed() > idle {
            // Battery and bronze: with nothing open, the paths' keepalives are not worth it.
            self.dormant = true;
            crate::stop(&TUNNEL);
            crate::stop(&RELAY);
            self.punched_from = None;
            if let Ok(mut s) = self.shared.status.lock() { s.dormant = true }
            self.state("dormant: idle, nothing sent until the next connection");
            note(&format!("idle for {} s: paths let go, no keepalives, until the next connection", idle.as_secs()));
            return
        }
        if !alive(&RELAY) { self.open_relay().await }
        // Every 6 s, bronze's view: behind Diamond's NAT, or the public address moved (a carrier
        // switch behind a hotspot, invisible from here) and the direct path's hole is for the old one.
        if alive(&RELAY) && self.last_info.is_none_or(|at| at.elapsed() > Duration::from_secs(6)) {
            self.last_info = Some(Instant::now());
            if let Some((ip, home_ip)) = info().await {
                self.public_ip = Some(ip.clone());
                if let Ok(mut s) = self.shared.status.lock() { s.public_ip = ip.clone() }
                if home_ip.as_deref() == Some(ip.as_str()) {
                    // Behind Diamond's own NAT a punch can only hairpin, which gets through one
                    // way at best: the relay carries everything here.
                    if !self.behind_diamonds_nat { note(&format!("same public address as Diamond ({ip}): relay only, no punching")) }
                    crate::stop(&TUNNEL);
                    self.punched_from = None;
                    self.behind_diamonds_nat = true;
                    self.state("behind Diamond's own network address: relay only");
                    return
                }
                if self.behind_diamonds_nat { note(&format!("public address {ip} is no longer Diamond's: direct paths may be punched")) }
                self.behind_diamonds_nat = false;
                match self.punched_from.clone() {
                    Some(from) if alive(&TUNNEL) && from != ip => {
                        if !self.relay_reopened_for_mismatch {
                            note(&format!("bronze sees {ip}, the direct path is from {from}: reconnecting the relay over the current network first"));
                            crate::stop(&RELAY);
                            self.relay_reopened_for_mismatch = true;
                            return
                        }
                        note(&format!("public address changed ({from} → {ip}): punching again"));
                        crate::stop(&TUNNEL);
                        self.punched_from = None;
                        self.next_punch = Instant::now();
                        self.relay_reopened_for_mismatch = false;
                    }
                    _ => self.relay_reopened_for_mismatch = false,
                }
            }
        }
        if alive(&TUNNEL) || self.behind_diamonds_nat || !alive(&RELAY) { return }
        let config = self.shared.config.lock().map(|c| c.clone()).ok();
        let Some(config) = config else { return };
        if config.token.is_empty() { self.state("relay only: no device token to ask Diamond for a direct path"); return }
        // After a failure, the backoff; a path that dropped is tried at once.
        if Instant::now() < self.next_punch { self.state("relay; waiting to punch again"); return }
        self.start_punch(config);
    }

    async fn open_relay(&mut self) {
        let config = self.shared.config.lock().map(|c| c.clone()).ok();
        let Some(config) = config else { return };
        let relay = self.relay;
        note(&format!("relay: connecting to {relay}"));
        let port = tokio::task::spawn_blocking(move || {
            crate::stop(&RELAY);
            let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") else { return 0 };
            crate::open_on(&RELAY, socket, relay, config.relay_fingerprint, b"tbrelay", "torrbaby-rendezvous", 5, crate::RELAY_TIMING, crate::APP_WINDOWS)
        }).await.unwrap_or(0);
        self.relay_port = port;
        if port == 0 {
            self.state(&format!("relay: could not reach {relay}"));
            note(&format!("relay: could not reach {relay}"));
        } else {
            self.state("relay open");
            note(&format!("relay open (signalling on loopback port {port})"));
        }
    }

    /// Carriers where a punch has worked: the kind of connection and the public IP's first two
    /// parts (China Mobile punches on cellular, but a hotspot on it may never).
    fn carrier_key(&self, ip: &str) -> String {
        let interface = NETWORK.lock().map(|n| n.1.clone()).unwrap_or_default();
        let kind = if ["rmnet", "ccmni", "pdp"].iter().any(|p| interface.starts_with(p)) { "cellular" } else { "wifi" };
        format!("{kind} {}", ip.split('.').take(2).collect::<Vec<_>>().join("."))
    }
    fn carriers_file(&self) -> Option<PathBuf> {
        self.shared.config.lock().ok().and_then(|c| c.state_dir.clone()).map(|dir| dir.join("punchable-carriers.txt"))
    }
    fn remembered_carriers(&self) -> Vec<String> {
        let held = REMEMBERED.lock().map(|r| r.clone()).unwrap_or_default();
        let Some(file) = self.carriers_file() else { return held };
        let mut carriers: Vec<String> = std::fs::read_to_string(file).unwrap_or_default().lines().map(str::to_owned).filter(|l| !l.is_empty()).collect();
        for c in held { if !carriers.contains(&c) { carriers.push(c) } }
        carriers
    }
    fn remember_carrier(&self, ip: &str) {
        let key = self.carrier_key(ip);
        let mut carriers = self.remembered_carriers();
        if carriers.contains(&key) { return }
        carriers.push(key);
        let keep: Vec<String> = carriers.iter().rev().take(64).rev().cloned().collect();
        if let Ok(mut held) = REMEMBERED.lock() { *held = keep.clone() }
        if let Some(file) = self.carriers_file() { let _ = std::fs::write(file, keep.join("\n") + "\n"); }
    }
    /// Six failures in a row even here: treated as a network that does not punch.
    fn punchable(&self) -> bool {
        if self.punch_failures >= 6 { return false }
        self.punched_on_this_network || self.public_ip.as_ref().is_some_and(|ip| self.remembered_carriers().contains(&self.carrier_key(ip)))
    }

    fn start_punch(&mut self, config: Config) {
        let gentle = self.punch_failures > 0 && !self.punchable();
        let interface = NETWORK.lock().map(|n| n.1.clone()).unwrap_or_default();
        let plan = punch::Plan {
            stun: config.stun(), stun2: config.stun2.clone(), signal: format!("http://127.0.0.1:{}", self.relay_port), token: config.token.clone(),
            interface: (!interface.is_empty()).then_some(interface),
            random_from: self.window_failed_from.iter().cloned().collect(),
            sockets: if gentle { 96 } else { 192 },
            interval: Duration::from_millis(if gentle { 400 } else { 200 }),
        };
        note(&format!("punching a direct path{}", if gentle { " (gently: this network has not punched yet)" } else { "" }));
        self.state("relay; punching a direct path");
        if let Ok(mut s) = self.shared.status.lock() { s.punching = true }
        let (epoch, at, shared) = (self.epoch.clone(), self.epoch.load(Ordering::SeqCst), self.shared.clone());
        let current = move || epoch.load(Ordering::SeqCst) == at && !shared.stopped.load(Ordering::SeqCst);
        self.punch = Some(tokio::task::spawn_blocking(move || {
            let punched = match punch::punch(&plan, |r| note(&format!("direct: {}", r.step))) {
                Ok(punched) => punched,
                Err(failure) => return if current() { Outcome::Failed(failure) } else { Outcome::Stale },
            };
            if !current() { return Outcome::Stale }
            crate::stop(&TUNNEL);
            let started = Instant::now();
            // Keepalives every 5 s and a 15 s idle limit, as in the iOS extension (this process
            // is not suspended either): a dead path fails over to the relay quickly.
            let port = crate::open_on(&TUNNEL, punched.socket, punched.server, punched.fingerprint, b"tbdirect", "torrbaby-direct", 8, crate::LEAN_TIMING, crate::APP_WINDOWS);
            if port == 0 {
                return Outcome::Failed(punch::Failure { message: "Punched, but QUIC did not open on the punched socket within 8 s".into(), session: None, window_failed_at: None })
            }
            if !current() { crate::stop(&TUNNEL); return Outcome::Stale }
            Outcome::Opened { summary: punched.summary, public_address: punched.public_address, quic_ms: started.elapsed().as_millis() as u64 }
        }));
    }

    async fn finish_punch(&mut self) {
        if !self.punch.as_ref().is_some_and(|p| p.is_finished()) { return }
        let Some(handle) = self.punch.take() else { return };
        if let Ok(mut s) = self.shared.status.lock() { s.punching = false }
        match handle.await {
            Ok(Outcome::Opened { summary, public_address, quic_ms }) => {
                self.punched_from = Some(public_address.clone());
                self.punch_failures = 0;
                self.next_punch = Instant::now();
                self.punched_on_this_network = true;
                self.remember_carrier(&public_address);
                if let Ok(mut s) = self.shared.status.lock() { s.punched = summary.clone(); s.last_punch = format!("opened: {summary}"); s.next_punch = None }
                self.state("direct path open");
                note(&format!("direct open: {summary}, QUIC {quic_ms} ms"));
            }
            Ok(Outcome::Failed(failure)) => {
                if let Some(address) = failure.window_failed_at { self.window_failed_from.insert(address); }
                self.punch_failures += 1;
                let punchable = self.punchable();
                let waits = if punchable { [5, 15, 30, 60] } else { [60, 120, 300, 900] };
                let wait = waits[self.punch_failures.min(4) - 1];
                self.next_punch = Instant::now() + Duration::from_secs(wait);
                if let Ok(mut s) = self.shared.status.lock() { s.last_punch = format!("failed: {}", failure.message); s.next_punch = Some(self.next_punch) }
                self.state("relay; waiting to punch again");
                note(&format!("direct failed: {}", failure.message));
                note(&format!("next punch in {} (failure {} on this network, {})", if wait < 60 { format!("{wait} s") } else { format!("{} min", wait / 60) },
                    self.punch_failures, if punchable { "which punches: bronze only bridges" } else { "which has not punched yet" }));
            }
            Ok(Outcome::Stale) => note("punch result not used: the network changed meanwhile"),
            Err(_) => { self.punch_failures += 1; self.next_punch = Instant::now() + Duration::from_secs(60); note("the punch stopped unexpectedly") }
        }
    }
}

/// Carriers that punched, while there is no state directory to keep them in.
static REMEMBERED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// bronze's view over the relay: this device's public IP and Diamond's (when connected).
async fn info() -> Option<(String, Option<String>)> {
    let reply = tokio::task::spawn_blocking(crate::relay_info).await.ok()??;
    let text = String::from_utf8_lossy(&reply);
    let (mut ip, mut home) = (None, None);
    for line in text.lines() {
        if let Some(you) = line.strip_prefix("you ") { ip = you.trim().rsplit_once(':').map(|(host, _)| host.to_owned()) }
        if let Some(home_ip) = line.strip_prefix("homeip ") { home = Some(home_ip.trim().to_owned()) }
    }
    ip.map(|ip| (ip, home))
}
