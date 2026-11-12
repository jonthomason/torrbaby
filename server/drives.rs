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
    /// The name given in Settings, if any.
    pub custom_name: Option<String>,
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
            Err(_) => self.elsewhere().map_or(Status::Missing, Status::Elsewhere),
        };
        if status == Status::Online {
            // The marker proves this is the drive, so its folders can safely be created here.
            // A root set on another volume is created only while that volume is mounted.
            for (_, root) in self.roots() { if !root.is_dir() { let _ = create_media_folder(root); } }
            if let Ok(mut dismissed) = DISMISSED.lock() { dismissed.remove(&self.id); }
        }
        status
    }
    pub fn online(&self) -> bool { self.status() == Status::Online }
    /// As saved in a drive list: folders given exactly only where they differ from `Media/…`.
    pub fn to_config(&self) -> crate::config::DriveConfig {
        let exact = |root: &Path, folder: &str| (root != self.media.join(folder)).then(|| root.to_string_lossy().into_owned());
        crate::config::DriveConfig {
            id: self.id.clone(), name: self.custom_name.clone(), media: Some(self.media.to_string_lossy().into_owned()),
            downloads: exact(&self.downloads, "Downloads"), movies: exact(&self.movies, "Movies"), tv: exact(&self.tv, "TV"),
            accept_new: self.accept_new, reserve_bytes: (self.reserve_bytes != DEFAULT_RESERVE_BYTES).then_some(self.reserve_bytes),
        }
    }
    /// This drive's Media folder on another mounted volume, found by its marker.
    fn elsewhere(&self) -> Option<PathBuf> {
        let relative = self.media.strip_prefix("/Volumes").ok()?.components().skip(1).collect::<PathBuf>();
        std::fs::read_dir("/Volumes").ok()?.flatten().map(|v| v.path().join(&relative)).find(|media| {
            *media != self.media && std::fs::read(media.join(MARKER)).ok()
                .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
                .is_some_and(|v| v["id"] == self.id.as_str())
        })
    }
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
    /// Connected, but mounted somewhere else (macOS names it "Beta 1" when "Beta" is taken).
    Elsewhere(PathBuf),
}

/// The file on each drive that says which drive it is.
pub const MARKER: &str = ".torrbaby-drive.json";

/// The id in a Media folder's marker, if it has one.
pub fn read_marker(media: &Path) -> Option<String> {
    let raw = std::fs::read(media.join(MARKER)).ok()?;
    serde_json::from_slice::<Value>(&raw).ok()?["id"].as_str().map(str::to_owned)
}
/// Size and free space of a connected drive.
pub fn space(drive: &Drive) -> (u64, u64) { capacity(&drive.media).unwrap_or((0, 0)) }
pub fn capacity_of(path: &Path) -> Option<(u64, u64)> { capacity(path) }
/// Whether `path` is where a volume is mounted.
pub fn is_mount(path: &Path) -> bool {
    match (std::fs::metadata(path), path.parent().map(std::fs::metadata)) {
        (Ok(here), Some(Ok(parent))) => here.dev() != parent.dev(),
        _ => false,
    }
}

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

/// Create a folder for new media, unless it belongs on a volume that is not mounted:
/// then it would be made on the startup disk instead, out of sight of its drive.
pub fn create_media_folder(folder: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(on_its_volume(folder), "Its drive is not connected");
    std::fs::create_dir_all(folder)?;
    Ok(())
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
                id: LEGACY_DRIVE_ID.into(), name: volume_name(&media).unwrap_or_else(|| LEGACY_DRIVE_ID.into()), custom_name: None, media,
                downloads, movies, tv, accept_new: true, reserve_bytes: DEFAULT_RESERVE_BYTES,
            }]);
        }
        Drives(config.drives.iter().map(|d| {
            let media = d.media.as_deref().map(Path::new);
            let root = |explicit: &Option<String>, folder: &str| explicit.as_ref().map(PathBuf::from)
                .or_else(|| media.map(|m| m.join(folder))).unwrap_or_default();
            let media = media.map(Path::to_path_buf).unwrap_or_else(|| root(&d.downloads, "Downloads"));
            Drive {
                name: d.name.clone().or_else(|| volume_name(&media)).unwrap_or_else(|| d.id.clone()), custom_name: d.name.clone(), media,
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
    /// Where a new download goes (docs/disk-aggregation.md, placement rules): `prefer`
    /// (the drive with the show) when it has room; else the drive with the most free space.
    /// Only connected drives that take new downloads are considered, and each keeps its
    /// reserve free. When every drive is down to its reserve, the one with the most space
    /// still takes a download that fits, so a full library slows down rather than stops.
    /// `bytes` is 0 when the size is not known yet.
    pub fn place(&self, prefer: Option<&str>, bytes: u64) -> anyhow::Result<&Drive> {
        let candidates: Vec<(&Drive, u64)> = self.0.iter().filter(|d| d.accept_new && d.online())
            .filter_map(|d| capacity(&d.media).map(|(_, free)| (d, free))).collect();
        anyhow::ensure!(!candidates.is_empty(), "No connected drive takes new downloads");
        let roomy = |(d, free): &&(&Drive, u64)| *free >= bytes.saturating_add(d.reserve_bytes);
        if let Some((d, _)) = candidates.iter().filter(roomy).find(|(d, _)| Some(d.id.as_str()) == prefer) { return Ok(d); }
        if let Some((d, _)) = candidates.iter().filter(roomy).max_by_key(|(_, free)| *free) { return Ok(d); }
        let (d, free) = candidates.iter().max_by_key(|(_, free)| *free).expect("candidates is not empty");
        anyhow::ensure!(*free > bytes.saturating_add(MARGIN_BYTES), "Not enough free space on any drive");
        Ok(d)
    }
    /// Whether `drive` can take `bytes` more and keep its reserve.
    pub fn has_room(drive: &Drive, bytes: u64) -> bool {
        capacity(&drive.media).is_some_and(|(_, free)| free >= bytes.saturating_add(drive.reserve_bytes))
    }
    /// The drive holding the newest season of a show already in the library.
    pub fn show_drive(&self, assets: &[Value], entity: &str) -> Option<String> {
        assets.iter().filter(|a| a["entity_id"] == entity && a["role"] != "extra")
            .filter_map(|a| Some((a["season"].as_u64().unwrap_or(0), self.drive_of(Path::new(a["path"].as_str()?))?)))
            .max_by_key(|(season, _)| *season).map(|(_, d)| d.id.clone())
    }
    /// Where a new download named `name` goes: with its show when the library has it.
    pub fn place_named(&self, entities: &[Value], assets: &[Value], name: Option<&str>, bytes: u64) -> anyhow::Result<&Drive> {
        let show = name.and_then(|name| show_for_name(entities, assets, name));
        let prefer = show.as_deref().and_then(|show| self.show_drive(assets, show));
        self.place(prefer.as_deref(), bytes)
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
                Status::Elsewhere(_) => "It is connected under another name. Restart the server Mac to put it back. Nothing has been removed.",
            };
            let now = crate::storage::now();
            Some(json!({"id":ATTENTION_BASE + index as i64,"asset_id":"","title":d.name,"stage":"drive","severity":"error",
                "summary":"Drive not connected","error":error,"occurrences":1,"first_seen":now,"last_seen":now}))
        }).collect()
    }
    /// Dismiss drive items by attention id, or all of them; they come back if the drive
    /// returns and goes missing again.
    pub fn dismiss(&self, ids: &[i64], all: bool) {
        // Status first: it takes the same lock when a drive is back.
        let away: Vec<String> = self.0.iter().enumerate().filter(|(index, d)| (all || ids.contains(&(ATTENTION_BASE + *index as i64))) && !d.online())
            .map(|(_, d)| d.id.clone()).collect();
        if let Ok(mut dismissed) = DISMISSED.lock() { dismissed.extend(away); }
    }
    /// For clients: every volume, plus the totals across them.
    pub fn report(&self) -> Value {
        // A drive that is not connected is one row under its own name, not one per folder.
        let (here, away): (Vec<&Drive>, Vec<&Drive>) = self.0.iter().partition(|d| d.media.is_dir());
        let roots: Vec<(&str, &Path)> = here.iter().flat_map(|d| d.roots().map(|(role, root)| (role.name(), root))).collect();
        let mut volumes = volumes(&roots);
        volumes.extend(away.into_iter().map(|d| Volume { mount: d.media.clone(), name: d.name.clone(), total_bytes: 0, free_bytes: 0,
            roles: Role::ALL.iter().map(|r| r.name().to_owned()).collect() }));
        json!({
            "free_bytes": volumes.iter().map(|v| v.free_bytes).sum::<u64>(),
            "total_bytes": volumes.iter().map(|v| v.total_bytes).sum::<u64>(),
            "volumes": volumes.iter().map(|v| json!({"name":v.name,"mount":v.mount,"total_bytes":v.total_bytes,"free_bytes":v.free_bytes,"roles":v.roles,"available":v.total_bytes > 0})).collect::<Vec<_>>(),
        })
    }
}

/// Never fill a drive closer than this, even when every drive is down to its reserve.
const MARGIN_BYTES: u64 = 2_000_000_000;

/// The show in the library a release name belongs to, by its title.
pub fn show_for_name(entities: &[Value], assets: &[Value], name: &str) -> Option<String> {
    let (title, _, kind) = crate::metadata::parse_name(name);
    if kind != "tv" { return None; }
    show_titled(entities, assets, &title)
}
/// The show in the library with this title.
pub fn show_titled(entities: &[Value], assets: &[Value], title: &str) -> Option<String> {
    let wanted = crate::metadata::normalized(title);
    if wanted.is_empty() { return None; }
    let named = |value: &Value| value.as_str().is_some_and(|t| crate::metadata::normalized(t) == wanted);
    entities.iter().find(|e| e["kind"] == "tv" && (named(&e["title"]) || named(&e["original_title"])))
        .and_then(|e| e["id"].as_str().map(str::to_owned))
        .or_else(|| assets.iter().find(|a| named(&a["series_name"]) && a["entity_id"].is_string())
            .and_then(|a| a["entity_id"].as_str().map(str::to_owned)))
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
    fn new_downloads_go_with_their_show_else_to_the_most_room_and_never_to_a_missing_drive() {
        let root = std::env::temp_dir().join(format!("torrbaby-place-{}", uuid::Uuid::new_v4()));
        let media = |d: &str| root.join(d).join("Media");
        for d in ["Alpha", "Beta", "Gamma"] { for f in ["Downloads", "Movies", "TV"] { std::fs::create_dir_all(media(d).join(f)).unwrap(); } }
        let free = capacity(&root).unwrap().1;
        let drives = |list: Value| Drives::of(&config(json!({"source":"","movies":"","tv":"","drives":list})));
        let plain = |id: &str, d: &str| json!({"id":id,"media":media(d)});
        // One disk underneath, so which has "most room" varies from read to read.
        let three = drives(json!([plain("alpha","Alpha"), plain("beta","Beta"), plain("gamma","Gamma")]));
        assert!(three.place(None, 1).is_ok());
        assert_eq!(three.place(Some("gamma"), 1).unwrap().id, "gamma", "a show stays where it is");
        // A drive down to its reserve takes nothing while another has room, even for its show.
        let full = drives(json!([{"id":"alpha","media":media("Alpha"),"reserve_bytes":u64::MAX / 4}, plain("beta","Beta")]));
        assert_eq!(full.place(Some("alpha"), 1_000).unwrap().id, "beta");
        // Retired drives and missing drives are skipped.
        let retired = drives(json!([{"id":"alpha","media":media("Alpha"),"accept_new":false}, plain("beta","Beta")]));
        assert_eq!(retired.place(Some("alpha"), 1).unwrap().id, "beta");
        let missing = drives(json!([{"id":"alpha","media":root.join("Unplugged/Media")}, plain("beta","Beta")]));
        assert_eq!(missing.place(Some("alpha"), 1).unwrap().id, "beta");
        assert!(!root.join("Unplugged").exists());
        // Every drive at its reserve: the most room still takes what fits; nothing takes what does not.
        let tight = drives(json!([{"id":"alpha","media":media("Alpha"),"reserve_bytes":free}]));
        assert_eq!(tight.place(None, 1).unwrap().id, "alpha");
        assert!(tight.place(None, free).is_err());
        let none = drives(json!([{"id":"alpha","media":media("Alpha"),"accept_new":false}]));
        assert!(none.place(None, 1).is_err());
        // Episodes follow the drive with the show's newest season.
        let entities = vec![json!({"id":"tmdb:tv:1","kind":"tv","title":"The Show"}), json!({"id":"tmdb:movie:2","kind":"movie","title":"The Show"})];
        let assets = vec![json!({"entity_id":"tmdb:tv:1","season":1,"path":media("Alpha").join("TV/The Show/S01E01.mkv")}),
                          json!({"entity_id":"tmdb:tv:1","season":2,"path":media("Gamma").join("TV/The Show/S02E01.mkv")})];
        assert_eq!(show_for_name(&entities, &assets, "The.Show.S02E05.1080p.WEB.mkv").as_deref(), Some("tmdb:tv:1"));
        assert_eq!(show_for_name(&entities, &assets, "Another.Show.S01E01.mkv"), None);
        assert_eq!(show_for_name(&entities, &assets, "The.Show.2019.1080p.mkv"), None, "a film is not an episode");
        assert_eq!(three.place_named(&entities, &assets, Some("The.Show.S02E05.mkv"), 1).unwrap().id, "gamma");
        let only_beta = drives(json!([{"id":"alpha","media":media("Alpha"),"reserve_bytes":u64::MAX / 4}, plain("beta","Beta"), {"id":"gamma","media":media("Gamma"),"reserve_bytes":u64::MAX / 4}]));
        assert_eq!(only_beta.place_named(&entities, &assets, Some("The.Show.S03E01.mkv"), 1).unwrap().id, "beta", "the show's drive is full");
        std::fs::remove_dir_all(root).ok();
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
