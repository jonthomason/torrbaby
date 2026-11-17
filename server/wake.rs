//! Background work runs when something asks for it, not on a clock. Each worker waits on its
//! signal, woken by what makes its work necessary (a file finished, a torrent added, a request
//! that changed something), or until the one moment time itself matters, a deadline the worker
//! worked out from its data (a retry that is due, a playback heartbeat going stale). The user's
//! goal (2026-11-17): no timers.
use std::time::{Duration, Instant};

pub struct Signal(tokio::sync::Notify);
impl Signal {
    pub const fn new() -> Self { Self(tokio::sync::Notify::const_new()) }
    /// Run the worker now (or as soon as its current pass ends: a wake is kept until taken).
    pub fn wake(&self) { self.0.notify_one(); }
    /// Until woken, or until `deadline` when there is one.
    pub async fn wait(&self, deadline: Option<Instant>) {
        match deadline {
            None => self.0.notified().await,
            Some(at) => tokio::select! { () = self.0.notified() => {}, () = tokio::time::sleep_until(at.into()) => {} },
        }
    }
}

pub static PROCESSING: Signal = Signal::new();
pub static ORGANIZER: Signal = Signal::new();
pub static SAMPLER: Signal = Signal::new();
pub static METADATA: Signal = Signal::new();
pub static ARTWORK: Signal = Signal::new();
pub static REMOVALS: Signal = Signal::new();
pub static TRIALS: Signal = Signal::new();
pub static MOVES: Signal = Signal::new();
/// Watched shows: the library or an episode list changed, so the next check is worked out again.
pub static WATCHER: Signal = Signal::new();
/// The names devices reach the server by may have changed (network settings).
pub static ROUTES: Signal = Signal::new();
/// Cloud downloads: a drive came back, a torrent finished seeding.
pub static CLOUD: Signal = Signal::new();
/// The library's folders may have changed (a drive, the drive settings).
pub static ROOTS: Signal = Signal::new();
/// Describing files' formats and listing waiting files' languages.
pub static FORMATS: Signal = Signal::new();

/// After a request that changed something (settings, a priority, playback, a match, a removal,
/// a trial, a move): the workers whose check is one query look again. The organizer (a pass over
/// every torrent) only for torrents and settings, not for each playback heartbeat.
pub fn after_request(path: &str) {
    PROCESSING.wake();
    REMOVALS.wake();
    TRIALS.wake();
    MOVES.wake();
    if ["/v1/torrents", "/v1/settings", "/v1/drives", "/v1/cloud", "/v1/library"].iter().any(|p| path.starts_with(p)) {
        ORGANIZER.wake();
        SAMPLER.wake();
    }
    if path.starts_with("/v1/library") || path.starts_with("/v1/metadata") { METADATA.wake(); }
    if path.starts_with("/v1/drives") || path.starts_with("/v1/settings") { ROOTS.wake(); }
}

/// The library's files or titles changed: what is prepared, matched and shown follows.
pub fn library_changed() {
    WATCHER.wake();
    PROCESSING.wake();
    FORMATS.wake();
    METADATA.wake();
    ARTWORK.wake();
}

/// A change counter several waiters can watch at once: a job that must give way when playback
/// preparation starts, or when its slot is taken back, waits on this instead of looking every
/// fraction of a second.
pub struct Changes(tokio::sync::watch::Sender<u64>);
impl Changes {
    fn new() -> Self { Self(tokio::sync::watch::Sender::new(0)) }
    pub fn bump(&self) { self.0.send_modify(|n| *n = n.wrapping_add(1)); }
    /// The first wait returns at once, so the waiter looks at the state as it is now.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> { let mut r = self.0.subscribe(); r.mark_changed(); r }
}
/// Playback preparation started or ended (virtual media's pending builds).
pub static PREPARATION: std::sync::LazyLock<Changes> = std::sync::LazyLock::new(Changes::new);
/// Processing slots: their number changed, one was freed, or a file was told to give its slot back.
pub static SLOTS: std::sync::LazyLock<Changes> = std::sync::LazyLock::new(Changes::new);

/// When someone last looked at processing (the Processing page): progress of a running helper
/// is read while they do.
static PROGRESS_VIEWED: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
pub fn progress_viewed() { if let Ok(mut v) = PROGRESS_VIEWED.lock() { *v = Some(Instant::now()); } }
pub fn progress_watched() -> bool { PROGRESS_VIEWED.lock().ok().and_then(|v| *v).is_some_and(|at| at.elapsed() < Duration::from_secs(60)) }

/// Wait for a helper process to exit, killing it if it runs past `limit`. None when it was killed
/// for time. One wait, no polling (each helper looked every 10-250 ms whether it had finished).
pub fn wait_child(child: &mut std::process::Child, limit: Duration) -> std::io::Result<Option<std::process::ExitStatus>> {
    let (done, finished) = std::sync::mpsc::channel::<()>();
    let pid = child.id() as libc::pid_t;
    let killed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let marker = killed.clone();
    let guard = std::thread::spawn(move || {
        if finished.recv_timeout(limit) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
            marker.store(true, std::sync::atomic::Ordering::SeqCst);
            unsafe { libc::kill(pid, libc::SIGKILL); }
        }
    });
    let status = child.wait();
    let _ = done.send(());
    let _ = guard.join();
    let status = status?;
    Ok((!killed.load(std::sync::atomic::Ordering::SeqCst)).then_some(status))
}

/// A deadline `seconds` from now (epoch seconds as stored are converted by the caller).
pub fn after(seconds: f64) -> Instant { Instant::now() + Duration::from_secs_f64(seconds.clamp(0.0, 86400.0 * 7.0)) }
/// An epoch time stored in the database, as a deadline.
pub fn at_epoch(epoch: f64) -> Instant { after(epoch - crate::storage::now()) }

/// Wake `signal` when files appear in `dir` (FSEvents). The watcher lives as long as the result.
pub fn watch_dir(dir: &std::path::Path, signal: &'static Signal) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher;
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok_and(|e| !matches!(e.kind, notify::EventKind::Access(_))) { signal.wake(); }
    }).map_err(|error| tracing::warn!(%error, dir = %dir.display(), "Folder watch could not start")).ok()?;
    watcher.watch(dir, notify::RecursiveMode::NonRecursive).map_err(|error| tracing::warn!(%error, dir = %dir.display(), "Folder not watched")).ok()?;
    Some(watcher)
}
