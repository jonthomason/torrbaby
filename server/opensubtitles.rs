//! English subtitles from OpenSubtitles (api.opensubtitles.com) for titles that have no
//! English or Chinese subtitles of their own: written by people and timed to a release,
//! far better than a transcript of the audio. The title is found by its TMDB id (a movie,
//! or a series with its season and episode) and the file's OpenSubtitles hash, which
//! picks out subtitles made for this very file when there are some.
//!
//! Credentials live in the data directory (opensubtitles.json: api_key, username,
//! password), never in logs. Without them this source is skipped. A free account may
//! download 20 subtitles a day; when they are used up, files wait for the reset rather
//! than being transcribed (see `limited_until`). Reached directly: the search proxy does
//! not carry it, and the site answers from China.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

const API: &str = "https://api.opensubtitles.com/api/v1";
const USER_AGENT: &str = "Torrbaby v1.0";
/// The start of the error a file waits on until the daily downloads reset.
pub const LIMIT_REACHED: &str = "OpenSubtitles daily download limit reached";

#[derive(serde::Deserialize)]
struct Credentials {
    api_key: String,
    #[serde(default)] username: String,
    #[serde(default)] password: String,
    /// False with a paid API package: downloads count against the key's own daily quota
    /// (2,000 on Light), not the account's 20, so no sign-in. True (free mode) by default.
    #[serde(default = "yes")] signed_in: bool,
    /// Also look for Chinese (zh-cn) people wrote before translating English on device:
    /// worth it while the quota is large. Off by default.
    #[serde(default)] chinese: bool,
}
fn yes() -> bool { true }

struct Session { token: String, host: String, at: std::time::Instant }

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
/// Seconds since the epoch when downloads may start again; 0 when they may now.
static LIMITED_UNTIL: Mutex<f64> = Mutex::new(0.0);

pub enum Found {
    /// An English SRT written to `path`.
    Ready(PathBuf),
    /// Nothing suitable on OpenSubtitles (or no credentials): the next source is tried.
    Missing(String),
}

fn credentials(data: &Path) -> Option<Credentials> {
    serde_json::from_slice(&std::fs::read(data.join("opensubtitles.json")).ok()?).ok()
}

pub fn configured(data: &Path) -> bool { credentials(data).is_some() }

/// Whether Chinese subtitles are looked for on OpenSubtitles before translating.
pub fn chinese(data: &Path) -> bool { credentials(data).is_some_and(|c| c.chinese) }

/// When downloads may start again, while the day's are used up.
pub fn limited_until(data: &Path) -> Option<f64> {
    let mut until = LIMITED_UNTIL.lock().map(|u| *u).unwrap_or(0.0);
    if until == 0.0 {
        // Kept across restarts, so a restart does not ask again and again.
        until = std::fs::read_to_string(data.join("opensubtitles-limit.txt")).ok().and_then(|t| t.trim().parse().ok()).unwrap_or(0.0);
        if let Ok(mut u) = LIMITED_UNTIL.lock() { *u = until; }
    }
    (until > crate::storage::now()).then_some(until)
}

fn limit(data: &Path, until: f64) {
    if let Ok(mut u) = LIMITED_UNTIL.lock() { *u = until; }
    let _ = std::fs::write(data.join("opensubtitles-limit.txt"), until.to_string());
    tracing::info!(until, "OpenSubtitles downloads used up for today");
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(Duration::from_secs(30)).connect_timeout(Duration::from_secs(10)).user_agent(USER_AGENT).build()?)
}

async fn session(client: &reqwest::Client, c: &Credentials) -> Result<(String, String)> {
    if let Some(s) = SESSION.lock().ok().and_then(|s| s.as_ref().filter(|s| s.at.elapsed() < Duration::from_secs(12 * 3600)).map(|s| (s.token.clone(), s.host.clone()))) {
        return Ok(s);
    }
    let reply = client.post(format!("{API}/login")).header("Api-Key", &c.api_key).header("Accept", "application/json")
        .json(&json!({"username": c.username, "password": c.password})).send().await.context("OpenSubtitles login")?;
    ensure!(reply.status().is_success(), "OpenSubtitles login refused ({})", reply.status());
    let body: Value = reply.json().await?;
    let token = body["token"].as_str().context("OpenSubtitles login gave no token")?.to_owned();
    let host = body["base_url"].as_str().filter(|h| !h.is_empty() && !h.contains('/')).unwrap_or("api.opensubtitles.com").to_owned();
    if let Ok(mut s) = SESSION.lock() { *s = Some(Session { token: token.clone(), host: host.clone(), at: std::time::Instant::now() }); }
    Ok((token, host))
}

/// The OpenSubtitles file hash: the size plus the 64-bit little-endian words of the
/// first and last 64 KB, wrapping.
pub fn movie_hash(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    ensure!(size >= 131_072, "File too small to hash");
    let mut hash = size;
    let mut chunk = vec![0u8; 65_536];
    for offset in [0, size - 65_536] {
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut chunk)?;
        for word in chunk.chunks_exact(8) { hash = hash.wrapping_add(u64::from_le_bytes(word.try_into().expect("8 bytes"))); }
    }
    Ok(format!("{hash:016x}"))
}

/// Tokens of a release name, for likeness between a file and a subtitle's release.
fn tokens(name: &str) -> std::collections::HashSet<String> {
    name.to_ascii_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|t| t.len() >= 2).map(str::to_owned).collect()
}

/// The best English subtitle of a search: made for this file (hash match) first, then
/// the release most like the file's name, then the most downloaded. Machine and AI
/// translations are never chosen; hearing-impaired versions only when nothing else fits.
pub(crate) fn choose(results: &[Value], release: &str, language: &str) -> Option<(u64, String)> {
    let mine = tokens(release);
    results.iter().filter_map(|r| {
        let a = &r["attributes"];
        if a["language"] != language || a["ai_translated"] == true || a["machine_translated"] == true || a["foreign_parts_only"] == true { return None; }
        let file = a["files"].as_array()?.first()?;
        let id = file["file_id"].as_u64()?;
        let theirs = tokens(a["release"].as_str().unwrap_or(""));
        let alike = mine.intersection(&theirs).count() as f64 / theirs.len().max(1) as f64;
        let score = (a["moviehash_match"] == true) as u8 as f64 * 100.0
            + alike * 10.0
            - (a["hearing_impaired"] == true) as u8 as f64 * 5.0
            + (a["download_count"].as_f64().unwrap_or(0.0) + 1.0).log10();
        Some((score, id, a["release"].as_str().unwrap_or("").to_owned()))
    }).max_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, id, release)| (id, release))
}

/// English subtitles for `asset`'s file at `input`, written into `directory` as `name`.
pub async fn find(data: &Path, asset: &Value, input: &Path, directory: &Path, name: &str) -> Result<Found> {
    find_language(data, asset, input, directory, name, "en").await
}

/// Subtitles in `language` ("en" or "zh-cn") for `asset`'s file, written into `directory`.
pub async fn find_language(data: &Path, asset: &Value, input: &Path, directory: &Path, name: &str, language: &str) -> Result<Found> {
    let (code, ours) = match language { "zh-cn" => ("zh-cn", "zh-Hans"), _ => ("en", "en") };
    let Some(c) = credentials(data) else { return Ok(Found::Missing("No OpenSubtitles account".into())) };
    let entity = asset["entity_id"].as_str().unwrap_or("");
    let mut query: Vec<(&str, String)> = Vec::new();
    if let Some(id) = entity.strip_prefix("tmdb:movie:") {
        query.push(("tmdb_id", id.split(':').next().unwrap_or(id).to_owned()));
    } else if let Some(id) = entity.strip_prefix("tmdb:tv:") {
        let (Some(season), Some(episode)) = (asset["season"].as_u64(), asset["episode"].as_u64()) else {
            return Ok(Found::Missing("Not an episode OpenSubtitles can look up".into()));
        };
        query.extend([("parent_tmdb_id", id.to_owned()), ("season_number", season.to_string()), ("episode_number", episode.to_string())]);
    } else {
        return Ok(Found::Missing("Title not identified on TMDB".into()));
    }
    let hashed = input.to_owned();
    if let Ok(Ok(hash)) = tokio::task::spawn_blocking(move || movie_hash(&hashed)).await { query.push(("moviehash", hash)); }
    query.push(("languages", code.into()));
    // OpenSubtitles redirects any other order of the parameters: send them sorted.
    query.sort();
    let client = client()?;
    let mut url = reqwest::Url::parse(&format!("{API}/subtitles"))?;
    for (key, value) in &query { url.query_pairs_mut().append_pair(key, value); }
    let reply = client.get(url).header("Api-Key", &c.api_key).header("Accept", "application/json").send().await.context("OpenSubtitles search")?;
    ensure!(reply.status().is_success(), "OpenSubtitles search failed ({})", reply.status());
    let results: Value = reply.json().await?;
    let release = format!("{} {}", asset["torrent_name"].as_str().unwrap_or(""), input.file_name().and_then(|n| n.to_str()).unwrap_or(""));
    let Some((file_id, chosen)) = choose(results["data"].as_array().map(Vec::as_slice).unwrap_or(&[]), &release, code) else {
        return Ok(Found::Missing(format!("No {ours} subtitles on OpenSubtitles")));
    };
    if let Some(until) = limited_until(data) { anyhow::bail!("{LIMIT_REACHED}; downloads reset at {until}"); }
    // Signed in, downloads count against the account (20 a day free); not signed in, against
    // the key's own quota (a paid API package).
    let request = if c.signed_in {
        let (token, host) = session(&client, &c).await?;
        client.post(format!("https://{host}/api/v1/download")).bearer_auth(&token)
    } else { client.post(format!("{API}/download")) };
    let reply = request.header("Api-Key", &c.api_key).header("Accept", "application/json")
        .json(&json!({"file_id": file_id, "sub_format": "srt"})).send().await.context("OpenSubtitles download")?;
    let status = reply.status();
    let body: Value = reply.json().await.unwrap_or(Value::Null);
    let reset = || body["reset_time_utc"].as_str().and_then(utc_seconds).unwrap_or(crate::storage::now() + 6.0 * 3600.0);
    if status.as_u16() == 406 || (status.is_success() && body["link"].is_null() && body["remaining"].as_i64().is_some_and(|r| r < 0)) {
        let until = reset();
        limit(data, until);
        anyhow::bail!("{LIMIT_REACHED}; downloads reset at {until}");
    }
    ensure!(status.is_success(), "OpenSubtitles download refused ({status})");
    if body["remaining"].as_i64().is_some_and(|r| r <= 0) { limit(data, reset()); }
    let link = body["link"].as_str().context("OpenSubtitles gave no download link")?;
    let bytes = client.get(link).send().await.context("OpenSubtitles file")?.error_for_status()?.bytes().await?;
    ensure!(bytes.len() <= 8 << 20, "Subtitle file too large");
    // A file that cannot be read is no source; the next one is tried.
    let read = crate::text_encoding::decode(&bytes, Some(ours)).and_then(|text| Ok((crate::subtitles::parse_srt(&text)?, text)));
    let (cues, text) = match read {
        Ok((cues, text)) if cues.len() >= 20 => (cues, text),
        Ok(_) => return Ok(Found::Missing("The OpenSubtitles file holds almost nothing".into())),
        Err(error) => return Ok(Found::Missing(format!("The OpenSubtitles file could not be read: {error}"))),
    };
    let path = directory.join(format!("{name}.{ours}.opensubtitles.srt"));
    let temporary = directory.join(format!("{name}.{ours}.opensubtitles.srt.tmp"));
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, &path)?;
    tracing::info!(asset = asset["id"].as_str().unwrap_or(""), language = ours, release = %chosen, cues = cues.len(), "Subtitles from OpenSubtitles");
    Ok(Found::Ready(path))
}

/// "2026-11-16T00:00:00.000Z" as seconds since the epoch.
fn utc_seconds(text: &str) -> Option<f64> {
    let (date, time) = text.trim_end_matches('Z').split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.split(':');
    let (h, min) = (t.next()?.parse::<i64>().ok()?, t.next()?.parse::<i64>().ok()?);
    let sec = t.next().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Some((days * 86_400 + h * 3600 + min * 60) as f64 + sec)
}

/// An OpenSubtitles failure (its limit, the site unreachable): the file tries again later
/// instead of going without subtitles or being transcribed.
pub fn retry_later(error: &str) -> bool { error.starts_with("OpenSubtitles") }

/// Seconds since the epoch in a limit error, so the file waits until then.
pub fn waits_until(error: &str) -> Option<f64> {
    error.strip_prefix(LIMIT_REACHED)?.rsplit(' ').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_hash_is_size_plus_the_words_at_both_ends() -> Result<()> {
        let path = std::env::temp_dir().join(format!("torrbaby-hash-{}", std::process::id()));
        std::fs::write(&path, vec![1u8; 200_000])?;
        // 200,000 + 2 × 8,192 words of 0x0101010101010101, wrapping.
        let expected = 200_000u64.wrapping_add(0x0101_0101_0101_0101u64.wrapping_mul(16_384));
        assert_eq!(movie_hash(&path)?, format!("{expected:016x}"));
        std::fs::remove_file(path)?;
        Ok(())
    }
    #[test]
    fn a_subtitle_for_this_file_wins_and_translations_never_do() {
        let result = |id: u64, release: &str, hash: bool, downloads: u64, extra: Value| {
            let mut a = json!({"language":"en","release":release,"moviehash_match":hash,"download_count":downloads,"hearing_impaired":false,"files":[{"file_id":id}]});
            if let (Some(a), Some(e)) = (a.as_object_mut(), extra.as_object()) { for (k, v) in e { a.insert(k.clone(), v.clone()); } }
            json!({"attributes": a})
        };
        let results = [
            result(1, "Parasite.2019.1080p.BluRay.x264-ZQ", false, 9000, json!({})),
            result(2, "Parasite (2019) [1080p] [YTS.LT]", false, 100, json!({})),
            result(3, "Parasite machine", true, 50, json!({"machine_translated":true})),
        ];
        assert_eq!(choose(&results, "Parasite (2019) Parasite (2019).mp4 YTS.LT 1080p", "en").map(|c| c.0), Some(2));
        let hashed = [result(1, "Other", false, 9000, json!({})), result(4, "Other", true, 1, json!({}))];
        assert_eq!(choose(&hashed, "x", "en").map(|c| c.0), Some(4));
        assert_eq!(choose(&[result(5, "x", false, 1, json!({"language":"fr"}))], "x", "en"), None);
        assert_eq!(choose(&[result(6, "x", false, 1, json!({"language":"zh-cn"}))], "x", "zh-cn").map(|c| c.0), Some(6));
        assert_eq!(utc_seconds("2026-11-16T00:00:00.000Z"), Some(1_794_787_200.0));
        assert_eq!(waits_until("OpenSubtitles daily download limit reached; downloads reset at 1794800000"), Some(1_794_800_000.0));
    }
}
