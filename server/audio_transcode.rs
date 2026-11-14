//! Dolby Digital Plus copies of audio tracks that Apple devices cannot decode
//! (DTS, TrueHD, FLAC, Opus…). A bundled ffmpeg decodes the original track once;
//! the copy lives in the server's data directory, is spliced into the virtual
//! MP4 next to the untouched video, and is removed with the title.
//!
//! Only files with no directly playable audio track are converted. The copy is
//! written on a timeline that starts at zero with gaps filled by silence, so
//! frame N always plays at N × 1536 / 48000 seconds.
use anyhow::{Context, Result, ensure};
use matroska_demuxer::{MatroskaFile, TrackType};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

/// Part of every record; raise it to redo conversions (and earlier failures).
const VERSION: u64 = 1;
const MAX_TRACKS: usize = 4;
pub const SAMPLE_RATE: u32 = 48_000;
pub const FRAME_SAMPLES: u64 = 1536;
/// One conversion at a time: several index builds can ask for the same title.
static GATE: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sidecar {
    /// Matroska track number of the original.
    pub track: u64,
    pub path: PathBuf,
}

pub fn directory(data: &Path) -> PathBuf { data.join("converted-audio") }

/// Keyed on the file's content (size and modification time), so a renamed or moved file
/// keeps its copy.
fn key(asset: &str, revision: &str) -> String {
    use sha2::{Digest, Sha256};
    let content = crate::subtitle_jobs::content(revision).unwrap_or_else(|| revision.to_owned());
    Sha256::digest(format!("{asset}\ncontent\n{content}").as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The folder holding this file's copy: its content key, or a copy made before copies were
/// keyed on content (by path and full revision), found by its record. Earlier copies are
/// not moved, because the indexes built with them name their paths.
fn folder(data: &Path, asset: &str, revision: &str) -> PathBuf {
    let root = directory(data);
    let keyed = root.join(key(asset, revision));
    if keyed.exists() { return keyed; }
    let earlier = fs::read_dir(&root).into_iter().flatten().flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().ends_with(".building"))
        .map(|entry| entry.path())
        .find(|path| record(path).is_some_and(|value| value["asset_id"] == asset
            && value["revision"].as_str().is_some_and(|r| crate::subtitle_jobs::same_content(r, revision))));
    earlier.unwrap_or(keyed)
}

fn helper() -> Option<PathBuf> {
    let path = std::env::current_exe().ok()?.with_file_name("torrbaby-ffmpeg");
    path.is_file().then_some(path)
}

fn record(folder: &Path) -> Option<Value> {
    let value: Value = serde_json::from_slice(&fs::read(folder.join("record.json")).ok()?).ok()?;
    (value["version"].as_u64() == Some(VERSION)).then_some(value)
}

fn save(folder: &Path, document: &Value) -> Result<()> {
    fs::create_dir_all(folder)?;
    let temporary = folder.join("record.json.tmp");
    fs::write(&temporary, document.to_string())?;
    fs::rename(temporary, folder.join("record.json"))?;
    Ok(())
}

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn ready(folder: &Path, value: &Value) -> Option<Vec<Sidecar>> {
    if value["state"] != "ready" { return None; }
    let mut sidecars = Vec::new();
    for track in value["tracks"].as_array()? {
        let path = folder.join(track["file"].as_str()?);
        if path.metadata().ok()?.len() != track["bytes"].as_u64()? { return None; }
        sidecars.push(Sidecar { track: track["track"].as_u64()?, path });
    }
    (!sidecars.is_empty()).then_some(sidecars)
}

/// True while a conversion could still give this file a playable audio track:
/// the converter is installed and has not already failed on this revision.
pub fn wanted(data: &Path, asset: &str, revision: &str) -> bool {
    helper().is_some()
        && record(&folder(data, asset, revision)).is_none_or(|r| r["state"] != "failed")
}

/// Whether every audio copy an index was built with is still in place.
pub fn intact(sidecars: &[(PathBuf, u64)]) -> bool {
    sidecars.iter().all(|(path, bytes)| path.metadata().is_ok_and(|m| m.len() == *bytes))
}

struct Plan {
    track: u64,
    stream: usize,
    channels: u64,
    codec: String,
}

fn supported(codec: &str) -> bool { matches!(codec, "A_AAC" | "A_AC3" | "A_EAC3") }

/// Tracks to convert: None when the file has no audio or already has a track that plays as is.
fn plan(input: &Path) -> Result<Option<Vec<Plan>>> {
    let mkv = MatroskaFile::open(BufReader::new(crate::matroska::Bounded::open(input)?))?;
    let audio = || mkv.tracks().iter().enumerate().filter(|(_, t)| t.track_type() == TrackType::Audio);
    if audio().next().is_none() || audio().any(|(_, t)| supported(t.codec_id())) { return Ok(None); }
    Ok(Some(audio()
        .filter(|(_, t)| t.content_encodings().is_none())
        .take(MAX_TRACKS)
        .map(|(stream, t)| Plan {
            track: t.track_number().get(),
            stream,
            channels: t.audio().map_or(2, |a| a.channels().get()),
            codec: t.codec_id().to_owned(),
        })
        .collect()))
}

fn arguments(input: &Path, folder: &Path, plans: &[Plan]) -> Vec<std::ffi::OsString> {
    let mut list: Vec<std::ffi::OsString> = ["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"].map(Into::into).into();
    list.push(input.into());
    for plan in plans {
        // Dolby Digital Plus carries at most 5.1 here; wider layouts are folded down.
        let (channels, rate) = match plan.channels {
            0 | 1 => (None, "128k"),
            2 => (None, "224k"),
            3..=5 => (None, "448k"),
            6 => (None, "640k"),
            _ => (Some("6"), "640k"),
        };
        for part in ["-map".to_owned(), format!("0:{}", plan.stream), "-c:a".into(), "eac3".into(), "-b:a".into(), rate.into(),
            "-ar".into(), SAMPLE_RATE.to_string(), "-af".into(), "aresample=async=1000:first_pts=0".into()] {
            list.push(part.into());
        }
        if let Some(channels) = channels { list.push("-ac".into()); list.push(channels.into()); }
        list.push("-f".into()); list.push("eac3".into());
        list.push(folder.join(format!("track-{}.eac3", plan.track)).into());
    }
    list
}

fn convert(helper: &Path, input: &Path, folder: &Path, plans: &[Plan]) -> Result<()> {
    let mut child = Command::new("/usr/bin/nice").args(["-n", "10"]).arg(helper).args(arguments(input, folder, plans))
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn()?;
    let mut stderr = child.stderr.take().context("Missing converter errors")?;
    let errors = std::thread::spawn(move || { let mut text = Vec::new(); let _ = (&mut stderr).take(8192).read_to_end(&mut text); std::io::copy(&mut stderr, &mut std::io::sink()).ok(); text });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? { break status; }
        if started.elapsed() > Duration::from_secs(3600) {
            let _ = child.kill(); let _ = child.wait();
            anyhow::bail!("Audio conversion timed out");
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    let text = String::from_utf8_lossy(&errors.join().unwrap_or_default()).trim().to_owned();
    ensure!(status.success(), "Audio conversion: {}", if text.is_empty() { "converter failed" } else { &text });
    Ok(())
}

/// The audio copies for a file, converting them first when needed. An empty
/// list means the file plays as it is, or cannot be converted (recorded once so
/// it is not attempted on every request).
pub fn ensure(data: &Path, asset: &str, input: &Path, revision: &str) -> Vec<Sidecar> {
    let Some(helper) = helper() else { return Vec::new() };
    let Ok(_gate) = GATE.lock() else { return Vec::new() };
    let root = directory(data);
    let found = folder(data, asset, revision);
    if let Some(value) = record(&found) {
        if let Some(sidecars) = ready(&found, &value) {
            // Last use, so the startup sweep keeps copies that are still played.
            if let Ok(directory) = fs::File::open(&found) { let _ = directory.set_times(fs::FileTimes::new().set_modified(std::time::SystemTime::now())); }
            return sidecars;
        }
        if value["state"] == "failed" { return Vec::new(); }
    }
    let folder = root.join(key(asset, revision));
    let plans = match plan(input) {
        Ok(None) => return Vec::new(),
        Ok(Some(plans)) if plans.is_empty() => { fail(data, asset, input, revision, "No convertible audio track"); return Vec::new(); }
        Ok(Some(plans)) => plans,
        Err(error) => { tracing::warn!(asset, %error, "Audio conversion could not read the file"); return Vec::new(); }
    };
    let codecs: Vec<&str> = plans.iter().map(|p| p.codec.as_str()).collect();
    let building = root.join(format!("{}.building", key(asset, revision)));
    let started = Instant::now();
    let outcome = (|| -> Result<Vec<Sidecar>> {
        let _ = fs::remove_dir_all(&building);
        fs::create_dir_all(&building)?;
        convert(&helper, input, &building, &plans)?;
        ensure!(crate::subtitle_jobs::revision(input)? == revision, "Media changed during audio conversion");
        let mut tracks = Vec::new();
        for plan in &plans {
            let file = format!("track-{}.eac3", plan.track);
            let bytes = building.join(&file).metadata()?.len();
            ensure!(bytes > 0, "Audio conversion produced an empty track");
            tracks.push(json!({"track":plan.track,"file":file,"bytes":bytes,"source_codec":plan.codec}));
        }
        let document = json!({"version":VERSION,"state":"ready","asset_id":asset,"input":input,"revision":revision,"tracks":tracks,"updated":now()});
        save(&building, &document)?;
        let _ = fs::remove_dir_all(&folder);
        fs::rename(&building, &folder)?;
        ready(&folder, &document).context("Converted audio is incomplete")
    })();
    match outcome {
        Ok(sidecars) => {
            tracing::info!(asset, ?codecs, seconds = started.elapsed().as_secs(), "Audio converted for Apple playback");
            // Copies made for an earlier revision of this file are no longer reachable.
            remove_where(data, |folder_key, value| value["asset_id"] == asset && folder_key != key(asset, revision)
                && !value["revision"].as_str().is_some_and(|r| crate::subtitle_jobs::same_content(r, revision)));
            sidecars
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&building);
            fail(data, asset, input, revision, &error.to_string());
            Vec::new()
        }
    }
}

/// Record that this revision cannot be converted, and drop any partial copy.
pub fn fail(data: &Path, asset: &str, input: &Path, revision: &str, error: &str) {
    tracing::warn!(asset, error, "Audio conversion failed");
    let folder = directory(data).join(key(asset, revision));
    let _ = fs::remove_dir_all(&folder);
    let _ = save(&folder, &json!({"version":VERSION,"state":"failed","asset_id":asset,"input":input,"revision":revision,"error":error,"updated":now()}));
}

fn remove_where(data: &Path, matches: impl Fn(&str, &Value) -> bool) {
    let Ok(entries) = fs::read_dir(directory(data)) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if !entry.file_type().is_ok_and(|t| t.is_dir()) { continue; }
        let Some(value) = fs::read(path.join("record.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { continue };
        if matches(&name, &value) { let _ = fs::remove_dir_all(path); }
    }
}

/// Deleting a title deletes its audio copies with it.
pub fn purge_assets(data: &Path, assets: &BTreeSet<String>) {
    remove_where(data, |_, value| value["asset_id"].as_str().is_some_and(|id| assets.contains(id)));
}

/// At startup: unfinished conversions, and copies whose original was removed outside the
/// app. The path a copy was made from is gone after a rename or a move too, and then the
/// copy is still wanted, so such a copy is removed only after 30 days unused. A file on a
/// disk that is merely unmounted is kept.
pub fn sweep(data: &Path) {
    let Ok(entries) = fs::read_dir(directory(data)) else { return };
    let mut stale = std::collections::HashSet::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".building") { let _ = fs::remove_dir_all(entry.path()); continue; }
        let unused = entry.metadata().and_then(|m| m.modified()).ok().and_then(|at| at.elapsed().ok());
        if unused.is_some_and(|age| age > Duration::from_secs(30 * 24 * 3600)) { stale.insert(name); }
    }
    remove_where(data, |name, value| stale.contains(name) && value["input"].as_str().is_some_and(|input| original_deleted(Path::new(input))));
}

fn original_deleted(input: &Path) -> bool {
    if input.exists() { return false; }
    // /Volumes/<disk>/…: only trust the absence while that disk is mounted.
    let mut parts = input.components();
    let volume: PathBuf = parts.by_ref().take(3).collect();
    if volume.parent() == Some(Path::new("/Volumes")) { return volume.is_dir(); }
    true
}

/// Sequential reader of a converted track: one E-AC-3 syncframe at a time.
pub struct Frames {
    file: BufReader<fs::File>,
    pub offset: u64,
}
impl Frames {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self { file: BufReader::with_capacity(1 << 20, fs::File::open(path)?), offset: 0 })
    }
    /// The next frame and its byte offset in the file; None at the end.
    pub fn next(&mut self) -> Result<Option<(u64, Vec<u8>)>> {
        let mut header = [0u8; 4];
        match self.file.read_exact(&mut header) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        ensure!(header[0] == 0x0b && header[1] == 0x77, "Converted audio lost frame sync");
        // strmtyp(2) substreamid(3) frmsiz(11): frame length in 16-bit words, minus one.
        let length = ((((header[2] & 7) as usize) << 8 | header[3] as usize) + 1) * 2;
        ensure!((7..=4096).contains(&length), "Invalid converted audio frame");
        let mut frame = vec![0u8; length];
        frame[..4].copy_from_slice(&header);
        self.file.read_exact(&mut frame[4..]).context("Truncated converted audio")?;
        let at = self.offset;
        self.offset += length as u64;
        Ok(Some((at, frame)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Result<Self> {
            let path = std::env::temp_dir().join(format!("torrbaby-audio-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path)?;
            Ok(Self(path))
        }
        fn path(&self) -> &Path { &self.0 }
    }
    impl Drop for Scratch { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
    #[test]
    fn copies_leave_with_their_title_and_failures_are_remembered() -> Result<()> {
        let data = Scratch::new()?;
        let folder = directory(data.path()).join(key("asset-a", "r1"));
        fs::create_dir_all(&folder)?;
        fs::write(folder.join("track-2.eac3"), [0u8; 10])?;
        let document = json!({"version":VERSION,"state":"ready","asset_id":"asset-a","input":"/nowhere/a.mkv","revision":"r1",
            "tracks":[{"track":2,"file":"track-2.eac3","bytes":10}]});
        save(&folder, &document)?;
        assert_eq!(ready(&folder, &document), Some(vec![Sidecar { track: 2, path: folder.join("track-2.eac3") }]));
        // A truncated copy is not offered to playback.
        fs::write(folder.join("track-2.eac3"), [0u8; 4])?;
        assert_eq!(ready(&folder, &document), None);
        fail(data.path(), "asset-b", Path::new("/nowhere/b.mkv"), "r1", "decoder missing");
        let failed = record(&directory(data.path()).join(key("asset-b", "r1"))).context("record")?;
        assert_eq!(failed["state"], "failed");
        purge_assets(data.path(), &BTreeSet::from(["asset-a".to_owned()]));
        assert!(!folder.exists());
        assert!(directory(data.path()).join(key("asset-b", "r1")).exists());
        // A missing original may have been renamed or moved: its copy stays while recently used…
        let b = directory(data.path()).join(key("asset-b", "r1"));
        sweep(data.path());
        assert!(b.exists());
        // …and leaves at startup after 30 days unused.
        fs::File::open(&b)?.set_times(fs::FileTimes::new().set_modified(std::time::SystemTime::now() - Duration::from_secs(31 * 24 * 3600)))?;
        sweep(data.path());
        assert!(!b.exists());
        Ok(())
    }
    #[test]
    fn a_renamed_file_finds_its_copy_made_under_the_earlier_key() -> Result<()> {
        let data = Scratch::new()?;
        let before = "16777232:100:5000:1700000000:5:1700000001:6";
        let after = "16777233:200:5000:1700000000:0:1760000000:0"; // moved: new drive, inode, change time
        let earlier = directory(data.path()).join("e".repeat(64));
        fs::create_dir_all(&earlier)?;
        fs::write(earlier.join("track-2.eac3"), [0u8; 10])?;
        save(&earlier, &json!({"version":VERSION,"state":"ready","asset_id":"asset-a","input":"/Volumes/Alpha/a.mkv","revision":before,
            "tracks":[{"track":2,"file":"track-2.eac3","bytes":10}]}))?;
        assert_eq!(folder(data.path(), "asset-a", after), earlier);
        assert_ne!(folder(data.path(), "asset-b", after), earlier);
        assert_ne!(folder(data.path(), "asset-a", "16777233:200:5001:1700000000:0:1760000000:0"), earlier);
        Ok(())
    }
    #[test]
    fn an_unmounted_disk_does_not_look_like_a_deleted_file() {
        assert!(!original_deleted(Path::new("/Volumes/torrbaby-test-disk-that-is-not-mounted/Movies/a.mkv")));
        assert!(original_deleted(Path::new("/private/tmp/torrbaby-missing/a.mkv")));
    }
    #[test]
    fn frames_follow_the_declared_length() -> Result<()> {
        let folder = Scratch::new()?;
        let path = folder.path().join("a.eac3");
        // frmsiz 3 → 8 bytes, then frmsiz 4 → 10 bytes.
        let mut bytes = vec![0x0b, 0x77, 0x00, 0x03, 1, 2, 3, 4];
        bytes.extend([0x0b, 0x77, 0x00, 0x04, 1, 2, 3, 4, 5, 6]);
        fs::write(&path, &bytes)?;
        let mut frames = Frames::open(&path)?;
        assert_eq!(frames.next()?.map(|(at, f)| (at, f.len())), Some((0, 8)));
        assert_eq!(frames.next()?.map(|(at, f)| (at, f.len())), Some((8, 10)));
        assert!(frames.next()?.is_none());
        Ok(())
    }
}
