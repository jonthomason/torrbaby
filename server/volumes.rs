//! The drives Torrbaby keeps media on. Every library root (downloads, movies, TV) lives on
//! some volume; several roots can share one. This is the single place that answers "which
//! drive, how much room": callers ask here rather than statting paths themselves, so
//! spanning several drives later changes this module, not every caller.
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

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
pub fn volumes(roots: &[(&str, &str)]) -> Vec<Volume> {
    let mut found = BTreeMap::<PathBuf, Volume>::new();
    for (role, root) in roots {
        if root.is_empty() { continue; }
        let path = Path::new(root);
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

/// For clients: every volume, plus the totals across them.
pub fn report(source: &str, movies: &str, tv: &str) -> Value {
    let volumes = volumes(&[("downloads", source), ("movies", movies), ("tv", tv)]);
    json!({
        "free_bytes": volumes.iter().map(|v| v.free_bytes).sum::<u64>(),
        "total_bytes": volumes.iter().map(|v| v.total_bytes).sum::<u64>(),
        "volumes": volumes.iter().map(|v| json!({"name":v.name,"mount":v.mount,"total_bytes":v.total_bytes,"free_bytes":v.free_bytes,"roles":v.roles,"available":v.total_bytes > 0})).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn roots_on_one_drive_are_one_volume_and_a_missing_drive_is_unavailable() {
        let dir = std::env::temp_dir().join(format!("torrbaby-volumes-{}", std::process::id()));
        for sub in ["Downloads", "Movies"] { std::fs::create_dir_all(dir.join(sub)).unwrap(); }
        let (a, b) = (dir.join("Downloads"), dir.join("Movies"));
        let volumes = super::volumes(&[("downloads", a.to_str().unwrap()), ("movies", b.to_str().unwrap()), ("tv", "/Volumes/Not-Mounted-Torrbaby/TV")]);
        assert_eq!(volumes.len(), 2);
        let local = volumes.iter().find(|v| v.roles.contains(&"downloads".to_owned())).unwrap();
        assert_eq!(local.roles, ["downloads", "movies"]);
        assert!(local.total_bytes > 0 && local.free_bytes <= local.total_bytes);
        assert_eq!(volumes.iter().find(|v| v.roles == ["tv"]).unwrap().total_bytes, 0);
        std::fs::remove_dir_all(dir).ok();
    }
}
