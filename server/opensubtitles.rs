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

#[derive(serde::Deserialize, Clone)]
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
    /// Also replace English the server read from text (bitmap subtitles, or Chinese read from
    /// the picture and translated): worth it while the quota is large. Off by default; audio
    /// transcripts and titles without subtitles always use OpenSubtitles.
    #[serde(default)] read_english: bool,
    /// Every title takes OpenSubtitles' English and Chinese wherever it has them, in place of
    /// any other source (release tracks, text read from the picture or bitmaps, transcripts,
    /// translations on device or by the fleet): the user's choice, 2026-11-16, for the paid
    /// month. What OpenSubtitles lacks keeps what the title has. Off by default.
    #[serde(default)] everything: bool,
}
fn yes() -> bool { true }

struct Session { token: String, host: String, at: std::time::Instant }

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
/// Seconds since the epoch when downloads may start again (0 when they may now), and whether
/// that was the signed-in account's quota or the key's own.
static LIMITED_UNTIL: Mutex<Option<(f64, bool)>> = Mutex::new(None);

pub enum Found {
    /// An SRT written to `path`, and where it came from (kept with the title, not shown).
    Ready(PathBuf, Value),
    /// Nothing suitable on OpenSubtitles (or no credentials): the next source is tried.
    Missing(String),
}

/// The account file, read once: nothing writes it but a person, and the server is restarted
/// after an edit. (It was read and parsed for each of the library's files on every pass.)
fn credentials(data: &Path) -> Option<Credentials> {
    static READ: std::sync::LazyLock<Mutex<std::collections::HashMap<std::path::PathBuf, Option<Credentials>>>> = std::sync::LazyLock::new(Default::default);
    let mut read = READ.lock().ok()?;
    read.entry(data.to_owned()).or_insert_with(|| std::fs::read(data.join("opensubtitles.json")).ok().and_then(|b| serde_json::from_slice(&b).ok())).clone()
}

pub fn configured(data: &Path) -> bool { credentials(data).is_some() }

/// Whether Chinese subtitles are looked for on OpenSubtitles before translating.
pub fn chinese(data: &Path) -> bool { credentials(data).is_some_and(|c| c.chinese) }

/// Whether English read from text is replaced by English from OpenSubtitles.
pub fn read_english(data: &Path) -> bool { credentials(data).is_some_and(|c| c.read_english) }

/// Whether every title takes OpenSubtitles' subtitles wherever it has them.
pub fn everything(data: &Path) -> bool { credentials(data).is_some_and(|c| c.everything) }

/// When downloads may start again, while the day's are used up. A limit applies only to the
/// quota that reached it: switching to a paid key's quota (or back) is not held up by the
/// other's (on 2026-11-16 the account's spent 20 would have held the paid 2,000 for a day).
pub fn limited_until(data: &Path) -> Option<f64> {
    let stored = LIMITED_UNTIL.lock().ok().and_then(|u| *u).unwrap_or_else(|| {
        // Kept across restarts, so a restart does not ask again and again.
        let stored = std::fs::read_to_string(data.join("opensubtitles-limit.txt")).ok().and_then(|t| parse_limit(&t)).unwrap_or((0.0, true));
        if let Ok(mut u) = LIMITED_UNTIL.lock() { *u = Some(stored); }
        stored
    });
    let signed_in = credentials(data).is_none_or(|c| c.signed_in);
    (stored.1 == signed_in && stored.0 > crate::storage::now()).then_some(stored.0)
}

/// "<until> account" or "<until> key"; a bare time was written signed in, before the key's quota.
fn parse_limit(text: &str) -> Option<(f64, bool)> {
    let mut words = text.split_whitespace();
    let until = words.next()?.parse().ok()?;
    Some((until, words.next() != Some("key")))
}

fn limit(data: &Path, until: f64, signed_in: bool) {
    if let Ok(mut u) = LIMITED_UNTIL.lock() { *u = Some((until, signed_in)); }
    let _ = std::fs::write(data.join("opensubtitles-limit.txt"), format!("{until} {}", if signed_in { "account" } else { "key" }));
    tracing::info!(until, signed_in, "OpenSubtitles downloads used up for today");
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
#[cfg(test)]
pub(crate) fn choose(results: &[Value], release: &str, language: &str) -> Option<(u64, String, Value)> {
    ranked(results, release, language).into_iter().next()
}

/// Every acceptable subtitle of a search, best first (see `choose`).
/// How uploads are ranked: version 2 (2026-11-18) puts trusted uploaders first and anonymous
/// ones last, with a matching hash only a tie-breaker. Version 1 put any hash match first, and
/// The Office S01E01 got an anonymous upload full of mistakes ("I couldn't closet") over the
/// administrator's from the DVD.
pub(crate) const RANKING: u64 = 2;

pub(crate) fn ranked(results: &[Value], release: &str, language: &str) -> Vec<(u64, String, Value)> {
    let mine = tokens(release);
    let mut found: Vec<_> = results.iter().filter_map(|r| {
        let a = &r["attributes"];
        // Results name Chinese "zh-CN" while searches ask for "zh-cn".
        if !a["language"].as_str().is_some_and(|l| l.eq_ignore_ascii_case(language)) || a["ai_translated"] == true || a["machine_translated"] == true || a["foreign_parts_only"] == true { return None; }
        let file = a["files"].as_array()?.first()?;
        let id = file["file_id"].as_u64()?;
        let theirs = tokens(a["release"].as_str().unwrap_or(""));
        let alike = mine.intersection(&theirs).count() as f64 / theirs.len().max(1) as f64;
        // Who made it counts most (votes are almost never given): OpenSubtitles' trusted
        // uploads (its administrators', often from the disc), then members by rank; anonymous
        // uploads last. A matching hash, the release's name and downloads break ties; the
        // length check after downloading still refuses another cut.
        let rank = a["uploader"]["rank"].as_str().unwrap_or("").to_lowercase();
        let anonymous = rank.is_empty() || rank == "anonymous" || a["uploader"]["name"].as_str().is_none_or(|n| n.eq_ignore_ascii_case("anonymous"));
        let trusted = a["from_trusted"] == true || ["administrator", "trusted"].iter().any(|t| rank.contains(t));
        let member = [("platinum", 6.0), ("gold", 4.0), ("silver", 2.0), ("bronze", 1.0)].iter().find(|(r, _)| rank.contains(r)).map_or(0.0, |(_, s)| *s);
        let votes = a["votes"].as_f64().unwrap_or(0.0);
        let rating = if votes > 0.0 { (a["ratings"].as_f64().unwrap_or(5.0) - 5.0) * votes.min(5.0) } else { 0.0 };
        let score = trusted as u8 as f64 * 30.0 + member - anonymous as u8 as f64 * 10.0 + rating
            + (a["moviehash_match"] == true) as u8 as f64 * 8.0
            + alike * 10.0
            - (a["hearing_impaired"] == true) as u8 as f64 * 5.0
            + (a["download_count"].as_f64().unwrap_or(0.0) + 1.0).log10();
        let provenance = json!({"source":"opensubtitles","subtitle_id":r["id"],"file_id":id,"release":a["release"],"language":language,
            "moviehash_match":a["moviehash_match"],"hearing_impaired":a["hearing_impaired"],"download_count":a["download_count"],"fps":a["fps"],"uploader":a["uploader"]["name"],
            "uploader_rank":a["uploader"]["rank"],"from_trusted":a["from_trusted"],"ranking":RANKING,"url":a["url"]});
        Some((score, id, a["release"].as_str().unwrap_or("").to_owned(), provenance))
    }).collect();
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    found.into_iter().map(|(_, id, release, provenance)| (id, release, provenance)).collect()
}

/// Whether subtitles whose last cue ends at `last_end` seconds are for a file `duration`
/// seconds long: they run past most of it and not beyond it. Catches another episode's or
/// another special's subtitles (OpenSubtitles numbered BSG's 45-minute "The Story So Far"
/// as the 90-minute Miniseries part 1). Unknown length: taken.
pub fn fits(last_end: f64, duration: Option<f64>) -> bool {
    let Some(duration) = duration.filter(|d| *d > 0.0) else { return true };
    last_end >= duration * 0.6 && last_end <= duration + 120.0
}

/// When OpenSubtitles last said "too many requests" (429): every search and download waits
/// until then and asks again, rather than each file finding out for itself and waiting an hour
/// (2026-11-18: re-asking 900 Chinese files at once left 248 waiting).
static PAUSED_UNTIL: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// `request` sent once OpenSubtitles' pause has passed, and again (up to 3 times) after each 429,
/// waiting as long as its Retry-After says (10 s when it says nothing).
async fn send(request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
    for _ in 0..3 {
        let until = PAUSED_UNTIL.lock().ok().and_then(|p| *p);
        if let Some(until) = until { tokio::time::sleep_until(until.into()).await }
        let reply = request.try_clone().context("OpenSubtitles request")?.send().await?;
        if reply.status() != reqwest::StatusCode::TOO_MANY_REQUESTS { return Ok(reply) }
        let wait = reply.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(10).clamp(1, 120);
        tracing::info!(wait, "OpenSubtitles: too many requests; every lookup waits");
        if let Ok(mut paused) = PAUSED_UNTIL.lock() { *paused = Some(std::time::Instant::now() + std::time::Duration::from_secs(wait)) }
    }
    anyhow::bail!("OpenSubtitles refused (429 Too Many Requests) three times")
}

/// Candidates downloaded at most for one lookup, looking for subtitles that fit the file.
const TRIES: usize = 3;

/// English subtitles for `asset`'s file at `input`, written into `directory` as `name`.
pub async fn find(data: &Path, asset: &Value, input: &Path, directory: &Path, name: &str) -> Result<Found> {
    find_language(data, asset, input, directory, name, "en", &[]).await
}

/// Subtitles in `language` ("en" or "zh-cn") for `asset`'s file, written into `directory`.
/// For Chinese, Traditional (zh-tw) is searched in the same request and used only when no
/// Simplified file fits: it is converted to Simplified, mainland usage (zhconv's phrase tables,
/// not a character swap). People's Traditional subtitles beat an English translation (the
/// user, 2026-11-18); Simplified is all that is shown (the Mainland).
/// Candidates are downloaded best first until one fits the file's length (`fits`), at most
/// `TRIES`; `refused` are file ids already found not to fit. A candidate that does not fit is
/// named in the answer's `refused`, so it is not downloaded again.
pub async fn find_language(data: &Path, asset: &Value, input: &Path, directory: &Path, name: &str, language: &str, refused: &[u64]) -> Result<Found> {
    let (code, ours) = match language { "zh-cn" => ("zh-cn", "zh-Hans"), _ => ("en", "en") };
    // The languages searched, in the order their files are tried.
    let codes: &[&str] = if code == "zh-cn" { &["zh-cn", "zh-tw"] } else { &[code] };
    let Some(c) = credentials(data) else { return Ok(Found::Missing("No OpenSubtitles account".into())) };
    let entity = asset["entity_id"].as_str().unwrap_or("");
    let client = client()?;
    let mut searches: Vec<Vec<(&str, String)>> = Vec::new();
    if let Some(id) = entity.strip_prefix("tmdb:movie:") {
        searches.push(vec![("tmdb_id", id.split(':').next().unwrap_or(id).to_owned())]);
    } else if let Some(id) = entity.strip_prefix("tmdb:tv:") {
        let (Some(season), Some(episode)) = (asset["season"].as_u64(), asset["episode"].as_u64()) else {
            return Ok(Found::Missing("Not an episode OpenSubtitles can look up".into()));
        };
        // Release packs number specials their own way (BSG's pack: S00E01 is the Miniseries,
        // TMDB's S00E01 "The Story So Far"), so a special is first looked up by its name.
        if season == 0 {
            if let Some(found) = input.file_stem().and_then(|n| n.to_str()).and_then(special) {
                if let Some(search) = special_search(&client, &c, &found).await { searches.push(search); }
            }
        }
        searches.push(vec![("parent_tmdb_id", id.to_owned()), ("season_number", season.to_string()), ("episode_number", episode.to_string())]);
    } else {
        return Ok(Found::Missing("Title not identified on TMDB".into()));
    }
    let hashed = input.to_owned();
    let hash = tokio::task::spawn_blocking(move || movie_hash(&hashed)).await.ok().and_then(Result::ok);
    let release = format!("{} {}", asset["torrent_name"].as_str().unwrap_or(""), input.file_name().and_then(|n| n.to_str()).unwrap_or(""));
    let measured = input.to_owned();
    let duration = tokio::task::spawn_blocking(move || crate::media_format::duration(&measured)).await.ok().flatten();
    let mut misfits: Vec<Value> = Vec::new();
    let mut unreadable = None;
    let mut tried: Vec<u64> = refused.to_vec();
    for mut query in searches {
        if let Some(hash) = &hash { query.push(("moviehash", hash.clone())); }
        query.push(("languages", codes.join(",")));
        // OpenSubtitles redirects any other order of the parameters: send them sorted.
        query.sort();
        let mut url = reqwest::Url::parse(&format!("{API}/subtitles"))?;
        for (key, value) in &query { url.query_pairs_mut().append_pair(key, value); }
        let reply = send(client.get(url).header("Api-Key", &c.api_key).header("Accept", "application/json")).await.context("OpenSubtitles search")?;
        ensure!(reply.status().is_success(), "OpenSubtitles search failed ({})", reply.status());
        let results: Value = reply.json().await?;
        let found = results["data"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        let candidates: Vec<_> = codes.iter().flat_map(|code| ranked(found, &release, code).into_iter()
            .filter(|(id, _, _)| !tried.contains(id)).take(TRIES).map(move |c| (*code, c))).collect();
        for (from, (file_id, chosen, mut provenance)) in candidates {
            tried.push(file_id);
            // Read as its own script: a Traditional file that is not UTF-8 is most likely Big5.
            let script = if from == "zh-tw" { "zh-Hant" } else { ours };
            let (cues, text) = match download(data, &client, &c, file_id, script).await? {
                Ok(read) => read,
                Err(why) => { unreadable = Some(why); continue }
            };
            let text = if from == "zh-tw" {
                provenance["converted_from"] = json!("zh-Hant");
                zhconv::zhconv(&text, zhconv::Variant::ZhCN)
            } else { text };
            let last_end = cues.iter().map(|c| c.end_ms).max().unwrap_or(0) as f64 / 1000.0;
            if !fits(last_end, duration) {
                tracing::info!(asset = asset["id"].as_str().unwrap_or(""), language = ours, release = %chosen, last_end, duration, "OpenSubtitles file does not fit this file's length");
                misfits.push(json!({"file_id": file_id, "release": chosen, "last_cue": last_end}));
                continue;
            }
            let path = directory.join(format!("{name}.{ours}.opensubtitles.srt"));
            let temporary = directory.join(format!("{name}.{ours}.opensubtitles.srt.tmp"));
            std::fs::write(&temporary, text)?;
            std::fs::rename(&temporary, &path)?;
            tracing::info!(asset = asset["id"].as_str().unwrap_or(""), language = ours, release = %chosen, cues = cues.len(), "Subtitles from OpenSubtitles");
            provenance["downloaded_at"] = json!(crate::storage::now());
            provenance["cues"] = json!(cues.len());
            provenance["fits"] = json!(true);
            provenance["duration"] = json!(duration);
            provenance["last_cue"] = json!(last_end);
            if !misfits.is_empty() { provenance["refused"] = json!(misfits); }
            return Ok(Found::Ready(path, provenance));
        }
    }
    if !misfits.is_empty() {
        return Ok(Found::Missing(format!("No {ours} subtitles on OpenSubtitles fit this file's length ({} tried)", misfits.len())));
    }
    Ok(Found::Missing(unreadable.unwrap_or_else(|| format!("No {ours} subtitles on OpenSubtitles"))))
}

/// A special's series and name from its file name, with the part when it has one:
/// "Battlestar Galactica (2003) - S00E01 - The Miniseries (1)" gives ("Battlestar Galactica",
/// "The Miniseries", Some(1)). Release tags in brackets ("(1080p BluRay x265 Celdra)") are dropped.
#[derive(Debug, PartialEq)]
pub(crate) struct Special { series: String, title: String, part: Option<u64> }
pub(crate) fn special(stem: &str) -> Option<Special> {
    use std::sync::LazyLock;
    static SPLIT: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)^(.+?)[ ._-]+S00E\d{1,3}(?:[- ]*E\d{1,3})*[ ._-]+(.+)$").expect("valid regex"));
    static TAGS: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)[(\[][^)\]]*(?:\d{3,4}p|bluray|web|hdtv|dvd|x26[45]|hevc)[^)\]]*[)\]]").expect("valid regex"));
    static YEAR: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\(\d{4}\)").expect("valid regex"));
    static PART: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\((\d{1,2})\)").expect("valid regex"));
    let parts = SPLIT.captures(stem)?;
    let series = YEAR.replace_all(&parts[1].replace(['.', '_'], " "), "").trim().to_owned();
    let title = TAGS.replace_all(&parts[2].replace(['.', '_'], " "), "").to_string();
    let part = PART.captures(&title).and_then(|c| c[1].parse().ok());
    let title = PART.replace_all(&title, " ").split_whitespace().collect::<Vec<_>>().join(" ");
    (!series.is_empty() && tokens(&title).len() >= 1).then_some(Special { series, title, part })
}

/// The search for a special by its name: OpenSubtitles' title for it (a movie, an episode
/// of its own, or a short series whose episodes are the parts), when one shares most of the
/// special's words. The length check after downloading still refuses a wrong one.
async fn special_search(client: &reqwest::Client, c: &Credentials, special: &Special) -> Option<Vec<(&'static str, String)>> {
    let mut url = reqwest::Url::parse(&format!("{API}/features")).ok()?;
    url.query_pairs_mut().append_pair("query", &format!("{} {}", special.series, special.title));
    let reply = send(client.get(url).header("Api-Key", &c.api_key).header("Accept", "application/json")).await.ok()?;
    if !reply.status().is_success() { return None }
    let found: Value = reply.json().await.ok()?;
    let (kind, imdb) = feature(found["data"].as_array()?, special)?;
    Some(if kind == "Tvshow" {
        vec![("parent_imdb_id", imdb.to_string()), ("episode_number", special.part.unwrap_or(1).to_string())]
    } else {
        vec![("imdb_id", imdb.to_string())]
    })
}
fn feature(features: &[Value], special: &Special) -> Option<(String, u64)> {
    let common = |w: &String| !["the", "of", "and", "an"].contains(&w.as_str());
    let wanted: std::collections::HashSet<String> = tokens(&special.title).into_iter().filter(common).collect();
    let series: std::collections::HashSet<String> = tokens(&special.series).into_iter().filter(common).collect();
    if wanted.is_empty() { return None }
    features.iter().filter_map(|f| {
        let a = &f["attributes"];
        let imdb = a["imdb_id"].as_u64()?;
        let theirs = tokens(a["title"].as_str().unwrap_or(""));
        if !series.iter().any(|w| theirs.contains(w)) && !a["parent_title"].as_str().is_some_and(|p| series.iter().any(|w| tokens(p).contains(w))) { return None }
        let alike = wanted.intersection(&theirs).count() as f64 / wanted.len().max(1) as f64;
        (alike >= 0.5).then(|| (alike, a["subtitles_count"].as_f64().unwrap_or(0.0), a["feature_type"].as_str().unwrap_or("").to_owned(), imdb))
    }).max_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))).map(|(_, _, kind, imdb)| (kind, imdb))
}

/// Download one OpenSubtitles file: its cues and text, or why it is no source (Ok(Err)).
/// Errors: the daily limit, or OpenSubtitles could not be asked.
async fn download(data: &Path, client: &reqwest::Client, c: &Credentials, file_id: u64, ours: &str) -> Result<Result<(Vec<crate::subtitles::TimedCue>, String), String>> {
    if let Some(until) = limited_until(data) { anyhow::bail!("{LIMIT_REACHED}; downloads reset at {until}"); }
    // Signed in, downloads count against the account (20 a day free); not signed in, against
    // the key's own quota (a paid API package).
    let request = if c.signed_in {
        let (token, host) = session(client, c).await?;
        client.post(format!("https://{host}/api/v1/download")).bearer_auth(&token)
    } else { client.post(format!("{API}/download")) };
    let reply = send(request.header("Api-Key", &c.api_key).header("Accept", "application/json")
        .json(&json!({"file_id": file_id, "sub_format": "srt"}))).await.context("OpenSubtitles download")?;
    let status = reply.status();
    let body: Value = reply.json().await.unwrap_or(Value::Null);
    let reset = || body["reset_time_utc"].as_str().and_then(utc_seconds).unwrap_or(crate::storage::now() + 6.0 * 3600.0);
    if status.as_u16() == 406 || (status.is_success() && body["link"].is_null() && body["remaining"].as_i64().is_some_and(|r| r < 0)) {
        let until = reset();
        limit(data, until, c.signed_in);
        anyhow::bail!("{LIMIT_REACHED}; downloads reset at {until}");
    }
    ensure!(status.is_success(), "OpenSubtitles download refused ({status})");
    if body["remaining"].as_i64().is_some_and(|r| r <= 0) { limit(data, reset(), c.signed_in); }
    tracing::info!(remaining = body["remaining"].as_i64(), "OpenSubtitles downloads left today");
    let link = body["link"].as_str().context("OpenSubtitles gave no download link")?;
    let bytes = client.get(link).send().await.context("OpenSubtitles file")?.error_for_status()?.bytes().await?;
    ensure!(bytes.len() <= 8 << 20, "Subtitle file too large");
    // A file that cannot be read is no source; the next one is tried.
    let read = crate::text_encoding::decode(&bytes, Some(ours)).and_then(|text| Ok((crate::subtitles::parse_srt(&text)?, text)));
    Ok(match read {
        Ok((cues, text)) if cues.len() >= 20 => Ok((cues, text)),
        Ok(_) => Err("The OpenSubtitles file holds almost nothing".into()),
        Err(error) => Err(format!("The OpenSubtitles file could not be read: {error}")),
    })
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
    #[test]
    fn a_trusted_upload_beats_an_anonymous_one_with_a_matching_hash() {
        let upload = |file: u64, hash: bool, trusted: bool, rank: &str, name: &str, downloads: u64| json!({"id": file.to_string(), "attributes": {
            "language": "en", "release": "The Office S01E01 Pilot", "moviehash_match": hash, "from_trusted": trusted, "download_count": downloads,
            "uploader": {"name": name, "rank": rank}, "files": [{"file_id": file}]}});
        // The Office S01E01 (2026-11-18): an anonymous hash match full of mistakes, the
        // administrator's from the DVD, and a gold member's.
        let results = [upload(1, true, false, "anonymous", "Anonymous", 70674), upload(2, false, true, "Administrator", "os-admin", 19642),
            upload(3, false, false, "Gold member", "someone", 47553)];
        let order: Vec<u64> = ranked(&results, "The Office (2005) - S01E01 - Pilot.mkv", "en").into_iter().map(|c| c.0).collect();
        assert_eq!(order, vec![2, 3, 1]);
    }

    #[test]
    fn traditional_subtitles_become_simplified_by_phrase() {
        // Not a character swap: 髮 and 發 both become 发, 乾 becomes 干 or stays 乾, and
        // Taiwanese words become mainland ones.
        let simplified = zhconv::zhconv("我們的頭髮很長。經濟發展很快。這個軟體很好用。乾隆皇帝", zhconv::Variant::ZhCN);
        assert_eq!(simplified, "我们的头发很长。经济发展很快。这个软件很好用。乾隆皇帝");
        // English lines in a bilingual file are left as they are.
        assert_eq!(zhconv::zhconv("I'm here.", zhconv::Variant::ZhCN), "I'm here.");
    }

    #[test]
    fn a_limit_holds_only_the_quota_that_reached_it() {
        assert_eq!(parse_limit("1794787199"), Some((1794787199.0, true)));
        assert_eq!(parse_limit("1794787199 account\n"), Some((1794787199.0, true)));
        assert_eq!(parse_limit("1794787199.5 key"), Some((1794787199.5, false)));
        assert_eq!(parse_limit(""), None);
    }

    use super::*;
    #[test]
    fn specials_are_looked_up_by_their_name() {
        let miniseries = special("Battlestar Galactica (2003) - S00E01 - The Miniseries (1)").unwrap();
        assert_eq!(miniseries, Special { series: "Battlestar Galactica".into(), title: "The Miniseries".into(), part: Some(1) });
        assert_eq!(special("Battlestar Galactica (2003) - S00E04-E05 - Razor (1080p BluRay x265 Celdra)").unwrap(),
            Special { series: "Battlestar Galactica".into(), title: "Razor".into(), part: None });
        assert_eq!(special("Battlestar Galactica (2003) - S00E33 - Razor Minisode (1) Day 4,571 (480p BluRay x265 Celdra)").unwrap().title, "Razor Minisode Day 4,571");
        assert!(special("Battlestar Galactica (2003) - S01E01 - 33").is_none());
        let features: Vec<Value> = serde_json::from_str(r#"[
            {"attributes":{"feature_type":"Movie","title":"Battlestar Galactica: The Plan","imdb_id":1286130,"subtitles_count":122}},
            {"attributes":{"feature_type":"Tvshow","title":"Battlestar Galactica (The Miniseries)","imdb_id":314979,"subtitles_count":280}},
            {"attributes":{"feature_type":"Tvshow","title":"Battlestar Galactica: Razor Flashbacks","imdb_id":1334430,"subtitles_count":53}},
            {"attributes":{"feature_type":"Episode","title":"Day 4,571","parent_title":"Battlestar Galactica: Razor Flashbacks","imdb_id":1336575,"subtitles_count":5}},
            {"attributes":{"feature_type":"Movie","title":"Planet of the Apes","imdb_id":1,"subtitles_count":9999}}]"#).unwrap();
        assert_eq!(feature(&features, &miniseries), Some(("Tvshow".into(), 314979)));
        let plan = special("Battlestar Galactica (2003) - S00E22 - The Plan (1080p BluRay x265 Celdra)").unwrap();
        assert_eq!(feature(&features, &plan), Some(("Movie".into(), 1286130)));
        let day = special("Battlestar Galactica (2003) - S00E33 - Razor Minisode (1) Day 4,571 (480p BluRay x265 Celdra)").unwrap();
        assert_eq!(feature(&features, &day), Some(("Episode".into(), 1336575)));
        let other = special("Battlestar Galactica (2003) - S00E40 - Blood & Chrome (1080p BluRay x265 Celdra)").unwrap();
        assert_eq!(feature(&features, &other), None);
    }

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
        assert_eq!(choose(&[result(5, "x", false, 1, json!({"language":"fr"}))], "x", "en").map(|c| c.0), None);
        assert_eq!(choose(&[result(6, "x", false, 1, json!({"language":"zh-CN"}))], "x", "zh-cn").map(|c| c.0), Some(6));
        assert_eq!(utc_seconds("2026-11-16T00:00:00.000Z"), Some(1_794_787_200.0));
        // BSG's Miniseries part 1 (about 91 minutes) against "The Story So Far" (about 44).
        assert!(!fits(44.0 * 60.0, Some(91.0 * 60.0)));
        assert!(fits(88.0 * 60.0, Some(91.0 * 60.0)));
        assert!(!fits(50.0 * 60.0, Some(44.0 * 60.0)), "a longer file's subtitles");
        assert!(fits(10.0, None));
        assert_eq!(waits_until("OpenSubtitles daily download limit reached; downloads reset at 1794800000"), Some(1_794_800_000.0));
    }
}
