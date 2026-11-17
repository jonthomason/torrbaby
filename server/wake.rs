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
}

/// The library's files or titles changed: what is prepared, matched and shown follows.
pub fn library_changed() {
    PROCESSING.wake();
    FORMATS.wake();
    METADATA.wake();
    ARTWORK.wake();
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
