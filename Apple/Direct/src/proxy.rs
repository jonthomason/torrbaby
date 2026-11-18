//! Torrbaby's link to Diamond away from home, for every client (the iPhone/iPad app and its
//! packet tunnel, the Android TV app): one stable loopback port, and in-process connections for
//! the packet tunnel, each going to Diamond by the best path that works now. The base URL
//! (http://127.0.0.1:<port>) stays the same whatever happens beneath it.
//!
//! The path is ABT's shared abt-link (the user, 2026-11-17: ABT's code, used by Torrbaby when
//! ABT is not installed; "No legacy code"): its route takes Diamond on this network (`home`),
//! else the direct path once it has proved healthy (checked every 2 s, dropped after two
//! missed checks, so the connections on it end and are asked again over the relay), else the
//! relay through bronze, end to end encrypted to Diamond's identity; connections on the relay
//! move to the direct path once it has been chosen for 2 s. Punches come from abt-link's
//! policy (one at a time, waits after failures, random ports at once where predicted ones
//! failed, bronze's shared hints, at most 2 minutes between tries while relayed).
//!
//! What stays Torrbaby's (`Keeper`, every 3 s):
//! - the relay through bronze (relay.rs: QUIC, or TCP 443 where UDP is blocked) kept open,
//!   and let go after `idle` with nothing carried (no keepalives) until the next connection,
//!   which opens it again within about a second;
//! - a network change (told by the app, `network`) that holds for 3 s: the relay moves to the
//!   new network and the route drops its direct path and asks for a punch;
//! - every 6 s, bronze's view (TBINFO): behind Diamond's own address no punch is tried (it
//!   could only hairpin); a new public address under the direct path (a carrier switch behind
//!   a hotspot) drops it.
use crate::relay::{Bronze, BronzeRelay};
use abt_link::device::policy::{Advice, Attempt, Policy, Reporter};
use abt_link::bronze::{Relay as Rendezvous, Seen, hints_from, open_signal, ours, watch_home};
use abt_link::device::{Failure, HttpSignal, Opened, Plan, Signal};
use abt_link::route::{Route, RouteConfig, State};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// How the link reaches bronze and Diamond.
#[derive(Clone, PartialEq)]
pub struct Config {
    /// bronze's relay ("ip:port") and its certificate fingerprint.
    pub relay: String,
    pub relay_fingerprint: String,
    /// This device's token for Diamond, which punching asks with (POST /v1/direct). Empty:
    /// relay only (a TV claiming its setup has no token yet).
    pub token: String,
    /// Diamond's identity (SHA-256 of its certificate): relayed streams are TLS to it end to
    /// end, and the direct path must present it. None only while pairing through the relay.
    pub pinned: Option<String>,
    /// Where the punch memory is kept across restarts; none: in memory.
    pub state_dir: Option<PathBuf>,
    /// Nothing carried this long: the relay is let go until the next connection.
    pub idle: Duration,
    /// How long `start` waits for the relay to open before giving up.
    pub open_wait: Duration,
    /// The packet tunnel extension (50 MB in all): smaller buffers.
    pub lean: bool,
}

impl Config {
    pub fn new(relay: &str, relay_fingerprint: &str) -> Config {
        Config { relay: relay.into(), relay_fingerprint: relay_fingerprint.trim().to_lowercase(),
            token: String::new(), pinned: crate::pinned(), state_dir: None, idle: Duration::from_secs(120), open_wait: Duration::from_secs(7), lean: false }
    }
}

/// What the keeper knows, for the status and the punches.
#[derive(Default)]
struct Status {
    state: String,
    public_ip: String,
    home_ip: String,
    behind_diamonds_nat: bool,
}

struct Shared {
    config: Mutex<Config>,
    status: Mutex<Status>,
    stopped: AtomicBool,
    /// Counts network changes: a punch's retry is for the network it began on.
    epoch: AtomicU64,
}

struct Link {
    runtime: tokio::runtime::Runtime,
    route: Arc<Route>,
    bronze: Arc<Bronze>,
    policy: Arc<Policy>,
    shared: Arc<Shared>,
    /// Where Diamond is seen: one entry per rendezvous (bronze, Shadow, miyu), so the app tells
    /// "Diamond is up but there is no path" from "Diamond is not seen anywhere".
    seen: tokio::sync::watch::Receiver<Vec<Seen>>,
}

/// Our rendezvous servers (abt-link's `ours`): bronze, then the backups, which carry only what
/// sets up a direct path. Punch requests go through whichever answers first.
fn rendezvous() -> Vec<Arc<Rendezvous>> { ours().into_iter().map(|r| Rendezvous::new(r.bronze)).collect() }

static LINK: Mutex<Option<Link>> = Mutex::new(None);
/// The network the app says it is on: a signature that changes with the network, and the
/// interface's name. Kept whether or not the link runs.
static NETWORK: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));
static NOTES: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
static LOGGER: Mutex<Option<fn(&str)>> = Mutex::new(None);

/// Where each line of what the link does goes (Android: logcat; iOS: the extension's log),
/// besides `notes`.
pub fn set_logger(logger: fn(&str)) { if let Ok(mut held) = LOGGER.lock() { *held = Some(logger) } }

fn note(line: &str) {
    if let Ok(mut notes) = NOTES.lock() {
        notes.push_back(line.to_owned());
        while notes.len() > 64 { notes.pop_front(); }
    }
    let logger = LOGGER.lock().ok().and_then(|held| *held);
    if let Some(logger) = logger { logger(line) }
}

/// The link's recent lines, oldest first.
pub fn notes() -> Vec<String> { NOTES.lock().map(|n| n.iter().cloned().collect()).unwrap_or_default() }

/// Start the link (or, already running for this relay and identity, take the new token and
/// STUN) and return its loopback port (bronze's relay may not have opened: then direct paths only).
/// Blocks until then: call it off the main thread, and not from inside an async task.
pub fn start(config: Config) -> u16 {
    if let Ok(held) = LINK.lock() && let Some(link) = held.as_ref() {
        let mut current = link.shared.config.lock().unwrap_or_else(|e| e.into_inner());
        if current.relay == config.relay && current.relay_fingerprint == config.relay_fingerprint && current.pinned == config.pinned && current.lean == config.lean {
            *current = config;
            return link.route.port();
        }
    }
    stop();
    let Ok(relay) = config.relay.parse::<SocketAddr>() else { note(&format!("link: bronze's address is not valid: {}", config.relay)); return 0 };
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("tblink").enable_all().build() else { return 0 };
    let bronze = Arc::new(Bronze::new(relay, &config.relay_fingerprint, config.lean));
    bronze.set_interface(&NETWORK.lock().map(|n| n.1.clone()).unwrap_or_default());
    let route_config = RouteConfig { host: "diamond".into(), pinned: config.pinned.clone(), ..RouteConfig::default() };
    let Ok(route) = runtime.block_on(Route::start(route_config, Arc::new(BronzeRelay(bronze.clone())), None)) else { runtime.shutdown_background(); return 0 };
    let policy = Policy::new();
    if let Some(dir) = &config.state_dir { policy.keep_memory(dir.join("punch-memory.json")) }
    let open_wait = config.open_wait;
    let shared = Arc::new(Shared { config: Mutex::new(config), status: Mutex::new(Status { state: "starting".into(), ..Status::default() }),
        stopped: AtomicBool::new(false), epoch: AtomicU64::new(0) });
    let port = route.port();
    note(&format!("link: loopback port {port}, relay {relay}"));
    // The relay first. Without it (bronze down) the link still runs: punches are asked of
    // Diamond through the backup rendezvous servers, and only a direct path carries anything.
    let opened = runtime.block_on(async { tokio::time::timeout(open_wait, bronze.open()).await.unwrap_or(false) });
    if opened { note(&format!("relay open{}", if bronze.over_tcp() { " over TCP 443 (UDP to bronze did not get through)" } else { "" })) }
    else { note(&format!("link: the relay at {relay} did not open: direct paths only, asked through the backup rendezvous")) }
    let seen = { let _inside = runtime.enter(); watch_home(rendezvous(), "diamond".into(), Duration::from_secs(20)) };
    {
        let _inside = runtime.enter();
        let attempt = Arc::new(Punch { shared: shared.clone(), route: Arc::downgrade(&route) });
        route.punch_with(policy.clone(), attempt, Arc::new(Hints(rendezvous())));
    }
    runtime.spawn(Keeper { shared: shared.clone(), route: route.clone(), bronze: bronze.clone(), last_info: None, network: String::new(),
        pending: None, last_bytes: 0, last_busy: Instant::now(), punched_from: None }.run());
    if let Ok(mut held) = LINK.lock() { *held = Some(Link { runtime, route, bronze, policy, shared, seen }) }
    port
}

/// Whether the link is running.
pub fn running() -> bool { LINK.lock().ok().is_some_and(|held| held.is_some()) }

/// The loopback port, while running.
pub fn port() -> Option<u16> { LINK.lock().ok().and_then(|held| held.as_ref().map(|l| l.route.port())) }

/// Stop the link: the route, its direct path and the relay.
pub fn stop() {
    let link = LINK.lock().ok().and_then(|mut held| held.take());
    if let Some(link) = link {
        link.shared.stopped.store(true, Ordering::SeqCst);
        link.route.stop();
        link.runtime.shutdown_background();
        note("link: stopped");
    }
}

/// A connection to Diamond for the packet tunnel, in process (no loopback hop): by the path
/// the route chooses. None while the link is not running or no path answers.
pub async fn connect() -> Option<tokio::io::DuplexStream> {
    let route = LINK.lock().ok().and_then(|held| held.as_ref().map(|l| l.route.clone()))?;
    route.connect(None).await.ok().map(|(stream, _)| stream)
}

/// Diamond's address on this network while the device is at home (the packet tunnel finds it):
/// connections go straight to it. None: away.
pub fn home(address: Option<SocketAddr>) {
    let route = LINK.lock().ok().and_then(|held| held.as_ref().map(|l| l.route.clone()));
    if let Some(route) = route { route.home(address) }
}

/// The network changed (or was first seen): `signature` identifies it (interface, addresses,
/// gateway), `interface` is its name ("en0", "pdp_ip0", "wlan0"). A change that holds for 3 s
/// moves the relay and drops the direct path.
pub fn network(signature: &str, interface: &str) {
    let changed = NETWORK.lock().map(|mut held| {
        let changed = held.0 != signature;
        *held = (signature.to_owned(), interface.to_owned());
        changed
    }).unwrap_or(false);
    if changed { note(&format!("network: {signature}")) }
}

/// The app is back from the background (iOS suspended it, so the direct path's keepalives
/// stopped): the route drops its direct path and asks for a punch.
pub fn resumed() {
    let route = LINK.lock().ok().and_then(|held| held.as_ref().map(|l| l.route.clone()));
    if let Some(route) = route { route.network_changed() }
}

/// The link now, as JSON for the app's Connection page: {"state": "home" | "direct" | "relay" |
/// "down" | "stopped", "rtt_ms", "last_answered_s", "relay": "quic" | "tcp" | "closed" |
/// "dormant", "public_ip", "behind_diamonds_nat", "punching", "last_punch": {"ok", "summary",
/// "at", "next_try"}, "note", "bytes": {"home", "direct", "relay"}, "notes"}.
pub fn state() -> serde_json::Value {
    use serde_json::json;
    let held = LINK.lock().ok().and_then(|held| held.as_ref().map(|l| (l.route.clone(), l.bronze.clone(), l.policy.clone(), l.shared.clone(), l.runtime.handle().clone(), l.seen.borrow().clone())));
    let Some((route, bronze, policy, shared, handle, seen)) = held else { return json!({"state": "stopped", "notes": notes()}) };
    let (state, rtt, answered) = match route.state() {
        State::Home => ("home", None, None),
        State::Direct { rtt_ms, last_answered, .. } => ("direct", Some(rtt_ms), Some(last_answered.elapsed().as_secs())),
        State::Relay => ("relay", None, None),
        State::Down => ("down", None, None),
    };
    let relay_alive = handle.block_on(bronze.alive());
    let relay = if bronze.dormant() { "dormant" } else if !relay_alive { "closed" } else if bronze.over_tcp() { "tcp" } else { "quic" };
    let stats = route.stats();
    let bytes = |p: abt_link::route::PathBytes| json!({"sent": p.sent, "received": p.received});
    let status = shared.status.lock().unwrap_or_else(|e| e.into_inner());
    json!({
        "state": state, "rtt_ms": rtt, "last_answered_s": answered, "relay": relay, "relay_line": handle.block_on(bronze.line()),
        "public_ip": status.public_ip, "home_ip": status.home_ip, "behind_diamonds_nat": status.behind_diamonds_nat, "keeper": status.state,
        "punching": policy.punching("diamond"), "note": policy.note(),
        "last_punch": policy.last("diamond").map(|l| json!({"ok": l.ok, "summary": l.summary, "at": l.at, "next_try": l.next_try})),
        "bytes": {"home": bytes(stats.home), "direct": bytes(stats.direct), "relay": bytes(stats.relay)},
        // Diamond's standing connection at each rendezvous: online true/false, or null when that
        // server did not answer.
        "diamond_seen": seen.iter().any(|s| s.online == Some(true)),
        "seen": seen.iter().map(|s| json!({"server": s.server, "online": s.online, "last_online": s.last_online})).collect::<Vec<_>>(),
        "notes": notes(),
    })
}

/// One line for the log.
pub fn status() -> String {
    let state = state();
    let mut line = format!("{}, relay {}", state["state"].as_str().unwrap_or("?"), state["relay"].as_str().unwrap_or("?"));
    if let Some(rtt) = state["rtt_ms"].as_u64() { line.push_str(&format!(" (direct rtt {rtt} ms)")) }
    if let Some(punching) = state["punching"].as_str() { line.push_str(&format!("; punching: {punching}")) }
    if let Some(summary) = state["last_punch"]["summary"].as_str() { line.push_str(&format!("; last punch {summary}")) }
    if let Some(keeper) = state["keeper"].as_str().filter(|k| !k.is_empty()) { line.push_str(&format!("; {keeper}")) }
    line
}

/// One punch toward Diamond, for the route: the opened QUIC becomes its direct path.
struct Punch {
    shared: Arc<Shared>,
    route: Weak<Route>,
}

impl Attempt for Punch {
    type Path = Opened;

    async fn attempt(&self, host: &str, advice: Advice, report: Reporter) -> Result<(Opened, String), Failure> {
        let config = self.shared.config.lock().map(|c| c.clone()).map_err(|_| Failure::new("the link stopped"))?;
        if config.token.is_empty() { return Err(Failure::new("relay only: no device token to ask Diamond for a direct path")) }
        if self.shared.status.lock().is_ok_and(|s| s.behind_diamonds_nat) {
            return Err(Failure::new("same public address as Diamond: a punch could only hairpin, so the relay carries everything here"));
        }
        let route = self.route.upgrade().ok_or_else(|| Failure::new("the link stopped"))?;
        let interface = NETWORK.lock().map(|n| n.1.clone()).unwrap_or_default();
        // Every rendezvous's STUN at once (bronze, Shadow, miyu): the first answer is used.
        let stun: Vec<String> = ours().into_iter().map(|r| r.stun).collect();
        let mut plan = Plan::from_json(&serde_json::json!({
            "stun": stun, "interface": interface, "lean": config.lean, "pinned": config.pinned,
        }));
        plan.random_from = advice.random_from;
        plan.pool = advice.pool;
        // No punch from this network on record: Diamond also sprays random ports.
        plan.known = advice.known;
        // Diamond is asked through the route itself while it has a path (the relay, or an old
        // direct path); else through whichever rendezvous answers first, TLS to Diamond end to end.
        let signal = Signals {
            route: HttpSignal { base: format!("http://127.0.0.1:{}", route.port()), token: config.token.clone() },
            through_route: !matches!(route.state(), State::Down),
            servers: rendezvous(), pinned: config.pinned.clone(), token: config.token.clone(), handle: tokio::runtime::Handle::current(),
        };
        let named = host.to_string();
        let opened = tokio::task::spawn_blocking(move || abt_link::device::punch_to(&named, &plan, &signal, |r| report(r)))
            .await
            .map_err(|_| Failure::new("the punch stopped unexpectedly"))?
            .map_err(|failure| Failure { message: failure.explained(), ..failure })?;
        let summary = format!("{}, QUIC in {} ms", opened.summary, opened.quic_ms);
        note(&format!("direct path open: {summary}"));
        Ok((opened, summary))
    }

    fn opened(&self, _host: &str, _path: Opened) {}

    fn retry_check(&self, _host: &str) -> impl Fn() -> bool + Send + 'static {
        let (shared, route, epoch) = (self.shared.clone(), self.route.clone(), self.shared.epoch.load(Ordering::SeqCst));
        move || !shared.stopped.load(Ordering::SeqCst) && shared.epoch.load(Ordering::SeqCst) == epoch && route.upgrade().is_some_and(|r| !r.straight())
    }

    fn relayed(&self, _host: &str) -> bool {
        self.route.upgrade().is_some_and(|r| matches!(r.state(), State::Relay))
    }
}

/// What every rendezvous's shared store says about punching from this network, merged.
struct Hints(Vec<Arc<Rendezvous>>);
impl abt_link::device::policy::Hints for Hints {
    async fn hints(&self) -> Option<serde_json::Value> { hints_from(&self.0).await }
}

/// How a punch request reaches Diamond: through the route while it has a path, else (or when
/// that fails) through the rendezvous servers, each stream TLS to Diamond's identity end to end
/// (the backups carry it without reading it: the request holds this device's token).
struct Signals {
    route: HttpSignal,
    through_route: bool,
    servers: Vec<Arc<Rendezvous>>,
    pinned: Option<String>,
    token: String,
    handle: tokio::runtime::Handle,
}

impl Signal for Signals {
    fn ask(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value, String> {
        if self.through_route {
            match self.route.ask(path, body) {
                Ok(answer) => return Ok(answer),
                Err(error) => note(&format!("punch request through the route failed ({error}): asking through the rendezvous")),
            }
        }
        let pinned = self.pinned.clone().ok_or("Diamond's identity is not known: no punch request through a rendezvous")?;
        self.handle.block_on(async {
            let (server, stream) = open_signal(&self.servers, "diamond").await.map_err(|e| format!("{e:#}"))?;
            let stream = abt_link::secure::e2e(stream, Some(&pinned)).await.map_err(|e| format!("TLS to Diamond through {server}: {e}"))?;
            let answer = abt_link::http::post_on(stream, &self.token, path, body).await;
            note(&format!("punch request through {server}: {}", if answer.is_ok() { "answered" } else { "failed" }));
            answer
        })
    }
}

/// The relay, network changes and bronze's view, every 3 s.
struct Keeper {
    shared: Arc<Shared>,
    route: Arc<Route>,
    bronze: Arc<Bronze>,
    last_info: Option<Instant>,
    /// The network acted on, and a different one waiting to hold for 3 s.
    network: String,
    pending: Option<(String, Instant)>,
    /// Bytes carried on any path at the last look, and when they last grew.
    last_bytes: u64,
    last_busy: Instant,
    /// The public address the direct path was punched from.
    punched_from: Option<String>,
}

impl Keeper {
    fn state(&self, text: &str) { if let Ok(mut s) = self.shared.status.lock() { s.state = text.into() } }

    async fn run(mut self) {
        while !self.shared.stopped.load(Ordering::SeqCst) {
            self.watch_network().await;
            self.maintain().await;
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }

    async fn watch_network(&mut self) {
        let (signature, interface) = NETWORK.lock().map(|n| n.clone()).unwrap_or_default();
        if signature.is_empty() { return }
        if self.network.is_empty() { self.network = signature; return }
        if signature == self.network { self.pending = None; return }
        match &self.pending {
            Some((waiting, since)) if *waiting == signature && since.elapsed() >= Duration::from_secs(3) => {}
            Some((waiting, _)) if *waiting == signature => return,
            _ => { self.pending = Some((signature, Instant::now())); return }
        }
        // A different network has held for 3 s: the direct path's hole was for the old one.
        self.shared.epoch.fetch_add(1, Ordering::SeqCst);
        self.bronze.set_interface(&interface);
        let moved = !self.bronze.dormant() && self.bronze.rebind().await;
        if !moved { self.bronze.close(self.bronze.dormant()).await }
        self.route.network_changed();
        note(&format!("network changed ({} → {signature}): direct path dropped, relay {}", self.network,
            if moved { "moved to the new network" } else if self.bronze.dormant() { "stays closed (dormant)" } else { "reopening" }));
        self.network = signature;
        self.pending = None;
        self.punched_from = None;
        self.last_info = None;
        if let Ok(mut s) = self.shared.status.lock() { s.public_ip.clear(); s.behind_diamonds_nat = false }
    }

    async fn maintain(&mut self) {
        let stats = self.route.stats();
        let bytes = stats.home.sent + stats.home.received + stats.direct.sent + stats.direct.received + stats.relay.sent + stats.relay.received;
        if bytes != self.last_bytes { self.last_bytes = bytes; self.last_busy = Instant::now() }
        let idle = self.shared.config.lock().map(|c| c.idle).unwrap_or(Duration::from_secs(120));
        if self.bronze.dormant() {
            // The next connection opens the relay again itself (relay.rs).
            if self.last_busy.elapsed() < idle { note("a connection woke the link"); }
            else { return }
        } else if self.last_busy.elapsed() > idle && !matches!(self.route.state(), State::Home) {
            // Battery and bronze: with nothing carried, the relay's keepalives are not worth it.
            self.bronze.close(true).await;
            self.state("dormant: idle, nothing sent until the next connection");
            note(&format!("idle for {} s: relay let go, no keepalives, until the next connection", idle.as_secs()));
            return
        }
        if !self.bronze.alive().await {
            note("relay: connecting");
            if self.bronze.open().await { self.state("relay open"); note("relay open") }
            else { self.state("relay: bronze did not answer"); return }
        }
        // Every 6 s, bronze's view of this device and of Diamond.
        if self.last_info.is_some_and(|at| at.elapsed() < Duration::from_secs(6)) { return }
        self.last_info = Some(Instant::now());
        let Some(reply) = self.bronze.ask(b"TBINFO1\n").await else { return };
        let text = String::from_utf8_lossy(&reply).to_string();
        let field = |name: &str| text.lines().find_map(|l| l.strip_prefix(name)).map(str::to_owned);
        let ip = field("you ").map(|you| you.rsplit_once(':').map(|(ip, _)| ip.trim_matches(['[', ']']).to_owned()).unwrap_or(you)).unwrap_or_default();
        let home_ip = field("homeip ").unwrap_or_default();
        let behind = !ip.is_empty() && ip == home_ip;
        let was_behind = self.shared.status.lock().map(|s| s.behind_diamonds_nat).unwrap_or(false);
        if let Ok(mut s) = self.shared.status.lock() { s.public_ip = ip.clone(); s.home_ip = home_ip; s.behind_diamonds_nat = behind }
        if behind && !was_behind { note(&format!("same public address as Diamond ({ip}): relay only, no punching")); self.state("behind Diamond's own network address: relay only") }
        if !behind && was_behind { note(&format!("public address {ip} is no longer Diamond's: direct paths may be punched")); self.route.network_changed() }
        // A new public address under the direct path (a carrier switch behind a hotspot, which
        // the device cannot see): its hole is for the old one.
        match (&self.punched_from, self.route.state()) {
            (Some(from), State::Direct { .. }) if *from != ip && !ip.is_empty() => {
                note(&format!("public address changed ({from} → {ip}): direct path dropped"));
                self.punched_from = None;
                self.route.network_changed();
            }
            (None, State::Direct { .. }) => self.punched_from = Some(ip),
            (_, State::Direct { .. }) => {}
            _ => self.punched_from = None,
        }
    }
}
