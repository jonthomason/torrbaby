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
const CONNECTIONS: u64 = 8;

#[derive(Clone, Serialize, Deserialize)]
struct JobFile {
    /// Path inside the share, as the search result listed it.
    path: String,
    size: u64,
    destination: PathBuf,
    #[serde(default)]
    done: bool,
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
}

static VIDEO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\.(mp4|mkv|ts|mov|m2ts|avi)$").expect("valid regex"));
static SHARE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^https://pan\.quark\.cn/s/([0-9a-zA-Z]{6,40})").expect("valid regex"));

/// Season and episode a file name states, if any: "S01E02", "第12集", "EP12", "12.mkv".
pub fn episode(name: &str) -> (Option<u64>, Option<u64>) {
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
        for job in &mut jobs { if job.state == "saving" || job.state == "downloading" { job.state = "queued".into(); } }
        Ok(Self {
            cookie_file: data.join("quark.json"),
            jobs_file,
            search_url: std::env::var("TORRBABY_LINK_SEARCH").unwrap_or_else(|_| "http://127.0.0.1:8888".into()),
            http: reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).read_timeout(Duration::from_secs(60)).build()?,
            jobs: Mutex::new(jobs),
            progress: AtomicU64::new(0),
            searches: Mutex::new(HashMap::new()),
            wake: tokio::sync::Notify::new(),
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
        let videos: Vec<Value> = files.iter().filter(|(path, entry)| VIDEO.is_match(path) && entry["size"].as_u64().unwrap_or(0) >= 50_000_000).map(|(path, entry)| {
            let (season, number) = episode(path);
            json!({"path": path, "name": path.rsplit('/').next().unwrap_or(path), "size_bytes": entry["size"], "season": season, "episode": number})
        }).collect();
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
            "video_count": videos.len(), "total_bytes": total, "files": videos.into_iter().take(400).collect::<Vec<_>>()}))
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
    pub fn jobs(&self) -> Value {
        let jobs = self.jobs.lock().map(|j| j.clone()).unwrap_or_default();
        let running = self.progress.load(Ordering::Relaxed);
        json!({"jobs": jobs.iter().rev().take(50).map(|job| {
            let total: u64 = job.files.iter().map(|f| f.size).sum();
            let finished: u64 = job.files.iter().filter(|f| f.done).map(|f| f.size).sum();
            let done = if job.state == "downloading" { (finished + running).min(total) } else { finished };
            json!({"id": job.id, "title": job.title, "state": job.state, "files": job.files.len(), "files_done": job.files.iter().filter(|f| f.done).count(),
                "done_bytes": done, "total_bytes": total, "error": job.error, "created": job.created})
        }).collect::<Vec<_>>()})
    }
    /// Queue chosen videos of a share for the library. `files` are paths from a search
    /// result; without them every video in the share is taken.
    pub async fn add(&self, s: &Service, body: &Value) -> Result<Value> {
        ensure!(self.cookie().is_some(), "Sign in to Quark in Settings first");
        let url = body["url"].as_str().context("Missing share link")?;
        let title = clean(body["title"].as_str().unwrap_or(""));
        ensure!(!title.is_empty() && title.chars().count() <= 150, "Missing title");
        let season = body["season"].as_u64().unwrap_or(1).min(99);
        let wanted: Option<Vec<&str>> = body["files"].as_array().map(|list| list.iter().filter_map(Value::as_str).collect());
        let (id, token, _) = self.open(url).await?;
        let chosen: Vec<(String, u64)> = self.contents(&id, &token).await?.into_iter()
            .filter(|(path, entry)| VIDEO.is_match(path) && entry["size"].as_u64().unwrap_or(0) >= 50_000_000)
            .filter(|(path, _)| wanted.as_ref().is_none_or(|wanted| wanted.contains(&path.as_str())))
            .map(|(path, entry)| (path, entry["size"].as_u64().unwrap_or(0))).collect();
        ensure!(!chosen.is_empty(), "Nothing in this share matches");
        ensure!(chosen.len() <= 200, "Choose at most 200 files at a time");
        let config = s.effective_config()?;
        // One video without an episode number is a film; anything else is a series.
        let film = chosen.len() == 1 && episode(&chosen[0].0).1.is_none() && body["kind"] != "tv" || body["kind"] == "movie";
        let folder = PathBuf::from(if film { &config.movies } else { &config.tv }).join(&title);
        let files = chosen.into_iter().map(|(path, size)| {
            let extension = path.rsplit('.').next().unwrap_or("mkv").to_ascii_lowercase();
            let (stated, number) = episode(&path);
            let name = match number {
                Some(number) if !film => format!("{title} - S{:02}E{number:02}.{extension}", stated.unwrap_or(season)),
                _ if film => format!("{title}.{extension}"),
                _ => clean(path.rsplit('/').next().unwrap_or(&path)),
            };
            JobFile { destination: folder.join(name), path, size, done: false }
        }).collect();
        let job = Job { id: uuid::Uuid::new_v4().simple().to_string(), url: url.to_owned(), title, state: "queued".into(), files, error: None, created: now() };
        let id = job.id.clone();
        { let mut jobs = self.jobs.lock().map_err(|_| anyhow::anyhow!("Cloud downloads unavailable"))?; jobs.push(job); self.persist(&jobs); }
        self.wake.notify_one();
        Ok(json!({"job_id": id}))
    }
    pub fn dismiss(&self, id: &str) -> Value {
        let mut removed = false;
        if let Ok(mut jobs) = self.jobs.lock() {
            let before = jobs.len();
            jobs.retain(|job| job.id != id || job.state == "saving" || job.state == "downloading");
            removed = jobs.len() != before;
            self.persist(&jobs);
        }
        json!({"removed": removed})
    }
    async fn folder(&self, cookie: &str, parent: &str, name: &str) -> Result<String> {
        for entry in self.pages("/file/sort", &[("pdir_fid", parent.into())], Some(cookie)).await? {
            if entry["dir"] == true && entry["file_name"] == name { return entry["fid"].as_str().map(str::to_owned).context("Quark folder has no ID"); }
        }
        let made = self.call("/file", &[], Some(json!({"pdir_fid": parent, "file_name": name, "dir_path": "", "dir_init_lock": false})), Some(cookie), false).await?;
        made["data"]["fid"].as_str().map(str::to_owned).context("Quark did not create the folder")
    }
    /// Fetch one file in parallel ranges. Parts are kept between attempts, so a retry continues.
    async fn fetch(&self, cookie: &str, fid: &str, size: u64, destination: &Path) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        let link = self.call("/file/download", &[], Some(json!({"fids": [fid]})), Some(cookie), true).await?;
        let url = link["data"][0]["download_url"].as_str().context("Quark gave no download link")?.to_owned();
        let parent = destination.parent().context("Invalid destination")?;
        tokio::fs::create_dir_all(parent).await?;
        let step = size.div_ceil(CONNECTIONS).max(1);
        let part = |i: u64| PathBuf::from(format!("{}.part{i:02}", destination.display()));
        let ranges: Vec<(u64, u64, u64)> = (0..CONNECTIONS).map(|i| (i, i * step, ((i + 1) * step).min(size))).filter(|(_, start, end)| start < end).collect();
        let fetched = Arc::new(AtomicU64::new(0));
        let pieces = ranges.iter().map(|&(i, start, end)| {
            let (url, path, fetched) = (url.clone(), part(i), fetched.clone());
            async move {
                for attempt in 0..6u32 {
                    let have = tokio::fs::metadata(&path).await.map(|m| m.len()).unwrap_or(0).min(end - start);
                    if attempt == 0 { fetched.fetch_add(have, Ordering::Relaxed); }
                    if have == end - start { return Ok(()); }
                    let sent = self.http.get(&url).header("User-Agent", DESKTOP).header("Referer", "https://pan.quark.cn/").header("Cookie", cookie)
                        .header("Range", format!("bytes={}-{}", start + have, end - 1)).send().await;
                    let Ok(mut response) = sent else { tokio::time::sleep(Duration::from_secs(3)).await; continue };
                    if !response.status().is_success() { tokio::time::sleep(Duration::from_secs(3)).await; continue; }
                    let mut file = tokio::fs::OpenOptions::new().create(true).append(true).open(&path).await?;
                    let mut room = end - start - have;
                    while let Ok(Some(chunk)) = response.chunk().await {
                        let take = (chunk.len() as u64).min(room) as usize;
                        file.write_all(&chunk[..take]).await?;
                        room -= take as u64;
                        fetched.fetch_add(take as u64, Ordering::Relaxed);
                        self.progress.store(fetched.load(Ordering::Relaxed), Ordering::Relaxed);
                        if room == 0 { break; }
                    }
                    file.flush().await?;
                }
                let have = tokio::fs::metadata(&path).await.map(|m| m.len()).unwrap_or(0);
                ensure!(have == end - start, "The download kept stopping");
                Ok(())
            }
        });
        futures_util::future::try_join_all(pieces).await?;
        let staged = PathBuf::from(format!("{}.joining", destination.display()));
        let mut whole = tokio::fs::File::create(&staged).await?;
        for &(i, _, _) in &ranges {
            let mut piece = tokio::fs::File::open(part(i)).await?;
            tokio::io::copy(&mut piece, &mut whole).await?;
        }
        whole.sync_all().await?;
        ensure!(whole.metadata().await?.len() == size, "The downloaded file is the wrong size");
        tokio::fs::rename(&staged, destination).await?;
        for &(i, _, _) in &ranges { let _ = tokio::fs::remove_file(part(i)).await; }
        Ok(())
    }
    async fn run(&self, job: &Job) -> Result<()> {
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
        for file in &pending {
            let name = file.path.rsplit('/').next().unwrap_or(&file.path);
            let entry = mine_files.iter().find(|e| e["file_name"] == name && e["size"].as_u64() == Some(file.size)).context("A saved file is missing from the drive")?;
            let present = tokio::fs::metadata(&file.destination).await.is_ok_and(|m| m.len() == file.size);
            if !present { self.fetch(&cookie, entry["fid"].as_str().unwrap_or(""), file.size, &file.destination).await?; }
            self.progress.store(0, Ordering::Relaxed);
            let path = file.path.clone();
            self.update(&job.id, |j| { if let Some(f) = j.files.iter_mut().find(|f| f.path == path) { f.done = true; } });
        }
        // The drive is only a staging post: remove the job's copy once it is in the library.
        let _ = self.call("/file/delete", &[], Some(json!({"action_type": 2, "filelist": [mine], "exclude_fids": []})), Some(&cookie), false).await;
        Ok(())
    }
    /// Run queued jobs one at a time for as long as the server lives.
    pub async fn work(&self, s: &Service) {
        loop {
            let next = self.jobs.lock().ok().and_then(|jobs| jobs.iter().find(|j| j.state == "queued").cloned());
            let Some(job) = next else {
                tokio::select! { _ = self.wake.notified() => {}, _ = tokio::time::sleep(Duration::from_secs(60)) => {} }
                continue;
            };
            match self.run(&job).await {
                Ok(()) => {
                    self.update(&job.id, |j| { j.state = "done".into(); j.error = None; });
                    if let Err(error) = crate::api::scan_library(s).await { tracing::warn!(%error, "Library scan after a cloud download failed"); }
                }
                Err(error) => {
                    tracing::warn!(job = job.id, %error, "Cloud download failed");
                    self.update(&job.id, |j| { j.state = "failed".into(); j.error = Some(error.to_string()); });
                }
            }
            self.progress.store(0, Ordering::Relaxed);
        }
    }
    pub fn retry(&self, id: &str) -> Value {
        let mut queued = false;
        self.update(id, |job| if job.state == "failed" { job.state = "queued".into(); job.error = None; queued = true; });
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
        assert_eq!(episode("28.4k.mp4"), (None, Some(28)));
        assert_eq!(episode("01 4K.mp4"), (None, Some(1)));
        assert_eq!(episode("12.5.1.mkv"), (None, Some(12)));
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
