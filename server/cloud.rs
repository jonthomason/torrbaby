//! Cloud-drive downloads. Shared folders are found through the local link-search
//! service, opened to see what they really hold, and the chosen videos are saved into
//! the member's own Quark account and fetched from there into the library.
//!
//! The Quark client is written against the observed behaviour of the service's web
//! interface. The member's login cookie is kept in a file only this server reads and is
//! never returned to a client, logged, or placed in a result.
use crate::api::Service;
use anyhow::{Context, Result, bail, ensure};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

const API: &str = "https://drive-pc.quark.cn/1/clouddrive";
const BROWSER: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";
/// Large files are only handed to Quark's desktop client, so downloads identify as it.
const DESKTOP: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) quark-cloud-drive/2.5.20 Chrome/100.0.4896.160 Electron/18.3.5.4-b478491100 Safari/537.36 Channel/pckk_other_ch";
/// Folder in the member's drive that holds files on their way to the library.
const STAGING: &str = "Torrbaby";
/// Pieces fetched at once. The drive's own client keeps to a few; many long-running
/// connections are what it throttles.
const CONNECTIONS: u64 = 4;

#[derive(Clone, Serialize, Deserialize)]
struct JobFile {
    /// Path inside the share, as the search result listed it.
    path: String,
    size: u64,
    destination: PathBuf,
    #[serde(default)]
    done: bool,
    /// How the video met the library: "added", "replaced" a lesser copy of its
    /// episode, or "discarded" because the library's copy was as good or better.
    #[serde(default)]
    outcome: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Job {
    id: String,
    url: String,
    title: String,
    /// queued, saving, downloading, done or failed.
    state: String,
    files: Vec<JobFile>,
    #[serde(default)]
    error: Option<String>,
    created: f64,
    /// Failed runs since the job last made progress; a few are retried unasked.
    #[serde(default)]
    attempts: u32,
}

/// A torrent whose episodes a better cloud copy has made redundant. It is removed,
/// with its files, once that is harmless: see `Cloud::reap`.
#[derive(Clone, Default, Serialize, Deserialize)]
struct Superseded {
    /// The torrent's library files that now have a better copy.
    paths: Vec<String>,
    /// The cloud copy is at least as sharp as the torrent's name says it would be,
    /// so an unfinished download of it is not worth waiting for.
    #[serde(default)]
    outclassed: bool,
}

pub struct Cloud {
    cookie_file: PathBuf,
    jobs_file: PathBuf,
    search_url: String,
    http: reqwest::Client,
    jobs: Mutex<Vec<Job>>,
    /// Bytes fetched so far by the job that is running.
    progress: AtomicU64,
    searches: Mutex<HashMap<String, (Instant, Value)>>,
    wake: tokio::sync::Notify,
    /// When the download speed was last measured, the bytes fetched by then, and the speed.
    pace: Mutex<std::collections::VecDeque<(Instant, u64)>>,
    /// Jobs the member has cancelled while they were running.
    cancelled: Mutex<std::collections::HashSet<String>>,
    /// What reading a copy's first video found, by share and copy; kept on disk.
    probes_file: PathBuf,
    probes: Mutex<HashMap<String, Value>>,
    /// One copy is read at a time: each read briefly saves a video to the member's drive.
    probing: tokio::sync::Mutex<()>,
    /// Asks for the library to be scanned; several asks while one scan runs make one more scan.
    rescan: tokio::sync::Notify,
    /// Torrents waiting to be removed because better copies of their episodes arrived, by hash; kept on disk.
    superseded_file: PathBuf,
    superseded: Mutex<HashMap<String, Superseded>>,
}

static VIDEO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\.(mp4|mkv|ts|mov|m2ts|avi)$").expect("valid regex"));
static SHARE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^https://pan\.quark\.cn/s/([0-9a-zA-Z]{6,40})").expect("valid regex"));

/// A name without the invisible characters some sharers thread through it to defeat
/// matching: "0\u{200b}1.mkv" reads "01.mkv" to the eye and must to the server too.
pub fn visible(name: &str) -> String {
    name.chars().filter(|c| !matches!(c, '\u{00AD}' | '\u{034F}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}' | '\u{FEFF}')).collect()
}
/// Season and episode a file name states, if any: "S01E02", "第12集", "EP12", "12.mkv".
pub fn episode(name: &str) -> (Option<u64>, Option<u64>) {
    // A date in the name ("2026.07.02-先导片") is not an episode number.
    static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:19|20)\d{2}[.\-_/年]\d{1,2}[.\-_/月]\d{1,2}日?").expect("valid regex"));
    let name = &DATE.replace_all(&visible(name), " ").into_owned();
    static SEASONED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)s(\d{1,2})[ ._-]?e(\d{1,3})").expect("valid regex"));
    static NAMED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?:第\s*(\d{1,3})\s*[集话話期]|\bep?[ ._-]?(\d{1,3})\b)").expect("valid regex"));
    static NOISE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(\d{3,4}p|\d{2,3}fps|[48]k|(19|20)\d{2}|[hx]\.?26[45]|ddp?\d\.\d|aac\d\.\d|\d+bit)").expect("valid regex"));
    // Audio layouts such as "5.1". Removed only after the labels above, or "01.4K" loses its "1.4".
    static DECIMAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|[^0-9])\d\.\d").expect("valid regex"));
    static BARE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|[^0-9])(\d{1,3})(?:[^0-9]|$)").expect("valid regex"));
    let stem = name.rsplit('/').next().unwrap_or(name);
    let stem = stem.rsplit_once('.').map_or(stem, |(stem, _)| stem);
    if let Some(found) = SEASONED.captures(stem) { return (found[1].parse().ok(), found[2].parse().ok()); }
    if let Some(found) = NAMED.captures(stem) {
        return (None, found.get(1).or(found.get(2)).and_then(|m| m.as_str().parse().ok()));
    }
    let plain = NOISE.replace_all(stem, " ");
    let plain = DECIMAL.replace_all(&plain, " ");
    (None, BARE.captures(&plain).and_then(|found| found[1].parse().ok()).filter(|n| *n > 0))
}

/// The season a title names: "第二季", "S02", "Season 2".
pub fn stated_season(text: &str) -> Option<u64> {
    static CHINESE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"第\s*([一二三四五六七八九十\d]{1,3})\s*[季部]").expect("valid regex"));
    static LATIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])(?:s|season\s*)(\d{1,2})(?:[^0-9]|$)").expect("valid regex"));
    if let Some(found) = CHINESE.captures(text) {
        let numeral = &found[1];
        if let Ok(number) = numeral.parse() { return Some(number); }
        let digit = |c: char| "一二三四五六七八九".chars().position(|d| d == c).map(|i| i as u64 + 1);
        let letters: Vec<char> = numeral.chars().collect();
        return match letters.as_slice() {
            ['十'] => Some(10),
            ['十', unit] => digit(*unit).map(|u| 10 + u),
            [tens, '十'] => digit(*tens).map(|t| t * 10),
            [tens, '十', unit] => Some(digit(*tens)? * 10 + digit(*unit)?),
            [unit] => digit(*unit),
            _ => None,
        };
    }
    LATIN.captures(text).and_then(|found| found[1].parse().ok()).filter(|n| *n > 0)
}
/// A searched title without the season it names: "庆余年 第二季" is the show "庆余年".
pub fn title_and_season(query: &str) -> (String, Option<u64>) {
    static MARK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s*(?:第\s*[一二三四五六七八九十\d]{1,3}\s*[季部]|\bseason\s*\d{1,2}\b|\bs\d{1,2}\b)\s*").expect("valid regex"));
    let season = stated_season(query);
    let title = if season.is_some() { MARK.replace_all(query, " ").trim().to_owned() } else { query.trim().to_owned() };
    if title.is_empty() { (query.trim().to_owned(), season) } else { (title, season) }
}
/// The picture size a video has, from what the drive measured or, failing that, its name.
fn resolution(video: &Value) -> Option<&'static str> {
    static NAMED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(2160p|4k|uhd)|(1080[pi])|(720p)").expect("valid regex"));
    let (width, height) = (video["width"].as_u64().unwrap_or(0), video["height"].as_u64().unwrap_or(0));
    if width > 0 || height > 0 {
        // Width decides: a 3840×1608 widescreen picture is still 4K.
        return Some(if width >= 3000 || height >= 2000 { "2160p" } else if width >= 1800 || height >= 1000 { "1080p" } else if width >= 1200 || height >= 700 { "720p" } else { "480p" });
    }
    let found = NAMED.captures(video["path"].as_str().unwrap_or(""))?;
    Some(if found.get(1).is_some() { "2160p" } else if found.get(2).is_some() { "1080p" } else { "720p" })
}
/// The separate copies of a series inside one share. Sharers often bundle the same
/// episodes in several qualities, a folder each; a copy is one folder's videos of one
/// picture size and frame rate. Each video is marked with its copy's key.
pub fn copies(videos: &mut [Value], listing: &str) -> Vec<Value> {
    static ATMOS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)atmos|杜比全景声|全景声").expect("valid regex"));
    static SURROUND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?:^|[^0-9])([57])\.1(?:[^0-9]|$)").expect("valid regex"));
    static STEREO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?:aac|ddp?|eac3|ac3)[ ._-]?2\.0|(?:^|[^0-9])2\.0(?:[^0-9]|$)|立体声").expect("valid regex"));
    static FAST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?:50|60|120)\s*(?:fps|帧)").expect("valid regex"));
    static VISION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)杜比视界|dolby[ ._-]?vision|dovi|(?:^|[^a-z])dv(?:[^a-z]|$)").expect("valid regex"));
    static HDR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)hdr|臻彩").expect("valid regex"));
    static CODEC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(h\.?265|hevc|x265)|(h\.?264|x264|avc)|(av1)").expect("valid regex"));
    let folder = |video: &Value| video["path"].as_str().unwrap_or("").rsplit_once('/').map_or(String::new(), |(folder, _)| folder.to_owned());
    let fast = |video: &Value| video["fps"].as_f64().is_some_and(|fps| fps >= 49.0) || (video["fps"].as_f64().is_none() && FAST.is_match(video["path"].as_str().unwrap_or("")));
    let mut folders: Vec<String> = Vec::new();
    for video in videos.iter() { let name = folder(video); if !folders.contains(&name) { folders.push(name); } }
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for name in folders {
        let members: Vec<usize> = (0..videos.len()).filter(|i| folder(&videos[*i]) == name).collect();
        let mut numbers: Vec<(Option<u64>, u64)> = members.iter().filter_map(|i| Some((videos[*i]["season"].as_u64(), videos[*i]["episode"].as_u64()?))).collect();
        let numbered = numbers.len();
        numbers.sort_unstable(); numbers.dedup();
        // One file per episode is one set, whatever each file measures: the drive's
        // measurements vary within a set, and late episodes are often added in another
        // quality. Only a folder holding an episode twice holds two sets, told apart by picture.
        let twice = numbers.len() < numbered;
        let only: Vec<&'static str> = { let mut seen: Vec<&'static str> = members.iter().filter_map(|i| resolution(&videos[*i])).collect(); seen.sort_unstable(); seen.dedup(); seen };
        for index in members {
            let key = if twice {
                // A video the drive did not measure takes the folder's picture size when there is just one.
                let size = resolution(&videos[index]).or((only.len() == 1).then(|| only[0]));
                format!("{name}|{}|{}", size.unwrap_or(""), if fast(&videos[index]) { "60" } else { "" })
            } else { format!("{name}||") };
            videos[index]["copy"] = json!(key);
            if !groups.contains_key(&key) { order.push(key.clone()); }
            groups.entry(key).or_default().push(index);
        }
    }
    // The listing's own title describes the share's one copy; with several it could mean
    // any of them, and is believed only about sound, which nothing else reveals.
    let whole = listing;
    let listing = if order.len() == 1 { listing } else { "" };
    // A trailer or extras folder beside the episodes is not a copy of the series.
    if order.iter().any(|key| groups[key].iter().any(|i| videos[*i]["episode"].is_u64())) {
        order.retain(|key| groups[key].iter().any(|i| videos[*i]["episode"].is_u64()));
    }
    let mut found: Vec<Value> = order.into_iter().map(|key| {
        let members: Vec<&Value> = groups[&key].iter().map(|i| &videos[*i]).collect();
        let first = members[0];
        let said = visible(&format!("{} {} {listing}", first["path"].as_str().unwrap_or(""), members.last().and_then(|v| v["path"].as_str()).unwrap_or("")));
        let folder = key.rsplitn(3, '|').nth(2).unwrap_or("");
        // The set is described by what most of its videos are; the rest are counted beside it.
        let mut pictures: Vec<(&'static str, usize)> = Vec::new();
        for size in members.iter().filter_map(|v| resolution(v)) {
            match pictures.iter_mut().find(|(known, _)| *known == size) { Some(entry) => entry.1 += 1, None => pictures.push((size, 1)) }
        }
        pictures.sort_by(|a, b| b.1.cmp(&a.1));
        let size = pictures.first().map_or("", |(size, _)| size);
        let quick = members.iter().filter(|v| fast(v)).count() * 2 > members.len();
        let typical = |field: &str| members.iter().filter(|v| resolution(v) == Some(size)).find_map(|v| v[field].as_u64().filter(|n| *n > 0));
        let mut episodes: Vec<u64> = members.iter().filter_map(|v| v["episode"].as_u64()).collect();
        episodes.sort_unstable(); episodes.dedup();
        let seasons: std::collections::BTreeSet<u64> = members.iter().filter_map(|v| v["season"].as_u64()).collect();
        let total: u64 = members.iter().filter_map(|v| v["size_bytes"].as_u64()).sum();
        let mut lengths: Vec<u64> = members.iter().filter_map(|v| v["duration"].as_u64()).filter(|d| *d > 0).collect();
        lengths.sort_unstable();
        let measured = lengths.len() == members.len();
        let seconds: u64 = lengths.iter().sum();
        let rate = members.iter().filter(|v| fast(v) == quick).filter_map(|v| v["fps"].as_f64()).find(|fps| *fps > 0.0).map(|fps| fps.round() as u64);
        let sound = [said.as_str(), whole].iter().find_map(|text| if ATMOS.is_match(text) { Some(json!("atmos")) } else if let Some(layout) = SURROUND.captures(text) { Some(json!(format!("{}.1", &layout[1]))) } else if STEREO.is_match(text) { Some(json!("stereo")) } else { None });
        json!({"key": key, "folder": folder, "resolution": if size.is_empty() { Value::Null } else { json!(size) },
            "width": typical("width"), "height": typical("height"),
            "pictures": pictures.iter().map(|(size, count)| json!({"resolution": size, "count": count})).collect::<Vec<_>>(),
            "fps": rate.or(quick.then_some(60)),
            "hdr": if VISION.is_match(&said) { json!("Dolby Vision") } else if HDR.is_match(&said) { json!("HDR") } else { Value::Null },
            "codec": CODEC.captures(&said).map(|c| if c.get(1).is_some() { "H.265" } else if c.get(2).is_some() { "H.264" } else { "AV1" }),
            // Names are the only evidence of sound: the drive measures the picture alone.
            "audio": sound,
            "season": if seasons.len() == 1 { json!(seasons.iter().next()) } else { Value::Null },
            "episodes": episodes, "extras": members.iter().filter(|v| v["episode"].is_null()).count(),
            "video_count": members.len(), "total_bytes": total,
            "minutes": (!lengths.is_empty()).then(|| (lengths[lengths.len() / 2] + 30) / 60),
            "mbps": (measured && seconds > 0).then(|| (total as f64 * 8.0 / seconds as f64 / 100_000.0).round() / 10.0)})
    }).collect();
    found.sort_by(|a, b| b["total_bytes"].as_u64().cmp(&a["total_bytes"].as_u64()));
    found
}
/// Set each copy against the series it belongs to: its season, how many of that
/// season's aired episodes it holds and which are missing. `show` is the series as
/// the metadata service knows it, or null when it does not.
pub fn cover(found: &mut Value, show: &Value, wanted: Option<u64>) {
    let seasons = show["seasons"].as_array().cloned().unwrap_or_default();
    for share in found["shares"].as_array_mut().into_iter().flatten() {
        let named = stated_season(share["title"].as_str().unwrap_or(""));
        for copy in share["copies"].as_array_mut().into_iter().flatten() {
            let season = copy["season"].as_u64().or_else(|| stated_season(&visible(copy["folder"].as_str().unwrap_or("")))).or(named).or(wanted)
                .or((seasons.len() == 1).then(|| seasons[0]["season"].as_u64()).flatten()).unwrap_or(1);
            copy["season"] = json!(season);
            let Some(known) = seasons.iter().find(|s| s["season"].as_u64() == Some(season)) else { continue };
            let (aired, total) = (known["aired"].as_u64().unwrap_or(0), known["episodes"].as_u64().unwrap_or(0));
            let held: Vec<u64> = copy["episodes"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
            let missing: Vec<u64> = (1..=aired).filter(|n| !held.contains(n)).collect();
            copy["aired"] = json!(aired); copy["total"] = json!(total);
            // Numbered past the series' last episode: another cut, whose numbers will not line up.
            let beyond = total > 0 && held.iter().any(|n| *n > total);
            copy["beyond"] = json!(beyond);
            copy["complete"] = json!(aired > 0 && missing.is_empty() && !beyond);
            copy["missing"] = json!(missing);
        }
    }
    if show.is_object() { found["show"] = show.clone(); }
}
/// How well a copy suits a library, best first: every aired episode, then the native
/// frame rate (60 fps copies of dramas are interpolated), then the sharpest picture,
/// then surround sound, then the larger files.
pub fn preference(copy: &Value) -> impl Ord + use<> {
    let held = copy["episodes"].as_array().map_or(0, Vec::len) as u64;
    let covered = match copy["aired"].as_u64() { Some(aired) => aired.saturating_sub(copy["missing"].as_array().map_or(0, Vec::len) as u64), None => held };
    let picture = match copy["resolution"].as_str() { Some("2160p") => 3, Some("1080p") => 2, Some("720p") => 1, _ => 0 };
    let sound = match copy["audio"].as_str() { Some("stereo") => 0, Some(_) => 2, None => 1 };
    // Native rate before sharpness: an interpolated copy is never the one wanted.
    std::cmp::Reverse((copy["complete"] == true, copy["beyond"] != true, covered, copy["fps"].as_u64().is_none_or(|fps| fps < 49), picture, sound, copy["total_bytes"].as_u64().unwrap_or(0)))
}
/// Make an MP4's HEVC video playable on Apple devices: retag its sample entry from
/// "hev1" to "hvc1". The two name the same video; "hev1" only allows the parameter sets
/// to sit in the stream instead of the description, and Apple's players show nothing
/// for it (the sound plays over black). The tag is changed, four bytes in place, only
/// when the description does hold the VPS, SPS and PPS, so "hvc1" is true of the file.
/// Returns how many tracks were retagged.
pub fn retag_hev1(path: &Path) -> Result<usize> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    let length = file.metadata()?.len();
    // The boxes directly inside a span: (kind, body start, end).
    fn boxes(file: &mut std::fs::File, start: u64, end: u64) -> Result<Vec<([u8; 4], u64, u64)>> {
        let (mut found, mut at) = (Vec::new(), start);
        while at + 8 <= end && found.len() < 4096 {
            let mut head = [0u8; 16];
            file.seek(SeekFrom::Start(at))?;
            file.read_exact(&mut head[..8])?;
            let mut size = u32::from_be_bytes(head[..4].try_into()?) as u64;
            let mut body = at + 8;
            if size == 1 { file.read_exact(&mut head[8..])?; size = u64::from_be_bytes(head[8..].try_into()?); body = at + 16; }
            else if size == 0 { size = end - at; }
            if size < body - at || at + size > end { break; }
            found.push((head[4..8].try_into()?, body, at + size));
            at += size;
        }
        Ok(found)
    }
    let mut spans = vec![(0u64, length)];
    for kind in [b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd"] {
        let mut next = Vec::new();
        for (start, end) in spans { for (found, body, stop) in boxes(&mut file, start, end)? { if &found == kind { next.push((body, stop)); } } }
        spans = next;
    }
    let mut retagged = 0;
    for (body, stop) in spans {
        // stsd: version and flags, entry count, then the entries.
        for (kind, entry, end) in boxes(&mut file, body + 8, stop)? {
            if ![*b"hev1", *b"dvhe", *b"hvc1"].contains(&kind) { continue; }
            let mut sets = std::collections::BTreeSet::new();
            // Dolby Vision that cannot be shown as plain HEVC (profile 5) must be named "dvh1".
            let mut exclusive = false;
            for (inner, config, config_end) in boxes(&mut file, entry + 78, end)? {
                if config_end - config > 1 << 20 { continue; }
                let mut record = vec![0u8; (config_end - config) as usize];
                file.seek(SeekFrom::Start(config))?;
                file.read_exact(&mut record)?;
                if &inner == b"dvcC" || &inner == b"dvvC" { exclusive |= crate::media_format::dolby_exclusive(&record); }
                if &inner != b"hvcC" || record.len() < 23 { continue; }
                let mut at = 23;
                for _ in 0..record[22] {
                    if at + 3 > record.len() { break; }
                    sets.insert(record[at] & 0x3f);
                    let count = u16::from_be_bytes([record[at + 1], record[at + 2]]);
                    at += 3;
                    for _ in 0..count { if at + 2 > record.len() { break; } at += 2 + u16::from_be_bytes([record[at], record[at + 1]]) as usize; }
                }
            }
            let complete = [32u8, 33, 34].iter().all(|kind| sets.contains(kind));
            // Parameter sets in the description allow the out-of-band names; without them the entry stays as it is.
            let wanted: [u8; 4] = if exclusive || &kind == b"dvhe" { *b"dvh1" } else { *b"hvc1" };
            if kind != wanted && (complete || &kind == b"hvc1") {
                file.seek(SeekFrom::Start(entry - 4))?;
                file.write_all(&wanted)?;
                retagged += 1;
            }
        }
    }
    if retagged > 0 { file.sync_all()?; }
    Ok(retagged)
}
/// Whether a job holds several videos for the episode this one belongs to.
fn in_parts_of(job: &Job, file: &JobFile) -> bool {
    let number = |f: &JobFile| f.destination.file_name().and_then(|n| n.to_str()).map(|n| (episode(n), crate::scanner::part(n))).filter(|(e, _)| e.1.is_some());
    number(file).is_some_and(|mine| job.files.iter().filter(|other| number(other) == Some(mine)).count() > 1)
}
/// Whether one copy of an episode is worth having over another, each given as its
/// read tracks and its size. In order: a native frame rate beats an interpolated 50 or
/// 60 (always: the member cannot stand the look); then the sharper picture; then a
/// clearly higher bitrate, which for the same episode is a clearly larger file; then
/// the fuller sound; then the larger file. Equal copies are not worth swapping.
pub fn better(new: (&Value, u64), old: (&Value, u64)) -> bool {
    let native = |m: &Value| m["video"]["fps"].as_f64().is_none_or(|fps| fps < 49.0);
    let picture = |m: &Value| { let (w, h) = (m["video"]["width"].as_u64().unwrap_or(0), m["video"]["height"].as_u64().unwrap_or(0));
        if w >= 3000 || h >= 2000 { 4 } else if w >= 1800 || h >= 1000 { 3 } else if w >= 1200 || h >= 700 { 2 } else if w > 0 || h > 0 { 1 } else { 0 } };
    let sound = |m: &Value| m["audio"].as_array().into_iter().flatten().map(|track| {
        let format = track["format"].as_str().unwrap_or("");
        if format.contains("Atmos") || format.contains("DTS:X") { 4 } else if track["channels"].as_u64().unwrap_or(0) > 2 { 3 } else if track["channels"].as_u64().unwrap_or(0) == 2 { 2 } else { 1 }
    }).max().unwrap_or(0);
    if native(new.0) != native(old.0) { return native(new.0); }
    if picture(new.0) != picture(old.0) { return picture(new.0) > picture(old.0); }
    if new.1 as f64 > old.1 as f64 * 1.25 { return true; }
    if old.1 as f64 > new.1 as f64 * 1.25 { return false; }
    if sound(new.0) != sound(old.0) { return sound(new.0) > sound(old.0); }
    new.1 > old.1
}
fn now() -> f64 { crate::storage::now() }
/// Seconds since 1970 as an ISO 8601 UTC time.
fn date(seconds: u64) -> String {
    let (days, rest) = (seconds / 86400, seconds % 86400);
    // Civil date from a day count (Howard Hinnant's algorithm).
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (day, month) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}
fn clean(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if "/\\:*?\"<>|\0".contains(c) || c.is_control() { ' ' } else { c }).collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ").trim_matches(|c: char| c == '.' || c == ' ').to_owned()
}

impl Cloud {
    pub fn new(data: &Path) -> Result<Self> {
        let jobs_file = data.join("cloud-jobs.json");
        let mut jobs: Vec<Job> = std::fs::read(&jobs_file).ok().and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or_default();
        // A job cut off by a restart starts again; finished files are kept and skipped.
        for job in &mut jobs { if job.state == "saving" || job.state == "downloading" || job.state == "retrying" { job.state = "queued".into(); } }
        Ok(Self {
            cookie_file: data.join("quark.json"),
            jobs_file,
            search_url: std::env::var("TORRBABY_LINK_SEARCH").unwrap_or_else(|_| "http://127.0.0.1:8888".into()),
            http: reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).read_timeout(Duration::from_secs(60)).build()?,
            jobs: Mutex::new(jobs),
            progress: AtomicU64::new(0),
            searches: Mutex::new(HashMap::new()),
            wake: tokio::sync::Notify::new(),
            pace: Mutex::new(std::collections::VecDeque::new()),
            cancelled: Mutex::new(std::collections::HashSet::new()),
            probes: Mutex::new(std::fs::read(data.join("cloud-probes.json")).ok().and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or_default()),
            probes_file: data.join("cloud-probes.json"),
            probing: tokio::sync::Mutex::new(()),
            superseded: Mutex::new(std::fs::read(data.join("cloud-superseded.json")).ok().and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or_default()),
            superseded_file: data.join("cloud-superseded.json"),
            rescan: tokio::sync::Notify::new(),
        })
    }
    fn cookie(&self) -> Option<String> {
        let saved: Value = serde_json::from_slice(&std::fs::read(&self.cookie_file).ok()?).ok()?;
        saved["cookie"].as_str().filter(|c| !c.is_empty()).map(str::to_owned)
    }
    pub fn set_cookie(&self, cookie: &str) -> Result<()> {
        use std::io::Write; use std::os::unix::fs::OpenOptionsExt;
        let cookie = cookie.trim().trim_start_matches("Cookie:").trim_start_matches("cookie:").trim();
        ensure!((20..=16384).contains(&cookie.len()) && cookie.contains('=') && !cookie.contains(['\n', '\r']), "That does not look like a Quark login cookie");
        let staged = self.cookie_file.with_extension("json.new");
        let mut file = std::fs::OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(&staged)?;
        file.write_all(&serde_json::to_vec(&json!({"cookie": cookie}))?)?; file.sync_all()?;
        Ok(std::fs::rename(staged, &self.cookie_file)?)
    }
    pub fn clear_cookie(&self) -> Result<()> {
        match std::fs::remove_file(&self.cookie_file) { Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()), _ => Ok(()) }
    }
    /// One request to Quark. Errors carry Quark's own message, never a request header.
    async fn call(&self, path: &str, query: &[(&str, String)], body: Option<Value>, cookie: Option<&str>, desktop: bool) -> Result<Value> {
        let mut url = reqwest::Url::parse(&format!("{API}{path}"))?;
        url.query_pairs_mut().append_pair("pr", "ucpro").append_pair("fr", "pc").append_pair("uc_param_str", "");
        for (key, value) in query { url.query_pairs_mut().append_pair(key, value); }
        let mut request = if let Some(body) = &body { self.http.post(url).json(body) } else { self.http.get(url) };
        request = request.header("User-Agent", if desktop { DESKTOP } else { BROWSER }).header("Referer", "https://pan.quark.cn/").header("Origin", "https://pan.quark.cn")
            .timeout(Duration::from_secs(40));
        if let Some(cookie) = cookie { request = request.header("Cookie", cookie); }
        let response = request.send().await.map_err(|e| anyhow::anyhow!("Quark could not be reached{}", if e.is_timeout() { " in time" } else { "" }))?;
        let answer: Value = response.json().await.map_err(|_| anyhow::anyhow!("Quark did not answer as expected"))?;
        if answer["code"].as_i64().unwrap_or(-1) != 0 {
            bail!("Quark: {} ({})", answer["message"].as_str().unwrap_or("request refused"), answer["code"]);
        }
        Ok(answer)
    }
    async fn pages(&self, path: &str, query: &[(&str, String)], cookie: Option<&str>) -> Result<Vec<Value>> {
        let mut found = Vec::new();
        for page in 1..=20u32 {
            let mut query: Vec<(&str, String)> = query.to_vec();
            query.extend([("_page", page.to_string()), ("_size", "100".into()), ("_fetch_total", "1".into()), ("_sort", "file_type:asc,file_name:asc".into())]);
            let answer = self.call(path, &query, None, cookie, false).await?;
            let items = answer["data"]["list"].as_array().cloned().unwrap_or_default();
            let last = items.len() < 100;
            found.extend(items);
            if last { break; }
        }
        Ok(found)
    }
    pub async fn status(&self) -> Result<Value> {
        let Some(cookie) = self.cookie() else { return Ok(json!({"configured": false})) };
        Ok(match self.call("/member", &[("fetch_subscribe", "true".into()), ("_ch", "home".into()), ("fetch_identity", "true".into())], None, Some(&cookie), false).await {
            Ok(answer) => json!({"configured": true, "signed_in": true, "member": answer["data"]["member_type"],
                "used_bytes": answer["data"]["use_capacity"], "total_bytes": answer["data"]["total_capacity"]}),
            // A saved cookie that Quark no longer accepts: the login has expired.
            Err(error) => json!({"configured": true, "signed_in": false, "error": error.to_string()}),
        })
    }
    /// The share's access token and title. Needs no login.
    async fn open(&self, url: &str) -> Result<(String, String, String)> {
        let id = SHARE.captures(url).context("Not a Quark share link")?[1].to_owned();
        let code = reqwest::Url::parse(url).ok().and_then(|u| u.query_pairs().find(|(k, _)| k == "pwd").map(|(_, v)| v.into_owned())).unwrap_or_default();
        let answer = self.call("/share/sharepage/token", &[], Some(json!({"pwd_id": id, "passcode": code})), None, false).await?;
        let token = answer["data"]["stoken"].as_str().context("This share is no longer available")?.to_owned();
        Ok((id, token, answer["data"]["title"].as_str().unwrap_or("").to_owned()))
    }
    /// Every file in a share, as (path inside the share, listing entry).
    async fn contents(&self, id: &str, token: &str) -> Result<Vec<(String, Value)>> {
        let mut files = Vec::new();
        let mut folders = vec![("0".to_owned(), String::new(), 0u8)];
        while let Some((folder, prefix, depth)) = folders.pop() {
            for entry in self.pages("/share/sharepage/detail", &[("pwd_id", id.into()), ("stoken", token.into()), ("pdir_fid", folder.clone()), ("force", "0".into())], None).await? {
                let name = entry["file_name"].as_str().unwrap_or("").to_owned();
                if entry["dir"] == true {
                    if depth < 4 { folders.push((entry["fid"].as_str().unwrap_or("").to_owned(), format!("{prefix}{name}/"), depth + 1)); }
                } else { files.push((format!("{prefix}{name}"), entry)); }
            }
            ensure!(files.len() <= 5000, "This share holds too many files");
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(files)
    }
    /// What a share really contains, or None when it holds no video (most do not).
    async fn inspect(&self, url: &str, posted: &str, note: &str) -> Option<Value> {
        let (id, token, title) = self.open(url).await.ok()?;
        let files = self.contents(&id, &token).await.ok()?;
        let (videos, copies) = Self::videos(&files, note);
        if videos.is_empty() { return None; }
        // When the share itself was made: the search service only knows when it found the link.
        let made = self.call("/share/sharepage/detail", &[("pwd_id", id.clone()), ("stoken", token.clone()), ("pdir_fid", "0".into()), ("force", "0".into()),
            ("_page", "1".into()), ("_size", "1".into()), ("_fetch_share", "1".into())], None, None, false).await.ok()
            .and_then(|answer| answer["data"]["share"]["created_at"].as_u64()).map(|ms| date(ms / 1000));
        let posted = made.as_deref().unwrap_or(posted);
        // Sharers often leave the folder with a meaningless name; the listing's own title says more.
        let title = if !note.trim().is_empty() { note } else { &title };
        let total: u64 = videos.iter().filter_map(|v| v["size_bytes"].as_u64()).sum();
        Some(json!({"url": format!("https://pan.quark.cn/s/{id}{}", url.split_once('?').map(|(_, q)| format!("?{q}")).unwrap_or_default()),
            "title": title, "note": note, "posted": posted,
            "video_count": videos.len(), "total_bytes": total, "copies": copies, "files": videos.into_iter().take(400).collect::<Vec<_>>()}))
    }
    /// A share's videos as results describe them, and the copies they form.
    fn videos(files: &[(String, Value)], listing: &str) -> (Vec<Value>, Vec<Value>) {
        let mut videos: Vec<Value> = files.iter().filter(|(path, entry)| VIDEO.is_match(path) && entry["size"].as_u64().unwrap_or(0) >= 50_000_000).map(|(path, entry)| {
            let (season, number) = episode(path);
            json!({"path": path, "name": visible(path.rsplit('/').next().unwrap_or(path)), "size_bytes": entry["size"], "season": season, "episode": number,
                "width": entry["video_width"], "height": entry["video_height"], "fps": entry["fps"], "duration": entry["duration"]})
        }).collect();
        let copies = copies(&mut videos, listing);
        (videos, copies)
    }
    /// Ways a title may be written in a listing: as typed, without its separators
    /// ("星汉灿烂·月升沧海" is as often "星汉灿烂月升沧海"), and its first part alone.
    fn spellings(query: &str) -> Vec<String> {
        const SEPARATORS: &[char] = &['·', '•', '・', ':', '：', '-', '—', '－', '_', '/', '|', '，', ',', '。', '.', '《', '》', '「', '」', '(', ')', '（', '）', '[', ']', '【', '】'];
        let joined: String = query.chars().filter(|c| !SEPARATORS.contains(c) && !c.is_whitespace()).collect();
        let first = query.split(|c: char| SEPARATORS.contains(&c)).map(str::trim).find(|part| part.chars().count() >= 2).unwrap_or("").to_owned();
        let mut all = vec![query.to_owned(), joined, first];
        all.retain(|s| !s.is_empty());
        let mut seen = std::collections::HashSet::new();
        all.retain(|s| seen.insert(s.clone()));
        all
    }
    async fn listed(&self, words: &str) -> Result<Vec<(String, String, String)>> {
        let mut url = reqwest::Url::parse(&format!("{}/api/search", self.search_url))?;
        url.query_pairs_mut().append_pair("kw", words).append_pair("res", "merge").append_pair("src", "plugin").append_pair("cloud_types", "quark");
        let answer: Value = self.http.get(url).timeout(Duration::from_secs(40)).send().await.map_err(|_| anyhow::anyhow!("The link search service is not running"))?
            .json().await.map_err(|_| anyhow::anyhow!("The link search service did not answer as expected"))?;
        let merged = if answer["data"].is_object() { &answer["data"]["merged_by_type"] } else { &answer["merged_by_type"] };
        Ok(merged["quark"].as_array().into_iter().flatten().filter_map(|row| {
            let url = row["url"].as_str().filter(|u| SHARE.is_match(u))?.to_owned();
            Some((url, row["datetime"].as_str().unwrap_or("").to_owned(), row["note"].as_str().unwrap_or("").to_owned()))
        }).collect())
    }
    /// Shares for a title that hold real video. The same release is shared by many
    /// people, so identical ones are merged; the answer comes back within a time limit
    /// with whatever has been checked, and is reused for half an hour.
    pub async fn search(&self, query: &str) -> Result<Value> {
        let query = query.split_whitespace().collect::<Vec<_>>().join(" ");
        ensure!((1..=100).contains(&query.chars().count()), "Enter a title to search for");
        if let Some((at, found)) = self.searches.lock().map_err(|_| anyhow::anyhow!("Search unavailable"))?.get(&query) {
            if at.elapsed() < Duration::from_secs(1800) { return Ok(found.clone()); }
        }
        let mut rows = Vec::new();
        let mut failure = None;
        // Listings for the title as typed come first; other spellings add to them.
        let spellings = Self::spellings(&query);
        for found in futures_util::future::join_all(spellings.iter().map(|words| self.listed(words))).await {
            match found { Ok(found) => rows.extend(found), Err(error) => failure = Some(error) }
        }
        // The time limit covers opening the shares, not the wait for the listings.
        let started = Instant::now();
        if rows.is_empty() { if let Some(error) = failure { return Err(error); } }
        let listed = rows.len();
        let mut seen = std::collections::HashSet::new();
        rows.retain(|(url, _, _)| seen.insert(SHARE.captures(url).map(|c| c[1].to_owned())));
        rows.truncate(80);
        // Nearly every listed link is dead or holds something else, so each is opened.
        // Opening stops at the time limit or once there is a good choice of releases.
        use futures_util::StreamExt;
        let mut opened = futures_util::stream::iter(rows.into_iter().map(|(url, posted, note)| async move { self.inspect(&url, &posted, &note).await })).buffer_unordered(8);
        let mut releases: Vec<Value> = Vec::new();
        let mut checked = 0usize;
        while started.elapsed() < Duration::from_secs(20) {
            let Ok(next) = tokio::time::timeout(Duration::from_secs(20).saturating_sub(started.elapsed()), opened.next()).await else { break };
            let Some(share) = next else { break };
            checked += 1;
            let Some(mut share) = share else { continue };
            // Same number of videos and the same total size: the same release, shared again.
            if let Some(same) = releases.iter_mut().find(|r| r["video_count"] == share["video_count"] && r["total_bytes"] == share["total_bytes"]) {
                let (newer, older) = if share["posted"].as_str() > same["posted"].as_str() { (&mut share, same.clone()) } else { (&mut same.clone(), share.clone()) };
                let mut mirrors = newer["mirrors"].as_array().cloned().unwrap_or_default();
                mirrors.push(older["url"].clone());
                mirrors.extend(older["mirrors"].as_array().cloned().unwrap_or_default());
                newer["mirrors"] = json!(mirrors);
                *same = newer.clone();
            } else { releases.push(share); }
            if releases.len() >= 12 { break; }
        }
        drop(opened);
        releases.sort_by(|a, b| b["total_bytes"].as_u64().cmp(&a["total_bytes"].as_u64()));
        let found = json!({"query": query, "listed": listed, "checked": checked, "shares": releases});
        // A search that ran out of time with nothing to show is not worth remembering.
        if found["shares"].as_array().is_some_and(|s| !s.is_empty()) {
            let mut cache = self.searches.lock().map_err(|_| anyhow::anyhow!("Search unavailable"))?;
            if cache.len() >= 64 { cache.clear(); }
            cache.insert(query, (Instant::now(), found.clone()));
        }
        Ok(found)
    }
    fn probe_key(url: &str, copy: &str) -> Option<String> { Some(format!("{}|{copy}", &SHARE.captures(url)?[1])) }
    /// What an earlier read of this copy found, if there was one.
    pub fn measured(&self, url: &str, copy: &str) -> Option<Value> {
        self.probes.lock().ok()?.get(&Self::probe_key(url, copy)?).cloned()
    }
    /// Add what has been read of each copy to a search's results. Read tracks outrank
    /// what names claim: the copy's sound, HDR format and codec are taken from them.
    pub fn attach_measured(&self, found: &mut Value) {
        for share in found["shares"].as_array_mut().into_iter().flatten() {
            let url = share["url"].as_str().unwrap_or("").to_owned();
            for copy in share["copies"].as_array_mut().into_iter().flatten() {
                let Some(measured) = self.measured(&url, copy["key"].as_str().unwrap_or("")) else { continue };
                // The fullest track speaks for the copy.
                let best = measured["audio"].as_array().into_iter().flatten().max_by_key(|track| (track["channels"].as_u64().unwrap_or(0), track["format"].as_str().is_some_and(|f| f.contains("Atmos"))));
                if let Some(best) = best {
                    copy["audio"] = if best["format"].as_str().is_some_and(|f| f.contains("Atmos") || f.contains("DTS:X")) { json!("atmos") }
                        else if best["channels"].as_u64().unwrap_or(0) > 2 { best["layout"].clone() } else { json!("stereo") };
                }
                if measured["video"].is_object() {
                    copy["hdr"] = measured["video"]["hdr"].clone();
                    if measured["video"]["codec"].as_str().is_some_and(|c| !c.is_empty()) { copy["codec"] = measured["video"]["codec"].clone(); }
                }
                copy["measured"] = measured;
            }
        }
    }
    /// Read the tracks of a copy's first episode without downloading it: the video is
    /// saved to the member's drive for a moment, its first and last few megabytes are
    /// fetched, and the bundled reader is run on those. Remembered, so each copy is read once.
    pub async fn probe(&self, url: &str, copy: &str) -> Result<Value> {
        let key = Self::probe_key(url, copy).context("Not a Quark share link")?;
        let _turn = self.probing.lock().await;
        if let Some(known) = self.measured(url, copy) { return Ok(known); }
        let cookie = self.cookie().context("Sign in to Quark in Settings first")?;
        let (id, token, _) = self.open(url).await?;
        let contents = self.contents(&id, &token).await?;
        let mut mine: Vec<Value> = Self::videos(&contents, "").0.into_iter().filter(|video| copy.is_empty() || video["copy"] == copy).collect();
        mine.sort_by_key(|video| (video["episode"].is_null(), video["episode"].as_u64()));
        let path = mine.first().and_then(|video| video["path"].as_str()).context("This copy is no longer in the share")?.to_owned();
        let entry = contents.iter().find(|(listed, _)| *listed == path).map(|(_, entry)| entry.clone()).context("This copy is no longer in the share")?;
        let size = entry["size"].as_u64().unwrap_or(0);
        let staging = self.folder(&cookie, "0", STAGING).await?;
        let mine = self.folder(&cookie, &staging, &format!("probe-{}", uuid::Uuid::new_v4().simple())).await?;
        let extension = path.rsplit('.').next().unwrap_or("mkv").to_ascii_lowercase();
        let local = std::env::temp_dir().join(format!("torrbaby-probe-{}.{extension}", uuid::Uuid::new_v4().simple()));
        let read = async {
            let saved = self.call("/share/sharepage/save", &[], Some(json!({"fid_list": [entry["fid"]], "fid_token_list": [entry["share_fid_token"]],
                "to_pdir_fid": mine, "pwd_id": id, "stoken": token, "pdir_fid": "0", "scene": "link"})), Some(&cookie), false).await?;
            let task = saved["data"]["task_id"].as_str().context("Quark did not start the save")?.to_owned();
            let mut finished = false;
            for attempt in 0..60u32 {
                let state = self.call("/task", &[("task_id", task.clone()), ("retry_index", attempt.to_string())], None, Some(&cookie), false).await?;
                if state["data"]["status"] == 2 { finished = true; break; }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            ensure!(finished, "Quark did not finish saving the video");
            let listed = self.pages("/file/sort", &[("pdir_fid", mine.clone())], Some(&cookie)).await?;
            let fid = listed.iter().find_map(|e| e["fid"].as_str()).context("A saved file is missing from the drive")?;
            let link = self.call("/file/download", &[], Some(json!({"fids": [fid]})), Some(&cookie), true).await?;
            let link = link["data"][0]["download_url"].as_str().context("Quark gave no download link")?.to_owned();
            self.sample(&cookie, &link, size, &local).await
        }.await;
        let _ = tokio::fs::remove_file(&local).await;
        let _ = self.call("/file/delete", &[], Some(json!({"action_type": 2, "filelist": [mine], "exclude_fids": []})), Some(&cookie), false).await;
        let mut measured = read?;
        measured["file"] = json!(path.rsplit('/').next().unwrap_or(&path));
        if let Ok(mut probes) = self.probes.lock() {
            if probes.len() >= 5000 { probes.clear(); }
            probes.insert(key, measured.clone());
            if let Ok(raw) = serde_json::to_vec(&*probes) {
                let staged = self.probes_file.with_extension("json.new");
                if std::fs::write(&staged, raw).is_ok() { let _ = std::fs::rename(staged, &self.probes_file); }
            }
        }
        Ok(measured)
    }
    /// The tracks of a video on the drive, from its first and last few megabytes alone.
    /// MKV describes itself at the start, MP4 often at the end: both ends are fetched
    /// into a file of the full length, the middle left a hole, and the reader is run on it.
    async fn sample(&self, cookie: &str, link: &str, size: u64, local: &Path) -> Result<Value> {
        use tokio::io::{AsyncSeekExt, AsyncWriteExt};
        const EDGE: u64 = 4 << 20;
        let mut file = tokio::fs::File::create(local).await?;
        file.set_len(size).await?;
        let mut ranges = vec![(0, EDGE.min(size))];
        if size > 2 * EDGE { ranges.push((size - EDGE, size)); }
        for (start, end) in ranges {
            let response = self.http.get(link).header("User-Agent", DESKTOP).header("Referer", "https://pan.quark.cn/").header("Cookie", cookie)
                .header("Range", format!("bytes={start}-{}", end - 1)).timeout(Duration::from_secs(60)).send().await.map_err(|_| anyhow::anyhow!("Quark could not be reached"))?;
            ensure!(response.status().is_success(), "Quark refused the download");
            let bytes = response.bytes().await.map_err(|_| anyhow::anyhow!("Quark stopped answering"))?;
            file.seek(std::io::SeekFrom::Start(start)).await?;
            file.write_all(&bytes[..bytes.len().min((end - start) as usize)]).await?;
        }
        file.flush().await?;
        drop(file);
        let target = local.to_owned();
        tokio::task::spawn_blocking(move || crate::media_format::outline(&target)).await?
    }
    /// Other videos of the same episode already in the folder a video is headed for.
    fn rivals(destination: &Path) -> Vec<PathBuf> {
        let Some(name) = destination.file_name().and_then(|n| n.to_str()) else { return Vec::new() };
        let (season, Some(number)) = episode(name) else { return Vec::new() };
        let Some(folder) = destination.parent() else { return Vec::new() };
        std::fs::read_dir(folder).into_iter().flatten().flatten().map(|entry| entry.path()).filter(|path| {
            let other = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            path.is_file() && VIDEO.is_match(other) && !other.starts_with('.') && { let (s, n) = episode(other); n == Some(number) && s.unwrap_or(1) == season.unwrap_or(1) }
                // Parts of one episode are not copies of each other.
                && crate::scanner::part(other) == crate::scanner::part(name)
        }).collect()
    }
    /// Whether a video on the drive should take the place of the library's copies of its
    /// episode. Decided from the new video's ends, before any of it is downloaded.
    /// An unreadable newcomer never displaces a file that plays.
    async fn outranks(&self, cookie: &str, fid: &str, size: u64, destination: &Path, rivals: &[PathBuf]) -> bool {
        let Ok(link) = self.call("/file/download", &[], Some(json!({"fids": [fid]})), Some(cookie), true).await else { return false };
        let Some(link) = link["data"][0]["download_url"].as_str() else { return false };
        let extension = destination.extension().and_then(|e| e.to_str()).unwrap_or("mkv");
        let local = std::env::temp_dir().join(format!("torrbaby-compare-{}.{extension}", uuid::Uuid::new_v4().simple()));
        let new = self.sample(cookie, link, size, &local).await;
        let _ = tokio::fs::remove_file(&local).await;
        let Ok(new) = new else { return false };
        for rival in rivals {
            let (path, length) = (rival.clone(), std::fs::metadata(rival).map(|m| m.len()).unwrap_or(0));
            // A library copy that cannot be read is no reason to keep it.
            let Ok(Ok(old)) = tokio::task::spawn_blocking(move || crate::media_format::outline(&path)).await else { continue };
            if !better((&new, size), (&old, length)) { return false; }
        }
        true
    }
    /// The library's title for the series a job belongs to: taken from a file already
    /// in the job's folder, else from the metadata service by the job's title.
    async fn entity(&self, s: &Service, job: &Job) -> Option<String> {
        let folder = job.files.first()?.destination.parent()?.to_owned();
        let known = s.db.lock().ok().and_then(|db| db.documents("assets").ok()).and_then(|assets| assets.iter()
            .find(|asset| asset["path"].as_str().is_some_and(|path| Path::new(path).parent() == Some(folder.as_path())) && asset["entity_id"].as_str().is_some_and(|id| id.starts_with("tmdb:tv:")))
            .and_then(|asset| asset["entity_id"].as_str().map(str::to_owned)));
        if known.is_some() { return known; }
        let show = s.metadata.series_episodes(&job.title).await.ok()?;
        show["provider_id"].as_str().map(|id| format!("tmdb:tv:{id}"))
    }
    /// Every file the library has mapped to the episode a video is headed for, other than
    /// that video itself: finished or not, in a torrent or loose.
    fn mapped(s: &Service, entity: Option<&str>, destination: &Path) -> Vec<Value> {
        let (Some(entity), Some(name)) = (entity, destination.file_name().and_then(|n| n.to_str())) else { return Vec::new() };
        let (season, Some(number)) = episode(name) else { return Vec::new() };
        let Some(assets) = s.db.lock().ok().and_then(|db| db.documents("assets").ok()) else { return Vec::new() };
        assets.into_iter().filter(|asset| asset["entity_id"] == entity && asset["role"] != "extra" && asset["episode"].as_u64() == Some(number)
            && asset["season"].as_u64().unwrap_or(1) == season.unwrap_or(1) && asset["path"].as_str().is_some_and(|path| Path::new(path) != destination)
            && asset["path"].as_str().map(crate::scanner::part) == Some(crate::scanner::part(name))).collect()
    }
    /// Torrents that are trying to fetch episodes this job has delivered, found by name.
    /// A torrent that has barely started has no files the library could map to an
    /// episode, so the database does not know it; its name ("Pursuit of Jade S01E21 …")
    /// does. Only unfinished torrents whose name gives the show and an explicit SxxEyy
    /// are taken, and they are marked like any other bettered copy.
    async fn supersede_named(&self, s: &Service, entity: Option<&str>, job: &Job) {
        static NUMBERED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])s(\d{1,2})[ ._-]?e(\d{1,3})(?:[^0-9]|$)").expect("valid regex"));
        let Some(entity) = entity else { return };
        let fold = |text: &str| text.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
        let titles: Vec<String> = s.db.lock().ok().and_then(|db| db.documents("entities").ok()).into_iter().flatten().filter(|e| e["id"] == entity)
            .flat_map(|e| [e["title"].as_str().map(fold), e["original_title"].as_str().map(fold)]).flatten().filter(|title| title.chars().count() >= 2).collect();
        if titles.is_empty() { return; }
        // The episodes this job now holds in the library.
        let held: Vec<(u64, u64)> = self.jobs.lock().ok().and_then(|jobs| jobs.iter().find(|j| j.id == job.id).map(|j| j.files.iter()
            .filter(|f| f.done && f.outcome.as_deref() != Some("discarded")).filter_map(|f| { let (season, number) = episode(f.destination.file_name()?.to_str()?); Some((season.unwrap_or(1), number?)) }).collect())).unwrap_or_default();
        if held.is_empty() { return; }
        let Ok(torrents) = crate::organizer::torrents(s).await else { return };
        for torrent in &torrents {
            if torrent["percent_done"].as_f64().unwrap_or(1.0) >= 1.0 { continue; }
            let (Some(hash), Some(name)) = (torrent["hash_string"].as_str(), torrent["name"].as_str()) else { continue };
            let Some(found) = NUMBERED.captures(name) else { continue };
            let (Ok(season), Ok(number)) = (found[1].parse::<u64>(), found[2].parse::<u64>()) else { continue };
            if !held.contains(&(season, number)) { continue; }
            let named = fold(name);
            if !titles.iter().any(|title| named.contains(title.as_str())) { continue; }
            self.supersede(hash, "", false);
        }
    }
    fn supersede(&self, hash: &str, path: &str, outclassed: bool) {
        let Ok(mut all) = self.superseded.lock() else { return };
        let entry = all.entry(hash.to_ascii_lowercase()).or_default();
        if !entry.paths.iter().any(|known| known == path) { entry.paths.push(path.to_owned()); }
        entry.outclassed |= outclassed;
        self.persist_superseded(&all);
    }
    fn persist_superseded(&self, all: &HashMap<String, Superseded>) {
        if let Ok(raw) = serde_json::to_vec(all) {
            let staged = self.superseded_file.with_extension("json.new");
            if std::fs::write(&staged, raw).is_ok() { let _ = std::fs::rename(staged, &self.superseded_file); }
        }
    }
    /// Remove, with their files, the torrents whose episodes all have better copies now,
    /// when that is harmless: the torrent has finished seeding; or it never finished
    /// downloading and either has barely started (under 5%) or promises no sharper a
    /// picture than the copy that replaced it; or it is going nowhere, with nobody
    /// connected and no activity for an hour, or with nothing ever uploaded. One actively seeding waits until it is
    /// done, and one holding an episode with no better copy is left alone.
    pub async fn reap(&self, s: &Service) {
        let waiting = self.superseded.lock().map(|all| all.clone()).unwrap_or_default();
        if waiting.is_empty() { return; }
        let Ok(torrents) = crate::organizer::torrents(s).await else { return };
        let Some(assets) = s.db.lock().ok().and_then(|db| db.documents("assets").ok()) else { return };
        let Ok(config) = s.effective_config() else { return };
        let drives = crate::drives::Drives::of(&config);
        let online = drives.online();
        let mut settled: Vec<String> = Vec::new();
        for (hash, entry) in &waiting {
            let Some(torrent) = torrents.iter().find(|t| t["hash_string"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(hash))) else { settled.push(hash.clone()); continue };
            // Stopped because its drive is away, not because it finished: wait for the drive.
            if drives.offline(Path::new(torrent["download_dir"].as_str().unwrap_or("")), &online) { continue; }
            // Being moved to another drive, or stopped by Torrbaby until its drive returned.
            if crate::drive_moves::moving_torrent(s, hash) || held_for_drive(s, hash) { continue; }
            let whole = assets.iter().filter(|asset| asset["torrent_hash"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(hash)) && asset["role"] != "extra")
                .all(|asset| asset["path"].as_str().is_some_and(|path| entry.paths.iter().any(|known| known == path)));
            if !whole { continue; }
            let done = torrent["percent_done"].as_f64().unwrap_or(0.0);
            // Seeding that will never finish is not worth waiting for: nobody connected,
            // nothing sent, and no activity for an hour. A torrent from the member's private
            // site is the exception; there the seeding time is owed, so it is waited out.
            let owed = torrent["labels"].as_array().is_some_and(|labels| labels.iter().any(|label| label == crate::torrent_search::PRIVATE_LABEL));
            let quiet = torrent["peers_connected"].as_u64().unwrap_or(0) == 0 && torrent["rate_upload"].as_u64().unwrap_or(0) == 0
                && torrent["activity_date"].as_f64().is_some_and(|last| now() - last > 3600.0);
            // Nothing ever uploaded is the other sign: nobody has wanted it.
            let unwanted = torrent["uploaded_ever"].as_u64().unwrap_or(0) == 0;
            let harmless = if done >= 1.0 { torrent["status"].as_u64() == Some(0) || ((quiet || unwanted) && !owed) } else { done < 0.05 || entry.outclassed || (quiet && !owed) };
            if !harmless { continue; }
            match crate::organizer::action(s, hash, "remove", &json!({"delete_data": true, "confirm_hash": hash})).await {
                Ok(_) => { tracing::info!(hash, name = torrent["name"].as_str().unwrap_or(""), "Removed a torrent whose episodes all have better copies"); settled.push(hash.clone()); }
                Err(error) => tracing::warn!(hash, %error, "Could not remove a superseded torrent"),
            }
        }
        if settled.is_empty() { return; }
        if let Ok(mut all) = self.superseded.lock() { for hash in &settled { all.remove(hash); } self.persist_superseded(&all); }
        if let Err(error) = crate::api::scan_library(s).await { tracing::warn!(%error, "Library scan after removing superseded torrents failed"); }
    }
    fn persist(&self, jobs: &[Job]) {
        if let Ok(raw) = serde_json::to_vec(jobs) {
            let staged = self.jobs_file.with_extension("json.new");
            if std::fs::write(&staged, raw).is_ok() { let _ = std::fs::rename(staged, &self.jobs_file); }
        }
    }
    fn update(&self, id: &str, change: impl FnOnce(&mut Job)) {
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(job) = jobs.iter_mut().find(|j| j.id == id) { change(job); }
            self.persist(&jobs);
        }
    }
    /// Whether a download for this library folder is waiting or under way.
    pub fn busy_with(&self, title: &str) -> bool {
        self.jobs.lock().is_ok_and(|jobs| jobs.iter().any(|job| job.title == title && matches!(job.state.as_str(), "queued" | "saving" | "downloading" | "retrying")))
    }
    pub fn jobs(&self) -> Value {
        let jobs = self.jobs.lock().map(|j| j.clone()).unwrap_or_default();
        let running = self.progress.load(Ordering::Relaxed);
        // Speed over the last several seconds, from how far the running download as a whole
        // has come since it was last asked. Measured across its videos, not within one:
        // a video now takes seconds, and a figure reset at each one read far too low.
        let fetched = jobs.iter().find(|job| job.state == "downloading").map_or(0, |job| job.files.iter().filter(|f| f.done).map(|f| f.size).sum::<u64>() + running);
        // Averaged over the last three minutes, so it is steady and the time left can be believed.
        let speed = self.pace.lock().map(|mut pace| {
            let now = Instant::now();
            if pace.back().is_some_and(|(_, last)| fetched < *last) { pace.clear(); }
            if pace.back().is_none_or(|(at, _)| at.elapsed() >= Duration::from_secs(5)) { pace.push_back((now, fetched)); }
            while pace.front().is_some_and(|(at, _)| at.elapsed() > Duration::from_secs(180)) && pace.len() > 2 { pace.pop_front(); }
            match (pace.front(), pace.back()) {
                (Some((first, from)), Some((last, to))) if last.duration_since(*first).as_secs_f64() >= 5.0 => (to - from) as f64 / last.duration_since(*first).as_secs_f64(),
                _ => 0.0,
            }
        }).unwrap_or(0.0);
        let waiting = jobs.iter().filter(|job| job.state == "queued").count();
        let mut place = 0usize;
        // The running one, then the waiting ones in the order they will run, then the rest, newest first.
        let mut listed: Vec<&Job> = jobs.iter().filter(|job| matches!(job.state.as_str(), "saving" | "downloading" | "retrying")).collect();
        listed.extend(jobs.iter().filter(|job| job.state == "queued"));
        listed.extend(jobs.iter().rev().filter(|job| !matches!(job.state.as_str(), "saving" | "downloading" | "retrying" | "queued")));
        json!({"jobs": listed.into_iter().take(50).map(|job| {
            // 1 for the next to run; null when it is not waiting.
            let position = (job.state == "queued").then(|| { place += 1; place });
            let _ = waiting;
            let total: u64 = job.files.iter().map(|f| f.size).sum();
            let finished: u64 = job.files.iter().filter(|f| f.done).map(|f| f.size).sum();
            let active = job.state == "downloading";
            let done = if active { (finished + running).min(total) } else { finished };
            let rate = if active { speed.max(0.0) as u64 } else { 0 };
            let name = |f: &JobFile| f.destination.file_name().map(|n| n.to_string_lossy().into_owned());
            json!({"id": job.id, "title": job.title, "state": job.state, "files": job.files.len(), "files_done": job.files.iter().filter(|f| f.done).count(),
                "done_bytes": done, "total_bytes": total, "error": job.error, "created": job.created,
                "rate": rate, "eta": (active && rate > 0).then(|| (total - done) / rate),
                // The video being fetched now, and where the job's videos are filed.
                "current": if active { job.files.iter().find(|f| !f.done).and_then(name) } else { None },
                "folder": job.files.first().and_then(|f| f.destination.parent()).map(|p| p.to_string_lossy().into_owned()),
                "attempts": job.attempts, "position": position,
                "replaced": job.files.iter().filter(|f| f.outcome.as_deref() == Some("replaced")).count(),
                "discarded": job.files.iter().filter(|f| f.outcome.as_deref() == Some("discarded")).count()})
        }).collect::<Vec<_>>()})
    }
    /// Queue chosen videos of a share for the library. `files` are paths from a search
    /// result; without them every video in the share is taken.
    pub async fn add(&self, s: &Service, body: &Value) -> Result<Value> {
        ensure!(self.cookie().is_some(), "Sign in to Quark in Settings first");
        let url = body["url"].as_str().context("Missing share link")?;
        let title = clean(body["title"].as_str().unwrap_or(""));
        ensure!(!title.is_empty() && title.chars().count() <= 150, "Missing title");
        let season = body["season"].as_u64().or_else(|| stated_season(body["title"].as_str().unwrap_or(""))).unwrap_or(1).min(99);
        let wanted: Option<Vec<&str>> = body["files"].as_array().map(|list| list.iter().filter_map(Value::as_str).collect());
        let (id, token, _) = self.open(url).await?;
        let contents = self.contents(&id, &token).await?;
        // A result for one copy of a share that holds several takes that copy's videos only.
        let copy: Option<Vec<String>> = body["copy"].as_str().filter(|key| !key.is_empty()).map(|key| {
            let mine: Vec<Value> = Self::videos(&contents, "").0.into_iter().filter(|video| video["copy"] == key).collect();
            // The whole copy means its episodes: a trailer beside them is not wanted unasked.
            let numbered = wanted.is_none() && mine.iter().any(|video| video["episode"].is_u64());
            mine.into_iter().filter(|video| !numbered || video["episode"].is_u64()).filter_map(|video| video["path"].as_str().map(str::to_owned)).collect()
        });
        let chosen: Vec<(String, u64)> = contents.into_iter()
            .filter(|(path, entry)| VIDEO.is_match(path) && entry["size"].as_u64().unwrap_or(0) >= 50_000_000)
            .filter(|(path, _)| wanted.as_ref().is_none_or(|wanted| wanted.contains(&path.as_str())))
            .filter(|(path, _)| copy.as_ref().is_none_or(|copy| copy.contains(path)))
            .map(|(path, entry)| (path, entry["size"].as_u64().unwrap_or(0))).collect();
        ensure!(!chosen.is_empty(), "Nothing in this share matches");
        ensure!(chosen.len() <= 200, "Choose at most 200 files at a time");
        let config = s.effective_config()?;
        // One video without an episode number is a film; anything else is a series.
        let film = chosen.len() == 1 && episode(&chosen[0].0).1.is_none() && body["kind"] != "tv" || body["kind"] == "movie";
        // With its show when the library has it, else on the drive with the most room.
        let bytes: u64 = chosen.iter().map(|(_, size)| size).sum();
        let (entities, assets) = { let db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?; (db.documents("entities")?, db.documents("assets")?) };
        let drives = crate::drives::Drives::of(&config);
        let prefer = if film { None } else { crate::drives::show_titled(&entities, &assets, &title).and_then(|show| drives.show_drive(&assets, &show)) };
        let folder = drives.place(prefer.as_deref(), bytes)?.library(!film).join(&title);
        // Several videos for one episode are its parts and extras (a variety show's
        // "第1期(一)", "第1期纯享"…), not copies of it: each keeps its own label in its name,
        // or they would all be filed as the one episode and replace each other.
        let mut shared: HashMap<(u64, u64), usize> = HashMap::new();
        for (path, _) in &chosen { let (stated, number) = episode(path); if let Some(number) = number { *shared.entry((stated.unwrap_or(season), number)).or_default() += 1; } }
        let files = chosen.into_iter().map(|(path, size)| {
            let extension = path.rsplit('.').next().unwrap_or("mkv").to_ascii_lowercase();
            let (stated, number) = episode(&path);
            let leaf = visible(path.rsplit('/').next().unwrap_or(&path));
            let stem = clean(leaf.rsplit_once('.').map_or(leaf.as_str(), |(stem, _)| stem));
            // A variety show's part is one episode and one part ("S03E05 - Part 1"); its bonus
            // videos keep their own label without a number, and the library lists them as extras.
            let part = (!film).then(|| crate::scanner::part_file_name(&title, stated.unwrap_or(season), &stem, &extension)).flatten();
            let bonus = !film && part.is_none() && crate::scanner::variety_labelled(&stem);
            let name = match number {
                _ if part.is_some() => part.unwrap_or_default(),
                _ if bonus => format!("{title} - {stem}.{extension}"),
                Some(number) if !film && shared.get(&(stated.unwrap_or(season), number)).copied().unwrap_or(0) > 1 =>
                    format!("{title} - S{:02}E{number:02} - {stem}.{extension}", stated.unwrap_or(season)),
                Some(number) if !film => format!("{title} - S{:02}E{number:02}.{extension}", stated.unwrap_or(season)),
                _ if film => format!("{title}.{extension}"),
                _ => format!("{title} - {stem}.{extension}"),
            };
            JobFile { destination: folder.join(name), path, size, done: false, outcome: None }
        }).collect();
        let files: Vec<JobFile> = files;
        // Asked for twice (a second press of Download, or another device): the videos that a
        // waiting or running download of this share already covers are not queued again.
        let covered: Option<String> = self.jobs.lock().ok().and_then(|jobs| jobs.iter()
            .find(|job| matches!(job.state.as_str(), "queued" | "saving" | "downloading" | "retrying") && SHARE.captures(&job.url).map(|c| c[1].to_owned()) == SHARE.captures(url).map(|c| c[1].to_owned())
                && files.iter().all(|wanted| job.files.iter().any(|held| held.path == wanted.path)))
            .map(|job| job.id.clone()));
        if let Some(existing) = covered { return Ok(json!({"job_id": existing, "already_queued": true, "total": files.len()})); }
        // More videos of a share whose download for the same title has not started yet join
        // that download: one show is one row, not two with the same name.
        {
            let mut jobs = self.jobs.lock().map_err(|_| anyhow::anyhow!("Cloud downloads unavailable"))?;
            if let Some(waiting) = jobs.iter_mut().find(|job| job.state == "queued" && job.title == title && job.files.iter().all(|f| !f.done)
                && SHARE.captures(&job.url).map(|c| c[1].to_owned()) == SHARE.captures(url).map(|c| c[1].to_owned())) {
                let before = waiting.files.len();
                for file in &files { if !waiting.files.iter().any(|held| held.path == file.path) { waiting.files.push(file.clone()); } }
                let (id, added) = (waiting.id.clone(), waiting.files.len() - before);
                self.persist(&jobs);
                return Ok(json!({"job_id": id, "merged": true, "added": added, "total": files.len()}));
            }
        }
        // Videos already in place, byte for byte the same length: there is nothing to fetch.
        // When that is all of them nothing is queued, and the member is told instead.
        let total = files.len();
        let had = files.iter().filter(|file| std::fs::metadata(&file.destination).is_ok_and(|found| found.len() == file.size)).count();
        if had == total { return Ok(json!({"job_id": Value::Null, "already_have": had, "total": total})); }
        let job = Job { id: uuid::Uuid::new_v4().simple().to_string(), url: url.to_owned(), title, state: "queued".into(), files, error: None, created: now(), attempts: 0 };
        let id = job.id.clone();
        { let mut jobs = self.jobs.lock().map_err(|_| anyhow::anyhow!("Cloud downloads unavailable"))?; jobs.push(job); self.persist(&jobs); }
        self.wake.notify_one();
        Ok(json!({"job_id": id, "already_have": had, "total": total}))
    }
    /// Remove a job from the list. One that is running is cancelled first: the worker
    /// stops it, clears its partial files and its staging folder, then drops it.
    pub fn dismiss(&self, id: &str) -> Value {
        let (mut removed, mut cancelling) = (false, false);
        if let Ok(mut jobs) = self.jobs.lock() {
            if jobs.iter().any(|job| job.id == id && (job.state == "saving" || job.state == "downloading")) {
                if let Ok(mut cancelled) = self.cancelled.lock() { cancelled.insert(id.to_owned()); cancelling = true; }
            } else {
                let before = jobs.len();
                jobs.retain(|job| job.id != id);
                removed = jobs.len() != before;
                self.persist(&jobs);
            }
        }
        json!({"removed": removed, "cancelling": cancelling})
    }
    /// Put right the MP4s that earlier downloads left tagged "hev1", which Apple devices
    /// show as black. Run by the server itself over every video its downloads delivered,
    /// so files from before the downloader did this are covered without anyone's help.
    /// Checking a file reads only its index; one already right is left untouched.
    async fn retag_delivered(&self) -> usize {
        let delivered: Vec<PathBuf> = self.jobs.lock().map(|jobs| jobs.iter().flat_map(|job| job.files.iter().filter(|f| f.done).map(|f| f.destination.clone())).collect()).unwrap_or_default();
        tokio::task::spawn_blocking(move || delivered.iter()
            .filter(|path| path.extension().and_then(|e| e.to_str()).is_some_and(|e| ["mp4", "mov", "m4v"].contains(&e.to_ascii_lowercase().as_str())) && path.is_file())
            .filter(|path| retag_hev1(path).is_ok_and(|count| count > 0)).count()).await.unwrap_or(0)
    }
    /// Clear the staging folder on the member's drive of anything that belongs to no
    /// download still in hand: a track read cut short by a restart, a job long gone.
    async fn sweep(&self) {
        let Some(cookie) = self.cookie() else { return };
        // Not while a track read is using its own folder there.
        let Ok(_turn) = self.probing.try_lock() else { return };
        let Ok(top) = self.pages("/file/sort", &[("pdir_fid", "0".into())], Some(&cookie)).await else { return };
        let Some(staging) = top.iter().find(|e| e["dir"] == true && e["file_name"] == STAGING).and_then(|e| e["fid"].as_str()) else { return };
        let Ok(entries) = self.pages("/file/sort", &[("pdir_fid", staging.to_owned())], Some(&cookie)).await else { return };
        let kept: Vec<String> = self.jobs.lock().map(|jobs| jobs.iter().filter(|job| job.state != "done").map(|job| job.id.clone()).collect()).unwrap_or_default();
        let stray: Vec<&str> = entries.iter().filter(|e| e["file_name"].as_str().is_some_and(|name| !kept.iter().any(|id| id == name))).filter_map(|e| e["fid"].as_str()).collect();
        if stray.is_empty() { return; }
        tracing::info!(count = stray.len(), "Clearing leftovers from the cloud staging folder");
        let _ = self.call("/file/delete", &[], Some(json!({"action_type": 2, "filelist": stray, "exclude_fids": []})), Some(&cookie), false).await;
    }
    /// Leave nothing of an abandoned job behind: pieces of its unfinished videos, and its folder on the drive.
    async fn discard(&self, job: &Job) {
        for file in job.files.iter().filter(|f| !f.done) {
            for i in 0..CONNECTIONS { let _ = tokio::fs::remove_file(format!("{}.part{i:02}", file.destination.display())).await; }
            let _ = tokio::fs::remove_file(format!("{}.joining", file.destination.display())).await;
            let _ = tokio::fs::remove_file(format!("{}.partial", file.destination.display())).await;
            let _ = tokio::fs::remove_file(format!("{}.progress", file.destination.display())).await;
        }
        // A folder made for this job and still empty goes too; one holding episodes stays.
        if let Some(folder) = job.files.first().and_then(|f| f.destination.parent()) { let _ = tokio::fs::remove_dir(folder).await; }
        let Some(cookie) = self.cookie() else { return };
        let Ok(staging) = self.folder(&cookie, "0", STAGING).await else { return };
        let Ok(entries) = self.pages("/file/sort", &[("pdir_fid", staging)], Some(&cookie)).await else { return };
        if let Some(mine) = entries.iter().find(|e| e["dir"] == true && e["file_name"] == job.id.as_str()).and_then(|e| e["fid"].as_str()) {
            let _ = self.call("/file/delete", &[], Some(json!({"action_type": 2, "filelist": [mine], "exclude_fids": []})), Some(&cookie), false).await;
        }
    }
    async fn folder(&self, cookie: &str, parent: &str, name: &str) -> Result<String> {
        for entry in self.pages("/file/sort", &[("pdir_fid", parent.into())], Some(cookie)).await? {
            if entry["dir"] == true && entry["file_name"] == name { return entry["fid"].as_str().map(str::to_owned).context("Quark folder has no ID"); }
        }
        let made = self.call("/file", &[], Some(json!({"pdir_fid": parent, "file_name": name, "dir_path": "", "dir_init_lock": false})), Some(cookie), false).await?;
        made["data"]["fid"].as_str().map(str::to_owned).context("Quark did not create the folder")
    }
    /// Fetch one file the way the drive's own desktop client does: in pieces of the size
    /// the drive names (about 20 MB), a few at a time, each written straight into the
    /// file at its place. Asking for long open-ended stretches instead is throttled by
    /// the drive to a crawl after a while. The file is kept under a working name until it
    /// is whole; which pieces are had is noted beside it every few seconds, after the
    /// data is safely on disk, so a restart or a retry carries on from there.
    async fn fetch(&self, cookie: &str, fid: &str, size: u64, destination: &Path) -> Result<()> {
        use tokio::io::{AsyncSeekExt, AsyncWriteExt};
        let link = self.call("/file/download", &[], Some(json!({"fids": [fid]})), Some(cookie), true).await?;
        let url = link["data"][0]["download_url"].as_str().context("Quark gave no download link")?.to_owned();
        let piece = link["data"][0]["range_size"].as_u64().filter(|n| (1 << 20..=1 << 30).contains(n)).unwrap_or(20_485_760);
        let parent = destination.parent().context("Invalid destination")?;
        crate::drives::create_media_folder(parent)?;
        // Pieces left by the earlier ways of downloading are of no use to this one.
        for i in 0..8u64 { let _ = tokio::fs::remove_file(format!("{}.part{i:02}", destination.display())).await; }
        let _ = tokio::fs::remove_file(format!("{}.joining", destination.display())).await;
        let (working, notes) = (PathBuf::from(format!("{}.partial", destination.display())), PathBuf::from(format!("{}.progress", destination.display())));
        let count = size.div_ceil(piece).max(1);
        let span = |index: u64| (index * piece, ((index + 1) * piece).min(size));
        // What an earlier attempt noted, if it was for this file cut the same way and the working file is still there.
        let had: std::collections::BTreeSet<u64> = match (tokio::fs::read(&notes).await.ok().and_then(|raw| serde_json::from_slice::<Value>(&raw).ok()), tokio::fs::metadata(&working).await) {
            (Some(noted), Ok(file)) if noted["size"].as_u64() == Some(size) && noted["piece"].as_u64() == Some(piece) && file.len() == size =>
                noted["pieces"].as_array().into_iter().flatten().filter_map(Value::as_u64).filter(|index| *index < count).collect(),
            _ => Default::default(),
        };
        if had.is_empty() {
            let file = tokio::fs::File::create(&working).await?;
            file.set_len(size).await?;
        }
        let wanted: Vec<u64> = (0..count).filter(|index| !had.contains(index)).collect();
        let done = Arc::new(Mutex::new(had));
        let bytes = |pieces: &std::collections::BTreeSet<u64>| pieces.iter().map(|index| { let (start, end) = span(*index); end - start }).sum::<u64>();
        self.progress.store(done.lock().map(|d| bytes(&d)).unwrap_or(0), Ordering::Relaxed);
        let next = AtomicU64::new(0);
        let workers = (0..CONNECTIONS.min(wanted.len() as u64)).map(|_| {
            let (url, working, done, wanted, next) = (url.clone(), working.clone(), done.clone(), &wanted, &next);
            async move {
                let mut file = tokio::fs::OpenOptions::new().write(true).open(&working).await?;
                while let Some(index) = wanted.get(next.fetch_add(1, Ordering::Relaxed) as usize).copied() {
                    let (start, end) = span(index);
                    let mut whole = false;
                    for _attempt in 0..6u32 {
                        let sent = self.http.get(&url).header("User-Agent", DESKTOP).header("Referer", "https://pan.quark.cn/").header("Cookie", cookie)
                            .header("Range", format!("bytes={start}-{}", end - 1)).send().await;
                        let Ok(mut response) = sent else { tokio::time::sleep(Duration::from_secs(3)).await; continue };
                        if !response.status().is_success() { tokio::time::sleep(Duration::from_secs(3)).await; continue; }
                        file.seek(std::io::SeekFrom::Start(start)).await?;
                        let mut got = 0u64;
                        while let Ok(Some(chunk)) = response.chunk().await {
                            let take = (chunk.len() as u64).min(end - start - got) as usize;
                            file.write_all(&chunk[..take]).await?;
                            got += take as u64;
                            self.progress.fetch_add(take as u64, Ordering::Relaxed);
                            if got == end - start { break; }
                        }
                        file.flush().await?;
                        if got == end - start { whole = true; break; }
                        // A piece cut short is fetched again from its start.
                        self.progress.fetch_sub(got, Ordering::Relaxed);
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    ensure!(whole, "The download kept stopping");
                    if let Ok(mut done) = done.lock() { done.insert(index); }
                }
                Ok(())
            }
        });
        // Beside the connections, one note-taker: every several seconds it lists the pieces
        // had, has the file put safely on disk, and only then records that list.
        let noting = async {
            let Ok(file) = tokio::fs::OpenOptions::new().write(true).open(&working).await else { return std::future::pending::<()>().await };
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                let reached: Vec<u64> = done.lock().map(|d| d.iter().copied().collect()).unwrap_or_default();
                if file.sync_data().await.is_ok() { let _ = tokio::fs::write(&notes, serde_json::to_vec(&json!({"size": size, "piece": piece, "pieces": reached})).unwrap_or_default()).await; }
            }
        };
        tokio::select! { fetched = futures_util::future::try_join_all(workers) => { fetched?; }, _ = noting => {} }
        ensure!(tokio::fs::metadata(&working).await?.len() == size && done.lock().is_ok_and(|d| d.len() as u64 == count), "The downloaded file is incomplete");
        tokio::fs::rename(&working, destination).await?;
        let _ = tokio::fs::remove_file(&notes).await;
        // An MP4 whose video is tagged "hev1" is black on Apple devices; the file is ours, so it is put right.
        if destination.extension().and_then(|e| e.to_str()).is_some_and(|e| ["mp4", "mov", "m4v"].contains(&e.to_ascii_lowercase().as_str())) {
            let target = destination.to_owned();
            match tokio::task::spawn_blocking(move || retag_hev1(&target)).await {
                Ok(Ok(count)) if count > 0 => tracing::info!(file = %destination.display(), "Retagged HEVC video from hev1 to hvc1 for Apple players"),
                Ok(Err(error)) => tracing::warn!(file = %destination.display(), %error, "Could not check an MP4's video tag"),
                _ => {}
            }
        }
        Ok(())
    }
    /// Whether a waiting download has been put ahead of this one ("Download next").
    fn overtaken(&self, id: &str) -> bool {
        self.jobs.lock().is_ok_and(|jobs| jobs.iter().position(|job| job.id == id).is_some_and(|mine| jobs[..mine].iter().any(|job| job.state == "queued")))
    }
    /// Run a job. True when it has finished; false when it stepped aside between two
    /// videos for a download that was put ahead of it, to carry on afterwards from
    /// where it stopped.
    async fn run(&self, s: &Service, job: &Job) -> Result<bool> {
        let cookie = self.cookie().context("Sign in to Quark in Settings first")?;
        let pending: Vec<&JobFile> = job.files.iter().filter(|f| !f.done).collect();
        self.update(&job.id, |j| { j.state = "saving".into(); j.error = None; });
        // Save the chosen files into a folder of the member's drive that is this job's alone.
        let (id, token, _) = self.open(&job.url).await?;
        let listed = self.contents(&id, &token).await?;
        let chosen: Vec<&Value> = pending.iter().filter_map(|f| listed.iter().find(|(path, _)| *path == f.path).map(|(_, entry)| entry)).collect();
        ensure!(chosen.len() == pending.len(), "The share no longer holds these files");
        let staging = self.folder(&cookie, "0", STAGING).await?;
        let mine = self.folder(&cookie, &staging, &job.id).await?;
        let already = self.pages("/file/sort", &[("pdir_fid", mine.clone())], Some(&cookie)).await?;
        let missing: Vec<&&Value> = chosen.iter().filter(|entry| !already.iter().any(|have| have["file_name"] == entry["file_name"] && have["size"] == entry["size"])).collect();
        if !missing.is_empty() {
            let saved = self.call("/share/sharepage/save", &[], Some(json!({
                "fid_list": missing.iter().map(|e| e["fid"].clone()).collect::<Vec<_>>(),
                "fid_token_list": missing.iter().map(|e| e["share_fid_token"].clone()).collect::<Vec<_>>(),
                "to_pdir_fid": mine, "pwd_id": id, "stoken": token, "pdir_fid": "0", "scene": "link"})), Some(&cookie), false).await?;
            let task = saved["data"]["task_id"].as_str().context("Quark did not start the save")?.to_owned();
            let mut finished = false;
            for attempt in 0..120u32 {
                let state = self.call("/task", &[("task_id", task.clone()), ("retry_index", attempt.to_string())], None, Some(&cookie), false).await?;
                if state["data"]["status"] == 2 { finished = true; break; }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            ensure!(finished, "Quark did not finish saving the files");
        }
        let mine_files = self.pages("/file/sort", &[("pdir_fid", mine.clone())], Some(&cookie)).await?;
        self.update(&job.id, |j| j.state = "downloading".into());
        self.progress.store(0, Ordering::Relaxed);
        let entity = self.entity(s, job).await;
        // Episodes delivered before a pause or restart may have torrents the database never knew.
        self.supersede_named(s, entity.as_deref(), job).await;
        self.reap(s).await;
        for file in &pending {
            if self.overtaken(&job.id) { return Ok(false); }
            let name = file.path.rsplit('/').next().unwrap_or(&file.path);
            let entry = mine_files.iter().find(|e| e["file_name"] == name && e["size"].as_u64() == Some(file.size)).context("A saved file is missing from the drive")?;
            let present = tokio::fs::metadata(&file.destination).await.is_ok_and(|m| m.len() == file.size);
            let fid = entry["fid"].as_str().unwrap_or("");
            // One copy of each episode is kept, the best. The library's database says where
            // every copy mapped to this episode lives, in a torrent or loose; a newcomer is
            // fetched only when it beats all that play, and then the lesser copies go. This
            // is how a download fills a show's gaps and raises its quality without duplicates.
            let mapped = if present || in_parts_of(job, file) { Vec::new() } else { Self::mapped(s, entity.as_deref(), &file.destination) };
            // A copy on a drive that is not connected cannot be compared, so it is neither a
            // rival nor bettered: both are kept, and the user chooses later.
            let mapped: Vec<Value> = mapped.into_iter().filter(|asset| asset["offline"] != true).collect();
            // The job's own videos are parts or siblings, never rivals of one another; and an
            // episode that comes in parts is not weighed against a whole copy, whose size says nothing about it.
            let key = |f: &JobFile| f.destination.file_name().and_then(|n| n.to_str()).map(|n| (episode(n), crate::scanner::part(n)));
            let in_parts = key(file).is_some_and(|mine| mine.0.1.is_some() && job.files.iter().filter(|other| key(other) == Some(mine)).count() > 1);
            let exists = |path: &PathBuf| path.is_file() && *path != file.destination && !in_parts && !job.files.iter().any(|other| other.destination == *path);
            let mut rivals: Vec<PathBuf> = if present { Vec::new() } else { Self::rivals(&file.destination) };
            rivals.extend(mapped.iter().filter(|asset| asset["available"] == true && asset["complete"] != false).filter_map(|asset| asset["path"].as_str().map(PathBuf::from)));
            rivals.retain(exists);
            rivals.sort(); rivals.dedup();
            let outcome = if present { "added" } else if rivals.is_empty() { self.fetch(&cookie, fid, file.size, &file.destination).await?; "added" }
                else if self.outranks(&cookie, fid, file.size, &file.destination, &rivals).await { self.fetch(&cookie, fid, file.size, &file.destination).await?; "replaced" }
                else { "discarded" };
            if !present && outcome != "discarded" {
                // Lesser loose copies go at once. A torrent's copy goes with its torrent, when that is harmless.
                let sharp = |size: Option<&str>| match size { Some("2160p") => 4, Some("1080p") => 3, Some("720p") => 2, Some(_) => 1, None => 0 };
                let mine = sharp(resolution(&json!({"width": entry["video_width"], "height": entry["video_height"], "path": name})));
                for asset in &mapped {
                    let Some(path) = asset["path"].as_str() else { continue };
                    let hash = asset["torrent_hash"].as_str().filter(|hash| !hash.is_empty() && asset["torrent_present"] != false);
                    let playable = rivals.iter().any(|rival| rival.as_os_str() == path);
                    match hash {
                        Some(hash) => {
                            // A copy that plays was beaten outright; an unfinished one is judged by what its name promises.
                            let promised = sharp(crate::torrent_search::stated_resolution(asset["torrent_name"].as_str().unwrap_or("")).as_deref());
                            self.supersede(hash, path, playable || (mine > 0 && mine >= promised));
                        }
                        None if playable => {
                            tracing::info!(kept = %file.destination.display(), removed = path, "A better copy of an episode replaced a lesser one");
                            let _ = tokio::fs::remove_file(path).await;
                        }
                        None => {}
                    }
                }
                for rival in rivals.iter().filter(|rival| !mapped.iter().any(|asset| asset["path"].as_str() == rival.to_str())) {
                    tracing::info!(kept = %file.destination.display(), removed = %rival.display(), "A better copy of an episode replaced a lesser one");
                    let _ = tokio::fs::remove_file(rival).await;
                }
                self.supersede_named(s, entity.as_deref(), job).await;
                self.reap(s).await;
            }
            let present = present || outcome == "discarded";
            self.progress.store(0, Ordering::Relaxed);
            let path = file.path.clone();
            self.update(&job.id, |j| { j.attempts = 0; if let Some(f) = j.files.iter_mut().find(|f| f.path == path) { f.done = true; f.outcome = Some(outcome.into()); } });
            // Each episode reaches the library as it lands, not when the whole series has.
            // It is scanned while the next one downloads; the scan is not waited for.
            if !present { self.rescan.notify_one(); }
        }
        // The drive is only a staging post: remove the job's copy once it is in the library.
        let _ = self.call("/file/delete", &[], Some(json!({"action_type": 2, "filelist": [mine], "exclude_fids": []})), Some(&cookie), false).await;
        Ok(true)
    }
    /// Run queued jobs one at a time for as long as the server lives.
    /// Run the downloads, and beside them the library scans they ask for: an episode is
    /// added to the library while the next one is already coming down.
    pub async fn work(&self, s: &Service) {
        let scans = async {
            loop {
                self.rescan.notified().await;
                if let Err(error) = crate::api::scan_library(s).await { tracing::warn!(%error, "Library scan after a cloud download failed"); }
            }
        };
        tokio::join!(self.downloads(s), scans);
    }
    async fn downloads(&self, s: &Service) {
        // The first tidy comes a minute after starting, once any interrupted job has been taken up again.
        let mut swept = Instant::now() - Duration::from_secs(3540);
        // Videos delivered before the downloader retagged them itself are put right at start.
        let fixed = self.retag_delivered().await;
        if fixed > 0 { tracing::info!(fixed, "Retagged earlier downloads from hev1 to hvc1"); self.rescan.notify_one(); }
        loop {
            let next = self.jobs.lock().ok().and_then(|jobs| jobs.iter().find(|j| j.state == "queued").cloned());
            let Some(job) = next else {
                // Torrents that were still seeding when their episodes were bettered are let go once they finish.
                self.reap(s).await;
                // Now and then, with nothing downloading, the staging folder is tidied.
                if swept.elapsed() >= Duration::from_secs(3600) { self.sweep().await; swept = Instant::now(); }
                tokio::select! { _ = self.wake.notified() => {}, _ = tokio::time::sleep(Duration::from_secs(60)) => {} }
                continue;
            };
            let stopped = async {
                loop {
                    if self.cancelled.lock().is_ok_and(|mut cancelled| cancelled.remove(&job.id)) { return; }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            };
            let outcome = tokio::select! { outcome = self.run(s, &job) => Some(outcome), _ = stopped => None };
            match outcome {
                None => {
                    self.discard(&job).await;
                    if let Ok(mut jobs) = self.jobs.lock() { jobs.retain(|j| j.id != job.id); self.persist(&jobs); }
                    if let Err(error) = crate::api::scan_library(s).await { tracing::warn!(%error, "Library scan after a cancelled cloud download failed"); }
                }
                // Stepped aside for a download put ahead of it: back in line, where it will be next again.
                Some(Ok(false)) => self.update(&job.id, |j| { j.state = "queued".into(); j.error = None; }),
                Some(Ok(true)) => {
                    self.update(&job.id, |j| { j.state = "done".into(); j.error = None; });
                    if let Err(error) = crate::api::scan_library(s).await { tracing::warn!(%error, "Library scan after a cloud download failed"); }
                }
                Some(Err(error)) => {
                    tracing::warn!(job = job.id, %error, "Cloud download failed");
                    // A dropped connection or a slow answer is ordinary over a long series:
                    // wait and go on from the videos already fetched, a few times, before giving up.
                    let attempts = job.attempts + 1;
                    let again = attempts <= 4 && self.cookie().is_some();
                    self.update(&job.id, |j| { j.attempts = attempts; j.state = if again { "retrying" } else { "failed" }.into(); j.error = Some(error.to_string()); });
                    if again {
                        tokio::time::sleep(Duration::from_secs(30 * attempts as u64)).await;
                        self.update(&job.id, |j| if j.state == "retrying" { j.state = "queued".into(); });
                    }
                }
            }
            self.progress.store(0, Ordering::Relaxed);
        }
    }
    /// Downloads run one at a time in the order they were added. This moves a waiting
    /// one to the head of the line. The one running finishes the video it is on, steps
    /// aside, and carries on after the one moved ahead is done.
    pub fn next(&self, id: &str) -> Value {
        let mut moved = false;
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(index) = jobs.iter().position(|job| job.id == id && job.state == "queued") {
                let job = jobs.remove(index);
                jobs.insert(0, job);
                moved = true;
                self.persist(&jobs);
            }
        }
        json!({"moved": moved})
    }
    pub fn retry(&self, id: &str) -> Value {
        let mut queued = false;
        self.update(id, |job| if job.state == "failed" || job.state == "retrying" { job.state = "queued".into(); job.error = None; job.attempts = 0; queued = true; });
        if queued { self.wake.notify_one(); }
        json!({"queued": queued})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn episode_numbers_are_read_from_the_usual_file_names() {
        assert_eq!(episode("4e291a34/02.mkv"), (None, Some(2)));
        assert_eq!(episode("逐玉.S01E17.2160p.WEB-DL.H265.DDP5.1.mkv"), (Some(1), Some(17)));
        assert_eq!(episode("逐玉 第12集 4K.mp4"), (None, Some(12)));
        assert_eq!(episode("Zhu.Yu.EP05.1080p.mkv"), (None, Some(5)));
        assert_eq!(episode("The.Wandering.Earth.II.2023.BluRay.2160p.DTS.mkv"), (None, None));
        assert_eq!(episode("繁花.2023.4K.60fps.E30.mkv"), (None, Some(30)));
        assert_eq!(episode("九重紫/4K/01.4K.mp4"), (None, Some(1)));
        // Invisible characters threaded through a name do not hide its number.
        assert_eq!(episode("S\u{200b}1\u{200c}|2160p|/\u{200c}/3\u{200b}\u{200b}\u{200c}\u{200d}8\u{200c}\u{200d}.mkv"), (None, Some(38)));
        assert_eq!(episode("0\u{200b}\u{feff}1.mkv"), (None, Some(1)));
        assert_eq!(visible("逐\u{200d}玉"), "逐玉");
        // Dates are not episodes; a variety show's parts all belong to their episode.
        assert_eq!(episode("X/2026.07.02-先导片.mp4"), (None, None));
        assert_eq!(episode("X/2026.07.04-第1期(五).mp4"), (None, Some(1)));
        assert_eq!(episode("X/2026.08.21-第8期(一)-4K.高码率.mp4"), (None, Some(8)));
        assert_eq!(episode("28.4k.mp4"), (None, Some(28)));
        assert_eq!(episode("01 4K.mp4"), (None, Some(1)));
        assert_eq!(episode("12.5.1.mkv"), (None, Some(12)));
    }
    #[test]
    fn a_share_with_several_qualities_is_several_copies() {
        let video = |path: &str, width: u64, fps: f64, size: u64| json!({"path": path, "size_bytes": size, "season": episode(path).0, "episode": episode(path).1,
            "width": if width > 0 { json!(width) } else { Value::Null }, "height": Value::Null, "fps": fps, "duration": 2700});
        let mut videos = vec![video("九重紫/4K/01.4K.mp4", 3840, 25.0, 900_000_000), video("九重紫/4K/02.4K.mp4", 3840, 25.0, 900_000_000), video("九重紫/4K/04.4K.mp4", 0, 25.0, 900_000_000),
            video("九重紫/臻彩视听HDR [60帧率版]/Blossom.S01E01.2160p.H265.HDR.60fps.mkv", 3840, 60.0, 2_000_000_000), video("九重紫/01.mp4", 1920, 25.0, 400_000_000), video("九重紫/花絮.mp4", 1920, 25.0, 400_000_000)];
        let found = copies(&mut videos, "九重紫 全集 杜比全景声");
        // One set per folder, largest first; the extras file stays with its folder's set.
        assert_eq!(found.len(), 3);
        assert_eq!(found[0]["key"], "九重紫/4K||");
        assert_eq!(found[0]["episodes"], json!([1, 2, 4]));
        assert_eq!(found[0]["audio"], "atmos");
        assert_eq!((found[1]["resolution"].as_str(), found[1]["fps"].as_u64(), found[1]["hdr"].as_str(), found[1]["codec"].as_str(), found[1]["season"].as_u64()), (Some("2160p"), Some(60), Some("HDR"), Some("H.265"), Some(1)));
        assert_eq!((found[2]["resolution"].as_str(), found[2]["extras"].as_u64(), found[2]["minutes"].as_u64(), found[2]["mbps"].as_f64()), (Some("1080p"), Some(1), Some(45), Some(1.2)));
        let mut one = vec![video("a/01.mp4", 3840, 25.0, 900_000_000)];
        assert_eq!(copies(&mut one, "尚公主 4K DDP5.1")[0]["audio"], "5.1");
        // Late episodes in another quality are still the same set; an episode held twice is two.
        let mut mixed = vec![video("a/01.mp4", 3840, 25.0, 9), video("a/02.mp4", 3840, 60.0, 9), video("a/03.mp4", 1920, 30.0, 9), video("a/片花.mp4", 1920, 25.0, 9), video("片花/x.mp4", 1920, 25.0, 9)];
        let set = copies(&mut mixed, "");
        assert_eq!((set.len(), set[0]["resolution"].as_str(), set[0]["fps"].as_u64(), set[0]["pictures"].clone()), (1, Some("2160p"), Some(25), json!([{"resolution": "2160p", "count": 2}, {"resolution": "1080p", "count": 2}])));
        let mut twice = vec![video("a/01.mp4", 1920, 25.0, 9), video("a/01 4K.mp4", 3840, 25.0, 9), video("a/02.mp4", 1920, 25.0, 9), video("a/02 4K.mp4", 0, 25.0, 9)];
        assert_eq!(copies(&mut twice, "").iter().map(|c| c["key"].as_str().unwrap_or("").to_owned()).collect::<Vec<_>>(), ["a|1080p|", "a|2160p|"]);
        let mut search = json!({"shares": [{"title": "九重紫", "copies": found}]});
        cover(&mut search, &json!({"title": "九重紫", "seasons": [{"season": 1, "episodes": 34, "aired": 4}]}), None);
        let copy = &search["shares"][0]["copies"][0];
        assert_eq!((copy["season"].as_u64(), copy["aired"].as_u64(), copy["total"].as_u64(), copy["complete"].as_bool()), (Some(1), Some(4), Some(34), Some(false)));
        assert_eq!(copy["missing"], json!([3]));
        assert_eq!(copy["beyond"], false);
    }
    #[test]
    fn the_better_copy_of_an_episode_is_native_rate_then_sharper_then_larger_then_fuller() {
        let copy = |width: u64, fps: f64, format: &str, channels: u64| json!({"video": {"width": width, "height": 0, "fps": fps}, "audio": [{"format": format, "channels": channels}]});
        let (uhd, hd, smooth) = (copy(3840, 25.0, "AAC", 2), copy(1920, 25.0, "AAC", 2), copy(3840, 60.0, "Dolby Atmos", 6));
        assert!(better((&uhd, 1_000), (&hd, 5_000)));
        assert!(!better((&hd, 5_000), (&uhd, 1_000)));
        // An interpolated copy loses to a native one whatever else it offers.
        assert!(!better((&smooth, 9_000), (&hd, 1_000)));
        assert!(better((&hd, 1_000), (&smooth, 9_000)));
        // The same picture: a clearly larger file wins; near enough in size, the fuller sound does.
        assert!(better((&uhd, 2_000), (&copy(3840, 25.0, "Dolby Atmos", 6), 1_000)));
        assert!(better((&copy(3840, 25.0, "Dolby Atmos", 6), 1_000), (&uhd, 1_100)));
        assert!(!better((&uhd, 1_000), (&uhd, 1_000)));
    }
    #[test]
    fn hev1_video_is_retagged_only_when_its_description_holds_the_parameter_sets() -> Result<()> {
        fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> { let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec(); out.extend(kind); out.extend(body); out }
        let dolby = |name: &[u8; 4], tag: &[u8; 4], profile: u64, compatible: u64| {
            let mut config = vec![0u8; 22];
            config.push(3);
            for kind in [32u8, 33, 34] { config.extend([kind, 0, 1, 0, 2, 0xAA, 0xBB]); }
            let mut entry = vec![0u8; 78];
            entry.extend(atom(b"hvcC", &config));
            entry.extend(atom(name, &crate::media_format::dolby_record(1, 0, profile, 6, true, false, true, compatible).expect("a record").1));
            let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
            stsd.extend(atom(tag, &entry));
            atom(b"moov", &atom(b"trak", &atom(b"mdia", &atom(b"minf", &atom(b"stbl", &atom(b"stsd", &stsd))))))
        };
        // Dolby Vision profile 5 must be named dvh1, whatever it arrived as; profile 8 stays plain HEVC.
        let vision = std::env::temp_dir().join(format!("torrbaby-retag-dv-{}.mp4", uuid::Uuid::new_v4()));
        for (tag, profile, compatible, becomes) in [(b"hev1", 5, 0, &b"dvh1"[..]), (b"hvc1", 5, 0, b"dvh1"), (b"dvhe", 5, 0, b"dvh1"), (b"hev1", 8, 1, b"hvc1"), (b"hvc1", 8, 1, b"hvc1")] {
            std::fs::write(&vision, dolby(if profile > 7 { b"dvvC" } else { b"dvcC" }, tag, profile, compatible))?;
            let _ = retag_hev1(&vision)?;
            let after = std::fs::read(&vision)?;
            assert_eq!(&after[60..64], becomes, "{} profile {profile}", String::from_utf8_lossy(tag));
        }
        std::fs::remove_file(&vision)?;
        let build = |sets: &[u8]| {
            let mut config = vec![0u8; 22];
            config.push(sets.len() as u8);
            for kind in sets { config.extend([*kind, 0, 1, 0, 2, 0xAA, 0xBB]); }
            let mut entry = vec![0u8; 78];
            entry.extend(atom(b"hvcC", &config));
            let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
            stsd.extend(atom(b"hev1", &entry));
            let mut file = atom(b"ftyp", b"isom");
            file.extend(atom(b"moov", &atom(b"trak", &atom(b"mdia", &atom(b"minf", &atom(b"stbl", &atom(b"stsd", &stsd)))))));
            file.extend(atom(b"mdat", &[1, 2, 3, 4]));
            file
        };
        let path = std::env::temp_dir().join(format!("torrbaby-retag-{}.mp4", uuid::Uuid::new_v4()));
        std::fs::write(&path, build(&[32, 33, 34]))?;
        assert_eq!(retag_hev1(&path)?, 1);
        let fixed = std::fs::read(&path)?;
        assert!(fixed.windows(4).any(|w| w == b"hvc1") && !fixed.windows(4).any(|w| w == b"hev1"));
        assert_eq!(fixed.len(), build(&[32, 33, 34]).len());
        assert_eq!(retag_hev1(&path)?, 0);
        // Without the sets in the description the tag stays as it is.
        std::fs::write(&path, build(&[33, 34]))?;
        assert_eq!(retag_hev1(&path)?, 0);
        assert!(std::fs::read(&path)?.windows(4).any(|w| w == b"hev1"));
        std::fs::remove_file(&path)?;
        Ok(())
    }
    #[test]
    fn seasons_are_read_from_titles() {
        assert_eq!(stated_season("庆余年 第二季 4K"), Some(2));
        assert_eq!(stated_season("某剧 第十二季"), Some(12));
        assert_eq!(stated_season("Show.S03.2160p"), Some(3));
        assert_eq!(stated_season("Show Season 4"), Some(4));
        assert_eq!(stated_season("九重紫"), None);
        assert_eq!(stated_season("Blossoms Shanghai"), None);
        assert_eq!(title_and_season("庆余年 第二季"), ("庆余年".to_owned(), Some(2)));
        assert_eq!(title_and_season("九重紫"), ("九重紫".to_owned(), None));
    }
    #[test]
    fn titles_are_searched_with_and_without_their_separators() {
        assert_eq!(Cloud::spellings("星汉灿烂·月升沧海"), ["星汉灿烂·月升沧海", "星汉灿烂月升沧海", "星汉灿烂"]);
        assert_eq!(Cloud::spellings("逐玉"), ["逐玉"]);
        assert_eq!(Cloud::spellings("庆余年 第二季"), ["庆余年 第二季", "庆余年第二季"]);
    }
    #[test]
    fn dates_are_iso_times() {
        assert_eq!(date(0), "1970-01-01T00:00:00Z");
        assert_eq!(date(1794424716), "2026-11-11T19:18:36Z");
        assert_eq!(date(951782400), "2000-02-29T00:00:00Z");
    }
    #[test]
    fn names_are_made_safe_for_the_library() {
        assert_eq!(clean("  逐玉: 第一季 / 4K  "), "逐玉 第一季 4K");
        assert_eq!(clean("a\\b*c?.."), "a b c");
    }
}

/// Stopped by the organizer while its drive was away, and not yet started again.
fn held_for_drive(s: &Service, hash: &str) -> bool {
    s.db.lock().ok().and_then(|db| db.state().ok())
        .is_some_and(|state| state["held_for_drive"].as_array().is_some_and(|list| list.iter().any(|h| h.as_str().is_some_and(|h| h.eq_ignore_ascii_case(hash)))))
}
