//! The drives Torrbaby keeps media on (docs/disk-aggregation.md). Each drive has its own
//! Downloads, Movies and TV roots. This is the single place that answers drive questions:
//! which roots exist, which drive a path is on, whether a path is managed, and how much
//! room each volume has. Callers ask here rather than reading the configured folders or
//! statting paths themselves, so spanning several drives changes this module, not them.
use crate::config::Config;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// A library root's role on its drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role { Downloads, Movies, Tv }

impl Role {
    pub const ALL: [Role; 3] = [Role::Downloads, Role::Movies, Role::Tv];
    pub fn name(self) -> &'static str {
        match self { Role::Downloads => "downloads", Role::Movies => "movies", Role::Tv => "tv" }
    }
}

/// One drive's library roots, resolved from the configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Drive {
    /// Stable identity, never derived from where the drive is mounted.
    pub id: String,
    /// What administrators call it: the configured name, else the volume's name.
    pub name: String,
    /// The folder holding the drive's marker (its `Media` folder).
    pub media: PathBuf,
    pub downloads: PathBuf,
    pub movies: PathBuf,
    pub tv: PathBuf,
    /// Whether new downloads may be placed here.
    pub accept_new: bool,
    /// Free space never to place below.
    pub reserve_bytes: u64,
}

impl Drive {
    pub fn root(&self, role: Role) -> &Path {
        match role { Role::Downloads => &self.downloads, Role::Movies => &self.movies, Role::Tv => &self.tv }
    }
    pub fn roots(&self) -> [(Role, &Path); 3] { Role::ALL.map(|role| (role, self.root(role))) }
    /// The finished-library root for a film or a series on this drive.
    pub fn library(&self, tv: bool) -> &Path { if tv { &self.tv } else { &self.movies } }

    /// Whether this drive is connected: its marker is there and names it. A configured drive
    /// whose folders are present on their own volume but carry no marker yet (Alpha, before
    /// drives existed) is given one. Folders that are present without a marker on a path
    /// whose volume is not mounted are never adopted: they are on the startup disk.
    pub fn status(&self) -> Status {
        let marker = self.media.join(MARKER);
        let status = match std::fs::read(&marker) {
            Ok(raw) => match serde_json::from_slice::<Value>(&raw).ok().and_then(|v| v["id"].as_str().map(str::to_owned)) {
                Some(id) if id == self.id => Status::Online,
                Some(id) => Status::Foreign(id),
                None => Status::Foreign(String::new()),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && self.adoptable() => match write_marker(&self.media, &self.id) {
                Ok(()) => { tracing::info!(drive = self.id, media = %self.media.display(), "Marked a configured drive"); Status::Online }
                Err(error) => { tracing::warn!(drive = self.id, %error, "Could not mark a configured drive"); Status::Missing }
            },
            Err(_) => Status::Missing,
        };
        if status == Status::Online {
            // The marker proves this is the drive, so its folders can safely be created here.
            for (_, root) in self.roots() { if !root.is_dir() { let _ = std::fs::create_dir_all(root); } }
            if let Ok(mut dismissed) = DISMISSED.lock() { dismissed.remove(&self.id); }
        }
        status
    }
    pub fn online(&self) -> bool { self.status() == Status::Online }
    fn adoptable(&self) -> bool {
        self.media.is_dir() && self.roots().iter().any(|(_, root)| root.is_dir()) && on_its_volume(&self.media)
    }
}

/// Whether a drive is connected. Never a reason to treat its files as deleted.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Online,
    /// Not mounted, or its folders are gone.
    Missing,
    /// Another drive (with this id, or an unreadable marker) is where this one belongs.
    Foreign(String),
}

/// The file on each drive that says which drive it is.
pub const MARKER: &str = ".torrbaby-drive.json";

pub fn write_marker(media: &Path, id: &str) -> std::io::Result<()> {
    let staged = media.join(format!("{MARKER}.new"));
    std::fs::write(&staged, serde_json::to_vec_pretty(&json!({"id": id,
        "about": "Torrbaby recognises this drive by this file. Keep it; it is not needed by anything else."}))?)?;
    std::fs::rename(staged, media.join(MARKER))
}

/// A path under `/Volumes/<name>` is only on its drive while that volume is mounted;
/// otherwise the folders are on the startup disk.
fn on_its_volume(path: &Path) -> bool {
    let mut parts = path.components();
    let (Some(std::path::Component::RootDir), Some(first), Some(name)) = (parts.next(), parts.next(), parts.next()) else { return true };
    if first.as_os_str() != "Volumes" { return true; }
    let volume = Path::new("/Volumes").join(name.as_os_str());
    match (std::fs::metadata(&volume), std::fs::metadata("/Volumes")) {
        (Ok(volume), Ok(parent)) => volume.dev() != parent.dev(),
        _ => false,
    }
}

/// Missing drives whose Needs attention item has been dismissed, until they return.
static DISMISSED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::LazyLock::new(Default::default);
/// Drive items in Needs attention use ids from here up, clear of processing rows.
pub const ATTENTION_BASE: i64 = 1 << 40;

/// The level the clients already show in orange.
pub const DEFAULT_RESERVE_BYTES: u64 = 50_000_000_000;
/// The identity given to the folders configured before drives existed.
pub const LEGACY_DRIVE_ID: &str = "primary";

/// Every configured drive, in configuration order. Never empty.
#[derive(Debug, Clone)]
pub struct Drives(Vec<Drive>);

impl Drives {
    /// The configured drives; with no `drives` list, the `source`/`movies`/`tv` folders are drive 1.
    pub fn of(config: &Config) -> Self {
        if config.drives.is_empty() {
            let (downloads, movies, tv) = (PathBuf::from(&config.source), PathBuf::from(&config.movies), PathBuf::from(&config.tv));
            // The marker goes in the folder holding all three, when there is one.
            let media = match downloads.parent() {
                Some(parent) if movies.parent() == Some(parent) && tv.parent() == Some(parent) => parent.to_path_buf(),
                _ => downloads.clone(),
            };
            return Drives(vec![Drive {
                id: LEGACY_DRIVE_ID.into(), name: volume_name(&media).unwrap_or_else(|| LEGACY_DRIVE_ID.into()), media,
                downloads, movies, tv, accept_new: true, reserve_bytes: DEFAULT_RESERVE_BYTES,
            }]);
        }
        Drives(config.drives.iter().map(|d| {
            let media = d.media.as_deref().map(Path::new);
            let root = |explicit: &Option<String>, folder: &str| explicit.as_ref().map(PathBuf::from)
                .or_else(|| media.map(|m| m.join(folder))).unwrap_or_default();
            let media = media.map(Path::to_path_buf).unwrap_or_else(|| root(&d.downloads, "Downloads"));
            Drive {
                name: d.name.clone().or_else(|| volume_name(&media)).unwrap_or_else(|| d.id.clone()), media,
                id: d.id.clone(), downloads: root(&d.downloads, "Downloads"), movies: root(&d.movies, "Movies"),
                tv: root(&d.tv, "TV"), accept_new: d.accept_new, reserve_bytes: d.reserve_bytes.unwrap_or(DEFAULT_RESERVE_BYTES),
            }
        }).collect())
    }
    pub fn all(&self) -> &[Drive] { &self.0 }
    /// Where new downloads go until placement chooses among drives.
    pub fn primary(&self) -> &Drive { &self.0[0] }
    /// Every root of one role, across all drives.
    pub fn roots(&self, role: Role) -> impl Iterator<Item = &Path> { self.0.iter().map(move |d| d.root(role)) }
    /// Every root of every role, drive by drive.
    pub fn all_roots(&self) -> impl Iterator<Item = (&Drive, Role, &Path)> {
        self.0.iter().flat_map(|d| d.roots().map(|(role, root)| (d, role, root)))
    }
    /// The root `path` is inside (or is), with its drive and role.
    pub fn root_of(&self, path: &Path) -> Option<(&Drive, Role, &Path)> {
        self.all_roots().find(|(_, _, root)| path.starts_with(root))
    }
    /// Which drive `path` is on, if it is inside a managed root.
    pub fn drive_of(&self, path: &Path) -> Option<&Drive> { self.root_of(path).map(|(d, _, _)| d) }
    /// Whether `path` is inside (or is) a managed root.
    #[allow(dead_code)] // Used once every drive is scanned (phase 2).
    pub fn is_managed(&self, path: &Path) -> bool { self.root_of(path).is_some() }
    /// Whether `path` is exactly one of the managed roots.
    pub fn is_root(&self, path: &Path) -> bool { self.all_roots().any(|(_, _, root)| path == root) }
    /// The drive whose Downloads root is exactly `path`.
    pub fn downloading_into(&self, path: &Path) -> Option<&Drive> { self.0.iter().find(|d| d.downloads == path) }
    /// Whether `path` is inside a library (Movies or TV) root.
    pub fn in_library(&self, path: &Path) -> bool {
        self.root_of(path).is_some_and(|(_, role, _)| role != Role::Downloads)
    }
    /// The ids of the drives connected now.
    pub fn online(&self) -> std::collections::HashSet<String> {
        self.0.iter().filter(|d| d.online()).map(|d| d.id.clone()).collect()
    }
    /// Whether `path` is on a configured drive that is not connected now. A path on no
    /// drive at all is not offline: it is simply outside the library.
    pub fn offline(&self, path: &Path, online: &std::collections::HashSet<String>) -> bool {
        self.drive_of(path).is_some_and(|d| !online.contains(&d.id))
    }
    /// One Needs attention item per drive that is not connected, unless dismissed.
    pub fn attention(&self) -> Vec<Value> {
        let dismissed = DISMISSED.lock().map(|d| d.clone()).unwrap_or_default();
        self.0.iter().enumerate().filter(|(_, d)| !dismissed.contains(&d.id)).filter_map(|(index, d)| {
            let error = match d.status() {
                Status::Online => return None,
                Status::Missing => "Its titles return when it is connected again. Nothing has been removed.",
                Status::Foreign(_) => "Another drive is connected where this one belongs. Nothing has been removed.",
            };
            let now = crate::storage::now();
            Some(json!({"id":ATTENTION_BASE + index as i64,"asset_id":"","title":d.name,"stage":"drive","severity":"error",
                "summary":"Drive not connected","error":error,"occurrences":1,"first_seen":now,"last_seen":now}))
        }).collect()
    }
    /// Dismiss drive items by attention id, or all of them; they come back if the drive
    /// returns and goes missing again.
    pub fn dismiss(&self, ids: &[i64], all: bool) {
        let Ok(mut dismissed) = DISMISSED.lock() else { return };
        for (index, d) in self.0.iter().enumerate() {
            if (all || ids.contains(&(ATTENTION_BASE + index as i64))) && !d.online() { dismissed.insert(d.id.clone()); }
        }
    }
    /// For clients: every volume, plus the totals across them.
    pub fn report(&self) -> Value {
        let roots: Vec<(&str, &Path)> = self.all_roots().map(|(_, role, root)| (role.name(), root)).collect();
        let volumes = volumes(&roots);
        json!({
            "free_bytes": volumes.iter().map(|v| v.free_bytes).sum::<u64>(),
            "total_bytes": volumes.iter().map(|v| v.total_bytes).sum::<u64>(),
            "volumes": volumes.iter().map(|v| json!({"name":v.name,"mount":v.mount,"total_bytes":v.total_bytes,"free_bytes":v.free_bytes,"roles":v.roles,"available":v.total_bytes > 0})).collect::<Vec<_>>(),
        })
    }
}

/// `/Volumes/Beta/Media` is on "Beta".
fn volume_name(path: &Path) -> Option<String> {
    let relative = path.strip_prefix("/Volumes").ok()?;
    relative.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned())
}

/// One mounted volume holding at least one library root.
#[derive(Debug, Clone, PartialEq)]
pub struct Volume {
    /// Where it is mounted, e.g. "/Volumes/Alpha".
    pub mount: PathBuf,
    pub name: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    /// The library roles kept on it: "downloads", "movies", "tv".
    pub roles: Vec<String>,
}

/// Size and free space of the file system holding `path`, for an unprivileged user.
fn capacity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut stat) } != 0 { return None; }
    let block = stat.f_frsize as u64;
    Some((stat.f_blocks as u64 * block, stat.f_bavail as u64 * block))
}

/// The mount point of `path`: the highest ancestor still on the same device.
fn mount_point(path: &Path) -> PathBuf {
    let Ok(device) = std::fs::metadata(path).map(|m| m.dev()) else { return path.to_path_buf() };
    let mut mount = path.to_path_buf();
    for ancestor in path.ancestors().skip(1) {
        match std::fs::metadata(ancestor) {
            Ok(meta) if meta.dev() == device => mount = ancestor.to_path_buf(),
            _ => break,
        }
    }
    mount
}

/// The volumes behind the given roles' roots, one per device, in mount order. A root that
/// is missing (a drive not mounted) is reported with no capacity rather than left out.
pub fn volumes(roots: &[(&str, &Path)]) -> Vec<Volume> {
    let mut found = BTreeMap::<PathBuf, Volume>::new();
    for (role, path) in roots {
        if path.as_os_str().is_empty() { continue; }
        let (mount, capacity) = match std::fs::metadata(path) {
            Ok(_) => (mount_point(path), capacity(path)),
            Err(_) => (path.to_path_buf(), None),
        };
        let name = if mount == Path::new("/") { "Macintosh HD".to_owned() }
            else { mount.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() };
        let volume = found.entry(mount.clone()).or_insert_with(|| Volume {
            mount, name, total_bytes: capacity.map_or(0, |c| c.0), free_bytes: capacity.map_or(0, |c| c.1), roles: Vec::new(),
        });
        if !volume.roles.iter().any(|r| r == role) { volume.roles.push((*role).to_owned()); }
    }
    found.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn config(extra: Value) -> Config {
        let mut base = json!({"listen_host":"127.0.0.1","listen_port":1,"token_file":"t","state_file":"s",
            "transmission_url":"http://127.0.0.1:9091","transmission_auth_file":null,
            "source":"/A/Media/Downloads","movies":"/A/Media/Movies","tv":"/A/Media/TV","poll_seconds":30});
        base.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let config: Config = serde_json::from_value(base).unwrap();
        config.validate().unwrap();
        config
    }

    #[test]
    fn without_a_drive_list_the_configured_folders_are_drive_one() {
        let drives = Drives::of(&config(json!({})));
        assert_eq!(drives.all().len(), 1);
        let d = drives.primary();
        assert_eq!((d.id.as_str(), d.downloads.as_path(), d.movies.as_path(), d.tv.as_path()),
            (LEGACY_DRIVE_ID, Path::new("/A/Media/Downloads"), Path::new("/A/Media/Movies"), Path::new("/A/Media/TV")));
        assert!(d.accept_new);
        assert_eq!(d.reserve_bytes, DEFAULT_RESERVE_BYTES);
    }

    #[test]
    fn a_drive_list_resolves_media_folders_and_overrides() {
        let drives = Drives::of(&config(json!({"source":"","movies":"","tv":"","drives":[
            {"id":"alpha","media":"/Volumes/Alpha/Media","tv":"/Volumes/Alpha/Shows"},
            {"id":"beta","media":"/Volumes/Beta/Media","accept_new":false,"reserve_bytes":1}]})));
        let [alpha, beta] = drives.all() else { panic!() };
        assert_eq!(alpha.tv, Path::new("/Volumes/Alpha/Shows"));
        assert_eq!(alpha.movies, Path::new("/Volumes/Alpha/Media/Movies"));
        assert_eq!(beta.downloads, Path::new("/Volumes/Beta/Media/Downloads"));
        assert!(!beta.accept_new && beta.reserve_bytes == 1);
        let film = Path::new("/Volumes/Beta/Media/Movies/Film/Film.mkv");
        assert_eq!(drives.drive_of(film).map(|d| d.id.as_str()), Some("beta"));
        assert!(drives.in_library(film));
        assert!(!drives.in_library(Path::new("/Volumes/Beta/Media/Downloads/x.mkv")));
        assert!(drives.is_managed(Path::new("/Volumes/Alpha/Shows/S/x.mkv")));
        assert!(!drives.is_managed(Path::new("/Volumes/Alpha/Media/TV/x.mkv")));
        assert!(!drives.is_managed(Path::new("/Volumes/Beta/Elsewhere/x.mkv")));
        assert!(drives.is_root(Path::new("/Volumes/Beta/Media/TV")));
        assert_eq!(drives.downloading_into(Path::new("/Volumes/Beta/Media/Downloads")).map(|d| d.id.as_str()), Some("beta"));
        assert_eq!(drives.roots(Role::Tv).collect::<Vec<_>>(), [Path::new("/Volumes/Alpha/Shows"), Path::new("/Volumes/Beta/Media/TV")]);
    }

    #[test]
    fn invalid_drive_lists_are_refused() {
        let base = json!({"listen_host":"127.0.0.1","listen_port":1,"token_file":"t","state_file":"s",
            "transmission_url":"x","transmission_auth_file":null,"source":"","movies":"","tv":"","poll_seconds":30});
        let refused = |drives: Value| {
            let mut v = base.clone(); v["drives"] = drives;
            serde_json::from_value::<Config>(v).map_err(anyhow::Error::from).and_then(|c| c.validate()).is_err()
        };
        assert!(refused(json!([{"id":"a","media":"/A"},{"id":"a","media":"/B"}])), "duplicate id");
        assert!(refused(json!([{"id":"a","media":"/A"},{"id":"b","media":"/A/Movies"}])), "overlap across drives");
        assert!(refused(json!([{"id":"a","media":"relative"}])), "relative");
        assert!(refused(json!([{"id":"a"}])), "no folders");
        assert!(refused(json!([{"id":"","media":"/A"}])), "empty id");
        assert!(refused(json!([{"id":"a/b","media":"/A"}])), "unsafe id");
        let mut legacy_too = base.clone();
        legacy_too["source"] = json!("/C/Downloads");
        legacy_too["drives"] = json!([{"id":"a","media":"/A"}]);
        assert!(serde_json::from_value::<Config>(legacy_too).unwrap().validate().is_err(), "both forms");
        assert!(!refused(json!([{"id":"a","media":"/A"},{"id":"b","media":"/B"}])));
    }

    #[test]
    fn roots_on_one_drive_are_one_volume_and_a_missing_drive_is_unavailable() {
        let dir = std::env::temp_dir().join(format!("torrbaby-volumes-{}", std::process::id()));
        for sub in ["Downloads", "Movies"] { std::fs::create_dir_all(dir.join(sub)).unwrap(); }
        let (a, b) = (dir.join("Downloads"), dir.join("Movies"));
        let volumes = super::volumes(&[("downloads", &a), ("movies", &b), ("tv", Path::new("/Volumes/Not-Mounted-Torrbaby/TV"))]);
        assert_eq!(volumes.len(), 2);
        let local = volumes.iter().find(|v| v.roles.contains(&"downloads".to_owned())).unwrap();
        assert_eq!(local.roles, ["downloads", "movies"]);
        assert!(local.total_bytes > 0 && local.free_bytes <= local.total_bytes);
        assert_eq!(volumes.iter().find(|v| v.roles == ["tv"]).unwrap().total_bytes, 0);
        std::fs::remove_dir_all(dir).ok();
    }
}
