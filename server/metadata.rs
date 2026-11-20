use anyhow::{Context, Result, bail};
use regex::Regex;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio::sync::Mutex;

static YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|\D)((?:19|20)\d{2})(?:\D|$)").expect("valid regex"));
// A file may hold two episodes, as two-part pilots and finales do: S01E01E02, S01E01-E02, S01E01-02.
static SEASON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:s\d{1,2}(?:[ ._-]*e\d{1,3}(?:-?e\d{1,3}|-\d{1,3})*)?|season[ ._-]*\d{1,2}|\d{1,2}x\d{2})(?:[^a-z0-9]|$)").expect("valid regex")
});
pub static SERIES_PACK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bcomplete[ ._-]+(?:tv[ ._-]+)?series\b").expect("valid regex")
});
static TECHNICAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:complete|collection|trilogy|1080p|2160p|720p|bluray|webrip|web-dl|hdtv)\b",
    )
    .expect("valid regex")
});
static PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^www\.[^ ]+\s*[- ]*").expect("valid regex"));

/// The matcher's revision. Raised when it learns to name more (a country tag without a year,
/// & for and, a title a year off TMDB's: 2; TMDB's other titles: 3; a widely rated namesake
/// five times ahead: 4), so torrents it could not name are looked up again.
pub const MATCHER: u64 = 4;

pub fn parse_name(name: &str) -> (String, String, &'static str) {
    let name = PREFIX.replace(name, "");
    let name = name.trim_start_matches("[BT下载]");
    static CHINESE_EPISODES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:全|第|更新至|共)[0-9一二三四五六七八九十百]+(?:[-–][0-9]+)?[集季]|[0-9]+集(?:全|完)").expect("valid regex"));
    let chinese_episode = CHINESE_EPISODES.find(name);
    let chinese_title = if let Some(rest)=name.strip_prefix('[') {
        rest.split_once(']').map(|(title,_)|title)
    } else { name.split_once('[').map(|(title,_)|title) }
        .filter(|title|title.chars().any(|c|('\u{4e00}'..='\u{9fff}').contains(&c)));
    let season = SEASON.find(name);
    // A title may itself start with a year or contain one (1917, 2001, Blade Runner 2049).
    // The release year is the last year token before episode/encoding markers,
    // and must have a nonempty title before it.
    static ENCODING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(?:1080p|2160p|720p|bluray|webrip|web-dl|hdtv)\b").expect("valid regex"));
    let boundary = season.as_ref().map(|m|m.start()).into_iter()
        .chain(ENCODING.find(&name).map(|m|m.start())).min().unwrap_or(name.len());
    let year = YEAR.captures_iter(&name).filter(|c| c.get(1).is_some_and(|m|
        m.start() > 0 && m.start() < boundary
        && name[..m.start()].chars().any(|c|c.is_alphanumeric()))).last();
    let cutoff = [
        season.as_ref().map(|m| m.start()),
        chinese_episode.as_ref().map(|m|m.start()),
        year.as_ref().and_then(|c| c.get(1)).map(|m| m.start()),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or(name.len());
    let title = name[..cutoff].replace(['.', '_'], " ");
    let cutoff = TECHNICAL
        .find(&title)
        .map(|m| m.start())
        .unwrap_or(title.len());
    (
        chinese_title.unwrap_or(&title[..cutoff])
            .trim_matches(|c: char| c.is_whitespace() || "-()[]".contains(c))
            .to_owned(),
        year.and_then(|c| c.get(1).map(|m| m.as_str().to_owned()))
            .unwrap_or_default(),
        if season.is_some() || chinese_episode.is_some() || SERIES_PACK.is_match(name) { "tv" } else { "movie" },
    )
}
pub(crate) fn normalized(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
fn image(value: &Value, size: &str) -> Value {
    value
        .as_str()
        .map(|s| json!(format!("https://image.tmdb.org/t/p/{size}{s}")))
        .unwrap_or(Value::Null)
}
fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// TMDB searches per file: the parsed name plus up to three fallbacks.
pub const MAX_MATCH_QUERIES: usize = 4;

/// The first page's leading results as library match candidates.
fn search_candidates(data: &Value, kind: &str) -> Vec<Value> {
    array(&data["results"]).iter().take(8).map(|hit|{
        let date=hit["release_date"].as_str().or(hit["first_air_date"].as_str()).unwrap_or("");
        let id=hit["id"].as_u64().unwrap_or(0).to_string();
        json!({"provider":"TMDB","provider_id":id,"kind":kind,"title":hit.get("title").or(hit.get("name")),
            "original_title":hit.get("original_title").or(hit.get("original_name")),"year":date.get(..4).unwrap_or(""),
            "genres":[],"content_rating":null,"duration_minutes":null,"summary":hit["overview"],"thumbnail":image(&hit["poster_path"],"w500"),
            "logo":null,"source_url":format!("https://www.themoviedb.org/{kind}/{id}"),"episodes":[],"votes":hit["vote_count"].as_u64().unwrap_or(0),
            "country":array(&hit["origin_country"]).first().cloned().unwrap_or(Value::Null)})
    }).collect()
}

/// Candidates whose normalized title or original title equals the query, in the given year.
/// A release's country tag ("The Office (US) (2005)", "Dancing.With.The.Stars.US.S35E05") need
/// not be in TMDB's title: with a year, the year tells the US Office from the UK one (2001);
/// without one, the show must come from that country.
fn exact_candidates<'a>(query: &str, year: &str, candidates: &'a [Value]) -> Vec<&'a Value> {
    static COUNTRY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)[\s(\[]+(US|USA|UK|AU|NZ|CA)[)\]]*\s*$").expect("valid regex"));
    let tag = COUNTRY.captures(query).map(|c| match c[1].to_ascii_uppercase().as_str() { "USA" => "US".to_owned(), "UK" => "GB".to_owned(), other => other.to_owned() });
    // "Deadpool and Wolverine" is TMDB's "Deadpool & Wolverine".
    let normalized = |value: &str| normalized(&value.replace('&', " and "));
    let untagged = tag.as_ref().map(|_| normalized(&COUNTRY.replace(query, ""))).filter(|q| !q.is_empty());
    let query = normalized(query);
    candidates.iter().filter(|c| {
        let titles = [normalized(c["title"].as_str().unwrap_or("")), normalized(c["original_title"].as_str().unwrap_or(""))];
        let untagged_ok = untagged.as_ref().is_some_and(|u| titles.contains(u)) && (!year.is_empty() || c["country"].as_str() == tag.as_deref());
        !query.is_empty()
            && (titles.contains(&query) || untagged_ok)
            && (year.is_empty() || c["year"] == year)
    }).collect()
}

/// Further queries for a name whose parsed title did not match exactly, such as
/// `007.James.Bond.Moonraker.1979` in `11.Moonraker.1979/`: the folder's title without
/// a leading index number, the folder's title, then the file's title with leading words
/// dropped one at a time. Each keeps a release year and is accepted only on an exact
/// title and year match, so a shortened title cannot select a different film. Packs get
/// no fallbacks; a file name without a year only tries a folder that has one.
/// The Chinese name in a zh-CN TMDB response: its name when that is written in Chinese, else
/// the original title when that is (a Chinese production TMDB has not translated).
/// TMDB's Simplified Chinese overview. TMDB falls back to nothing (not to English) when a title
/// has no translation, but an overview without a Chinese character is not one either.
fn chinese_summary(detail: &Value) -> String {
    let text = detail["overview"].as_str().unwrap_or("").trim();
    if text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)) { text.to_owned() } else { String::new() }
}
/// How long a machine translation (or TMDB having none) stands before TMDB is asked again.
pub const CHINESE_SUMMARY_RECHECK_SECONDS: f64 = 30.0 * 86_400.0;
/// Records what TMDB said: its Chinese description, or that it has none. A machine translation
/// made earlier stays until TMDB has its own.
pub fn set_chinese_summary(entity: &mut Value, tmdb: &str) {
    if !tmdb.is_empty() {
        entity["summary_zh"] = json!(tmdb);
        entity["summary_zh_source"] = json!("tmdb");
    } else if entity["summary_zh_source"] != "machine" {
        entity["summary_zh"] = json!("");
        entity["summary_zh_source"] = json!("none");
    }
    entity["summary_zh_checked"] = json!(crate::storage::now());
}
/// Whether TMDB should be asked for this title's Chinese description: never asked, or asked
/// long enough ago that a machine translation (or nothing) may since have a TMDB one.
pub fn wants_chinese_summary(entity: &Value, now: f64) -> bool {
    match entity["summary_zh_source"].as_str() {
        Some("tmdb") => false,
        Some(_) => entity["summary_zh_checked"].as_f64().is_none_or(|at| now - at >= CHINESE_SUMMARY_RECHECK_SECONDS),
        None => true,
    }
}
/// When a machine-translated (or missing) Chinese description is due to be asked for again.
pub fn chinese_summary_due(entity: &Value) -> Option<f64> {
    match entity["summary_zh_source"].as_str() {
        Some("tmdb") | None => None,
        Some(_) => entity["summary_zh_checked"].as_f64().map(|at| at + CHINESE_SUMMARY_RECHECK_SECONDS),
    }
}
/// Whether this title's English description still needs translating on Diamond.
pub fn wants_summary_translation(entity: &Value) -> bool {
    entity["summary_zh_source"] == "none" && entity["summary"].as_str().is_some_and(|s| !s.trim().is_empty())
}
/// Japanese, not Chinese: it has kana (ドクターX ～外科医・大門未知子～ has kanji too).
fn japanese(text: &str) -> bool { text.chars().any(|c| ('\u{3040}'..='\u{30ff}').contains(&c)) }
fn chinese_name(detail: &Value, entity: &Value) -> String {
    let chinese = |text: &str| text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
    [detail.get("title").or(detail.get("name")), detail.get("original_title").or(detail.get("original_name")), entity.get("original_title")]
        .into_iter().flatten().filter_map(Value::as_str).map(str::trim).find(|t| chinese(t)).unwrap_or("").to_owned()
}
pub fn fallback_queries(query: &str, year: &str, name: &str, folder: Option<&str>) -> Vec<(String, String)> {
    static PACK: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?i)\b(?:complete|collection|trilogy|anthology|box[ ._-]*set)\b").expect("valid regex"));
    static INDEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{1,3}\s+").expect("valid regex"));
    if PACK.is_match(name) {
        return Vec::new();
    }
    let mut queries = Vec::new();
    if let Some(folder) = folder.filter(|folder| !PACK.is_match(folder)) {
        let (title, folder_year, _) = parse_name(folder);
        // The folder must name the same release year as the file.
        if !folder_year.is_empty() && (year.is_empty() || folder_year == year) {
            queries.push((INDEX.replace(&title, "").into_owned(), folder_year.clone()));
            queries.push((title, folder_year));
        }
    }
    if !year.is_empty() {
        let words: Vec<&str> = query.split_whitespace().collect();
        queries.extend((1..words.len()).map(|start| (words[start..].join(" "), year.to_owned())));
    }
    let mut seen = vec![(normalized(query), year.to_owned())];
    queries.into_iter().filter(|(q, y)| {
        let key = (normalized(q), y.clone());
        !key.0.is_empty() && !seen.contains(&key) && { seen.push(key); true }
    }).take(MAX_MATCH_QUERIES - 1).collect()
}

fn tmdb_cache_key(path: &str, query: &[(&str,&str)]) -> String {
    use sha2::{Digest,Sha256};
    let mut parameters=query.iter().map(|(key,value)|((*key).to_owned(),
        if *key=="query" {value.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()} else {(*value).to_owned()})).collect::<Vec<_>>();
    parameters.sort();
    let identity=serde_json::to_vec(&(path,parameters)).expect("Serializable TMDB request");
    Sha256::digest(identity).iter().map(|b|format!("{b:02x}")).collect()
}

#[derive(Default)]
struct SearchArtworkCache {
    images: std::collections::HashMap<String, String>,
    size: Option<String>,
    entries: std::collections::HashMap<String, (std::time::Instant, Value)>,
}

pub struct Metadata {
    client: reqwest::Client,
    token: PathBuf,
    host: Mutex<String>,
    cache: Mutex<std::collections::HashMap<String, Value>>,
    search_artwork: Mutex<SearchArtworkCache>,
    artwork_dir: PathBuf,
    response_dir: PathBuf,
    responses: Mutex<std::collections::HashMap<String, Arc<Mutex<Option<(std::time::Instant, Value)>>>>>,
    /// Earliest time the next network request to TMDB may start: requests are
    /// spaced out and pause entirely after a 429 (honouring Retry-After).
    next_request: Mutex<std::time::Instant>,
    /// Background lookups that failed recently, by entity: not retried for a while.
    failed_lookups: Mutex<std::collections::HashMap<String, std::time::Instant>>,
}
/// At most this many TMDB network requests per second from Diamond.
const TMDB_REQUESTS_PER_SECOND: u64 = 8;
impl Metadata {
    pub fn new(directory: &Path) -> Result<Self> {
        // TMDB from China is slow or blocked; it goes the way the torrent searches go (the
        // search tunnel, docs/search-proxy.md), whose far end opens TMDB and the search sites only.
        let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(20)).redirect(reqwest::redirect::Policy::none());
        if let Ok(proxy) = std::env::var("TORRBABY_SEARCH_PROXY") {
            anyhow::ensure!(!proxy.trim().is_empty(), "TORRBABY_SEARCH_PROXY must not be empty");
            builder = builder.proxy(reqwest::Proxy::all(&proxy).map_err(|_| anyhow::anyhow!("Invalid search proxy configuration"))?);
        }
        Ok(Self {
            client: builder.build()?,
            token: directory.join("tmdb.token"),
            host: Mutex::new("api.tmdb.org".to_owned()),
            cache: Mutex::new(std::collections::HashMap::new()),
            search_artwork: Mutex::new(SearchArtworkCache::default()),
            artwork_dir: directory.join("search-artwork"),
            response_dir: directory.join("tmdb-cache"),
            responses: Mutex::new(std::collections::HashMap::new()),
            next_request: Mutex::new(std::time::Instant::now()),
            failed_lookups: Mutex::new(std::collections::HashMap::new()),
        })
    }
    pub fn configured(&self) -> bool {
        self.token.exists()
    }
    pub async fn seed_cache(&self, entities: Vec<Value>) {
        let mut cache = self.cache.lock().await;
        for entity in entities {
            if entity["provider"] == "TMDB"
                && crate::storage::now() - entity["fetched_at"].as_f64().unwrap_or(0.0) < 86400.0
            {
                if let Some(id) = entity["id"].as_str() {
                    cache.insert(id.to_owned(), entity);
                }
            }
        }
    }
    pub async fn request(&self, path: &str, query: &[(&str, &str)]) -> Result<Value> {
        let key=tmdb_cache_key(path,query);
        let slot={
            let mut cache=self.responses.lock().await;
            if cache.len()>=256 {cache.retain(|_,slot|Arc::strong_count(slot)>1);}
            cache.entry(key.clone()).or_insert_with(||Arc::new(Mutex::new(None))).clone()
        };
        let mut cached=slot.lock().await;
        let ttl=Duration::from_secs(if path.starts_with("search/"){6*3600}else{86400});
        if let Some((at,value))=&*cached {if at.elapsed()<ttl {return Ok(value.clone());}}
        let file=self.response_dir.join(&key);
        if let Ok(meta)=std::fs::metadata(&file) {
            let age=meta.modified().ok().and_then(|t|t.elapsed().ok());
            if meta.len()<=8*1024*1024 && age.is_some_and(|age|age<ttl) {
                if let Ok(bytes)=std::fs::read(&file) {
                    if let Ok(value)=serde_json::from_slice::<Value>(&bytes) {
                        *cached=Some((std::time::Instant::now()-age.unwrap_or_default(),value.clone()));
                        return Ok(value);
                    }
                }
            }
        }
        let token = tokio::fs::read_to_string(&self.token).await.context("Configure a TMDB token on Diamond")?;
        let value=self.request_with_token(path,query,token.trim()).await?;
        if let Ok(bytes)=serde_json::to_vec(&value) {
            if bytes.len()<=8*1024*1024 && std::fs::create_dir_all(&self.response_dir).is_ok() {
                let tmp=file.with_extension("tmp");
                if std::fs::write(&tmp,bytes).is_ok(){let _=std::fs::rename(tmp,&file);}
                let mut files=std::fs::read_dir(&self.response_dir).into_iter().flatten().filter_map(Result::ok)
                    .filter_map(|e|{let m=e.metadata().ok()?;Some((m.modified().ok()?,m.len(),e.path()))}).collect::<Vec<_>>();
                files.sort_by_key(|e|e.0);let mut total:u64=files.iter().map(|e|e.1).sum();
                for (modified,len,path) in files {
                    if total<=128*1024*1024 && modified.elapsed().unwrap_or_default()<Duration::from_secs(86400){continue;}
                    if std::fs::remove_file(path).is_ok(){total=total.saturating_sub(len);}
                }
            }
        }
        *cached=Some((std::time::Instant::now(),value.clone()));
        Ok(value)
    }
    async fn request_with_token(
        &self,
        path: &str,
        query: &[(&str, &str)],
        token: &str,
    ) -> Result<Value> {
        let preferred = self.host.lock().await.clone();
        let alternate = if preferred == "api.tmdb.org" {
            "api.themoviedb.org"
        } else {
            "api.tmdb.org"
        };
        for host in [preferred.as_str(), alternate] {
            let mut url = reqwest::Url::parse(&format!("https://{host}/3/{path}"))?;
            url.query_pairs_mut().extend_pairs(query.iter().copied());
            let key = token.len() == 32 && token.bytes().all(|b| b.is_ascii_hexdigit());
            if key {
                url.query_pairs_mut().append_pair("api_key", token);
            }
            let mut request = self
                .client
                .get(url)
                .header("User-Agent", "Torrbaby/0.3 personal media library");
            if !key {
                request = request.bearer_auth(token);
            }
            {
                // Space out network requests; cached responses never reach here.
                let mut next = self.next_request.lock().await;
                let now = std::time::Instant::now();
                if *next > now { tokio::time::sleep(*next - now).await; }
                *next = std::time::Instant::now() + Duration::from_millis(1000 / TMDB_REQUESTS_PER_SECOND);
            }
            let response = match request.send().await {
                Ok(response) => response,
                Err(_) => continue,
            };
            if response.status().as_u16() == 429 {
                let wait = response.headers().get("retry-after").and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok()).unwrap_or(10).clamp(1, 300);
                *self.next_request.lock().await = std::time::Instant::now() + Duration::from_secs(wait);
                bail!("TMDB rate limited; retrying after {wait} s");
            }
            if !response.status().is_success() {
                bail!("TMDB HTTP {}", response.status().as_u16());
            }
            let result = response
                .json()
                .await
                .map_err(|_| anyhow::anyhow!("Invalid TMDB response"))?;
            *self.host.lock().await = host.to_owned();
            return Ok(result);
        }
        bail!("TMDB network unavailable")
    }
    pub async fn configure(&self, token: &str) -> Result<()> {
        let token = token.trim();
        if token.len() < 16 || token.len() > 4096 || token.contains(char::is_whitespace) {
            bail!("Invalid TMDB credential");
        }
        self.request_with_token("configuration", &[], token).await?;
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let temporary = self
            .token
            .with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(token.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(temporary, &self.token)?;
        Ok(())
    }
    /// Small posters for visible search rows. Never enrich or mutate the library.
    /// Poster for a release name, plus its running time so a result's size can be
    /// read as a bitrate: the film's runtime, or for `season` the number of
    /// episodes and their average length.
    pub async fn search_artwork(&self, name: &str, season: Option<u64>) -> Result<Value> {
        if !self.configured() { return Ok(json!({"data":null})); }
        anyhow::ensure!(name.len() <= 4096, "Release name too long");
        static PACK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(collection|trilogy|anthology|\d+[ -]+\d+[ ._-]+pack)\b").expect("valid regex"));
        let (query, year, kind) = parse_name(name);
        // Movie titles without a release year and collections are too ambiguous.
        if query.is_empty() || (kind == "movie" && year.is_empty())
            || PACK.is_match(name) {
            return Ok(json!({"data":null}));
        }
        let season = season.filter(|_| kind == "tv");
        let key = format!("{kind}:{}:{year}:{}", normalized(&query), season.map(|s| s.to_string()).unwrap_or_default());
        // Serializes lookups and coalesces duplicate releases of the same title.
        // This separate lock does not delay torrent searches or library requests.
        let mut cache = self.search_artwork.lock().await;
        if let Some((at, value)) = cache.entries.get(&key) {
            if at.elapsed() < Duration::from_secs(6 * 3600) { return Ok(value.clone()); }
        }
        let mut parameters = vec![("query", query.as_str()), ("language", "en-US"), ("include_adult", "false")];
        if !year.is_empty() { parameters.push((if kind == "movie" { "year" } else { "first_air_date_year" }, year.as_str())); }
        let data = self.request(&format!("search/{kind}"), &parameters).await?;
        let exact = array(&data["results"]).iter().filter(|hit| {
            let title = hit.get("title").or(hit.get("name")).and_then(Value::as_str).unwrap_or("");
            let original = hit.get("original_title").or(hit.get("original_name")).and_then(Value::as_str).unwrap_or("");
            let date = hit.get("release_date").or(hit.get("first_air_date")).and_then(Value::as_str).unwrap_or("");
            (normalized(&query) == normalized(title) || normalized(&query) == normalized(original))
                && (year.is_empty() || date.get(..4) == Some(year.as_str()))
        }).collect::<Vec<_>>();
        let mut result = json!({"data":null});
        if data["total_pages"].as_u64().unwrap_or(1) <= 1 && exact.len() == 1 {
            if let Some(path) = exact[0]["poster_path"].as_str() {
                if let Some((encoded, size)) = Self::thumbnail_in(self, &mut cache, path).await? {
                    result = json!({"data":encoded,
                        "provider":"TMDB","provider_id":exact[0]["id"],"size":size,
                        "confidence":if year.is_empty() {"exact_unique_title"} else {"exact_title_year"}});
                }
            }
        }
        // Running time is optional: a failed lookup only leaves the estimate out.
        if data["total_pages"].as_u64().unwrap_or(1) <= 1 && exact.len() == 1 {
            if let Some(id) = exact[0]["id"].as_u64() {
                if kind == "movie" {
                    if let Ok(detail) = self.request(&format!("movie/{id}"), &[("language", "en-US")]).await {
                        if let Some(minutes) = detail["runtime"].as_f64().filter(|m| *m > 0.0) { result["runtime_minutes"] = json!(minutes); }
                    }
                } else if let Some(season) = season {
                    if let Ok(detail) = self.request(&format!("tv/{id}/season/{season}"), &[("language", "en-US")]).await {
                        let episodes = array(&detail["episodes"]);
                        let lengths: Vec<f64> = episodes.iter().filter_map(|e| e["runtime"].as_f64()).filter(|m| *m > 0.0).collect();
                        if !lengths.is_empty() {
                            result["episode_minutes"] = json!(lengths.iter().sum::<f64>() / lengths.len() as f64);
                            result["season_episodes"] = json!(episodes.len());
                        }
                    }
                }
            }
        }
        if cache.entries.len() >= 128 { cache.entries.clear(); }
        cache.entries.insert(key, (std::time::Instant::now(), result.clone()));
        Ok(result)
    }
    /// TMDB's smallest poster for `path`, base64, with that size's name: from memory, the
    /// 64 MB folder of fetched thumbnails, or TMDB. None for a path that is not a poster's.
    /// The smallest poster size TMDB offers, asked once.
    async fn poster_size(&self, cache: &mut SearchArtworkCache) -> Result<Option<String>> {
        if cache.size.is_none() {
            let config = self.request("configuration", &[]).await?;
            cache.size = array(&config["images"]["poster_sizes"]).iter()
                .filter_map(Value::as_str)
                .filter_map(|size| Some((size.strip_prefix('w')?.parse::<u32>().ok()?, size)))
                .min_by_key(|(width, _)| *width).map(|(_, size)| size.to_owned());
        }
        Ok(cache.size.clone())
    }
    fn poster_path_ok(path: &str) -> bool {
        path.starts_with('/') && path.len() < 256 && !path.contains("..") && path.chars().all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
    }
    /// A poster's bytes ("w92/abc.jpg"), from the artwork folder or else image.tmdb.org,
    /// without the thumbnail cache's lock, so several can be fetched at once.
    async fn poster_bytes(&self, image_key: &str) -> Result<Vec<u8>> {
        use sha2::{Digest,Sha256};
        let file=self.artwork_dir.join(Sha256::digest(image_key.as_bytes()).iter().map(|b|format!("{b:02x}")).collect::<String>());
        if let Some(bytes)=std::fs::metadata(&file).ok().filter(|m|m.len()<=512*1024).and_then(|_|std::fs::read(&file).ok()) { return Ok(bytes); }
        // A poster is a nicety: one that is slow to come is left out rather than holding up the answer.
        let bytes = tokio::time::timeout(Duration::from_secs(6), async {
            let mut response=self.client.get(format!("https://image.tmdb.org/t/p/{image_key}")).send().await?.error_for_status()?;
            let mut bytes=Vec::new();
            while let Some(chunk)=response.chunk().await? {
                anyhow::ensure!(bytes.len()+chunk.len()<=512*1024,"Thumbnail too large");
                bytes.extend_from_slice(&chunk);
            }
            Ok::<_, anyhow::Error>(bytes)
        }).await.map_err(|_| anyhow::anyhow!("Poster took too long"))??;
        if std::fs::create_dir_all(&self.artwork_dir).is_ok() {
            let tmp=file.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
            if std::fs::write(&tmp,&bytes).is_ok(){let _=std::fs::rename(tmp,&file);}
            let mut files=std::fs::read_dir(&self.artwork_dir).into_iter().flatten().filter_map(Result::ok)
                .filter_map(|e|{let m=e.metadata().ok()?;Some((m.modified().ok()?,m.len(),e.path()))}).collect::<Vec<_>>();
            files.sort_by_key(|e|e.0);let mut total:u64=files.iter().map(|e|e.1).sum();
            for (_,len,path) in files {if total<=64*1024*1024 {break;}if std::fs::remove_file(path).is_ok(){total=total.saturating_sub(len);}}
        }
        Ok(bytes)
    }
    async fn thumbnail_in(&self, cache: &mut SearchArtworkCache, path: &str) -> Result<Option<(String, String)>> {
        use base64::Engine;
        if !Self::poster_path_ok(path) { return Ok(None); }
        let Some(size) = self.poster_size(cache).await? else { return Ok(None) };
        let image_key=format!("{size}{path}");
        if let Some(value)=cache.images.get(&image_key) { return Ok(Some((value.clone(), size))); }
        let encoded=base64::engine::general_purpose::STANDARD.encode(self.poster_bytes(&image_key).await?);
        if cache.images.len()>=128 {cache.images.clear();}
        cache.images.insert(image_key,encoded.clone());
        Ok(Some((encoded, size)))
    }
    /// Find's first step: films or series (`kind`) by name, with a small poster each, the
    /// Chinese title and, for a series, its country. TMDB's answers are cached as always.
    pub async fn find_titles(&self, query: &str, kind: &str) -> Result<Value> {
        anyhow::ensure!(["movie","tv"].contains(&kind), "Invalid media kind");
        let query = query.split_whitespace().collect::<Vec<_>>().join(" ");
        anyhow::ensure!((1..=200).contains(&query.chars().count()), "Enter a title");
        if !self.configured() { bail!("Connect TMDB in Settings to search by title"); }
        let parameters = [("query", query.as_str()), ("include_adult", "false")];
        // The Chinese titles come from the same search in Chinese, asked alongside; a miss only leaves them out.
        let path = format!("search/{kind}");
        let (in_english, in_chinese) = ([parameters[0], parameters[1], ("language", "en-US")], [parameters[0], parameters[1], ("language", "zh-CN")]);
        let (english, chinese) = tokio::join!(self.request(&path, &in_english), self.request(&path, &in_chinese));
        let (english, chinese) = (english?, chinese.unwrap_or(Value::Null));
        let mut titles = Vec::new();
        for hit in array(&english["results"]).iter().take(12) {
            let Some(id) = hit["id"].as_u64() else { continue };
            let zh = array(&chinese["results"]).iter().find(|c| c["id"] == hit["id"]).map(|c| chinese_name(c, &json!({}))).filter(|t| !japanese(t)).unwrap_or_default();
            let date = hit["release_date"].as_str().or(hit["first_air_date"].as_str()).unwrap_or("");
            titles.push(json!({"id":format!("tmdb:{kind}:{id}"),"kind":kind,"tmdb_id":id,
                "title":hit.get("title").or(hit.get("name")),"original_title":hit.get("original_title").or(hit.get("original_name")),
                "title_zh":zh,"year":date.get(..4).unwrap_or(""),"country":array(&hit["origin_country"]).first(),
                "summary":hit["overview"],"votes":hit["vote_count"].as_u64().unwrap_or(0),"poster_path":hit["poster_path"]}));
        }
        // Posters: those in memory at once, the rest fetched together (each gives up after a few
        // seconds) with no lock held meanwhile, so twelve slow images cost one wait, not twelve.
        use base64::Engine;
        let paths: Vec<Option<String>> = titles.iter_mut().map(|t| t["poster_path"].take().as_str().filter(|p| Self::poster_path_ok(p)).map(str::to_owned)).collect();
        let (size, mut wanted) = {
            let mut cache = self.search_artwork.lock().await;
            let size = self.poster_size(&mut cache).await.ok().flatten();
            let mut wanted = Vec::new();
            if let Some(size) = &size {
                for (index, path) in paths.iter().enumerate() {
                    let Some(path) = path else { continue };
                    let key = format!("{size}{path}");
                    match cache.images.get(&key) { Some(data) => titles[index]["poster"] = json!(data), None => wanted.push((index, key)) }
                }
            }
            (size, wanted)
        };
        if size.is_some() && !wanted.is_empty() {
            let fetched = futures_util::future::join_all(wanted.iter().map(|(_, key)| self.poster_bytes(key))).await;
            let mut cache = self.search_artwork.lock().await;
            for ((index, key), bytes) in wanted.drain(..).zip(fetched) {
                let Ok(bytes) = bytes else { continue };
                let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
                if cache.images.len()>=128 {cache.images.clear();}
                cache.images.insert(key, encoded.clone());
                titles[index]["poster"] = json!(encoded);
            }
        }
        Ok(json!({"titles":titles}))
    }
    /// What Find needs of one title before searching: the names releases may use (its title,
    /// original title and TMDB's alternative titles), its Chinese name and year, and for a
    /// series each season's aired episodes.
    /// A release's artwork and running time when Find already knows its title: no name
    /// matching, so "Deadpool and Wolverine" gets the poster of "Deadpool & Wolverine".
    pub async fn title_artwork(&self, title: &str, season: Option<u64>) -> Result<Value> {
        if !self.configured() { return Ok(json!({"data":null})); }
        let mut parts = title.splitn(3, ':');
        let (Some("tmdb"), Some(kind), Some(id)) = (parts.next(), parts.next(), parts.next().and_then(|n| n.parse::<u64>().ok())) else {
            anyhow::bail!("Invalid title");
        };
        anyhow::ensure!(["movie","tv"].contains(&kind), "Invalid media kind");
        let season = season.filter(|_| kind == "tv");
        let key = format!("title:{kind}:{id}:{}", season.map(|s| s.to_string()).unwrap_or_default());
        let mut cache = self.search_artwork.lock().await;
        if let Some((at, value)) = cache.entries.get(&key) {
            if at.elapsed() < Duration::from_secs(6 * 3600) { return Ok(value.clone()); }
        }
        let detail = self.request(&format!("{kind}/{id}"), &[("language", "en-US")]).await?;
        let mut result = json!({"data":null});
        if let Some(path) = detail["poster_path"].as_str() {
            if let Some((encoded, size)) = Self::thumbnail_in(self, &mut cache, path).await? {
                result = json!({"data":encoded,"provider":"TMDB","provider_id":id,"size":size,"confidence":"chosen_title"});
            }
        }
        // Running time is optional: a failed lookup only leaves the estimate out.
        if kind == "movie" {
            if let Some(minutes) = detail["runtime"].as_f64().filter(|m| *m > 0.0) { result["runtime_minutes"] = json!(minutes); }
        } else if let Some(season) = season {
            if let Ok(detail) = self.request(&format!("tv/{id}/season/{season}"), &[("language", "en-US")]).await {
                let episodes = array(&detail["episodes"]);
                let lengths: Vec<f64> = episodes.iter().filter_map(|e| e["runtime"].as_f64()).filter(|m| *m > 0.0).collect();
                if !lengths.is_empty() {
                    result["episode_minutes"] = json!(lengths.iter().sum::<f64>() / lengths.len() as f64);
                    result["season_episodes"] = json!(episodes.len());
                }
            }
        }
        if cache.entries.len() >= 128 { cache.entries.clear(); }
        cache.entries.insert(key, (std::time::Instant::now(), result.clone()));
        Ok(result)
    }
    pub async fn title_facts(&self, kind: &str, id: u64) -> Result<Value> {
        anyhow::ensure!(["movie","tv"].contains(&kind), "Invalid media kind");
        let detail = self.request(&format!("{kind}/{id}"), &[("language","en-US"),("append_to_response","alternative_titles")]).await?;
        let chinese = self.request(&format!("{kind}/{id}"), &[("language","zh-CN")]).await.unwrap_or(Value::Null);
        let title = detail.get("title").or(detail.get("name")).and_then(Value::as_str).unwrap_or("").to_owned();
        let original = detail.get("original_title").or(detail.get("original_name")).and_then(Value::as_str).unwrap_or("").to_owned();
        let latin = |t: &str| !t.is_empty() && t.chars().all(|c| c.is_ascii() || c.is_alphabetic() && (c as u32) < 0x250);
        let mut names = vec![title.clone()];
        if latin(&original) { names.push(original.clone()); }
        let alternatives = detail["alternative_titles"].get("titles").or(detail["alternative_titles"].get("results"));
        for alt in array(alternatives.unwrap_or(&Value::Null)) {
            let country = alt["iso_3166_1"].as_str().unwrap_or("");
            if let Some(name) = alt["title"].as_str() && latin(name) && ["US","GB","","KR","CN","JP"].contains(&country) { names.push(name.to_owned()); }
        }
        let mut seen = std::collections::HashSet::new();
        names.retain(|n| seen.insert(normalized(n)));
        let date = detail["release_date"].as_str().or(detail["first_air_date"].as_str()).unwrap_or("");
        let mut facts = json!({"id":format!("tmdb:{kind}:{id}"),"kind":kind,"title":title,"names":names,
            "title_zh":Some(chinese_name(&chinese, &json!({"original_title":original}))).filter(|t| !japanese(t)).unwrap_or_default(),"year":date.get(..4).unwrap_or(""),
            "country":array(&detail["origin_country"]).first()});
        if kind == "tv" {
            // Aired: every episode of an earlier season, up to the last aired one in its own.
            let last = &detail["last_episode_to_air"];
            let (last_season, last_episode) = (last["season_number"].as_u64().unwrap_or(0), last["episode_number"].as_u64().unwrap_or(0));
            let seasons: Vec<Value> = array(&detail["seasons"]).iter().filter_map(|season| {
                let number = season["season_number"].as_u64().filter(|n| *n > 0)?;
                let episodes = season["episode_count"].as_u64().unwrap_or(0);
                let aired = if number < last_season { episodes } else if number == last_season { last_episode.min(episodes.max(last_episode)) } else { 0 };
                Some(json!({"season":number,"episodes":episodes,"aired":aired}))
            }).collect();
            facts["seasons"] = json!(seasons);
        }
        Ok(facts)
    }
    pub async fn search(&self, name: &str) -> Result<Value> {
        self.search_in(name, None).await
    }
    /// Identify a file by its name, falling back to its folder's name. Only an exact
    /// title and year (or unique exact title) is selected; anything else needs review.
    pub async fn search_in(&self, name: &str, folder: Option<&str>) -> Result<Value> {
        let mut found = self.identify(name, folder).await?;
        let selected = if found["exact"].is_object() {
            let mut selected = self.enrich(&found["exact"]).await?;
            selected["match_confidence"] = json!(if found["matched_year"] == "" {
                "exact_unique_title"
            } else {
                "exact_title_year"
            });
            selected
        } else {
            Value::Null
        };
        let candidates = found["candidates"].take();
        let mut record = json!({"status":if !selected.is_null(){"matched"}else if array(&candidates).is_empty(){"no_match"}else{"review"},
            "query":found["query"],"candidates":candidates,"selected":selected});
        if found["matched_query"].is_string() && found["matched_query"] != found["query"] {
            record["matched_query"] = found["matched_query"].take();
        }
        Ok(record)
    }
    /// TMDB lookups without enrichment or library changes: the parsed query first, then
    /// at most `MAX_MATCH_QUERIES - 1` folder-derived or shortened queries (see
    /// `fallback_queries`), stopping at the first exact match. Cached responses are reused.
    pub async fn identify(&self, name: &str, folder: Option<&str>) -> Result<Value> {
        let (query, year, kind) = parse_name(name);
        if query.is_empty() {
            bail!("Could not infer a title from the filename");
        }
        let attempts = std::iter::once((query.clone(), year.clone(), false))
            .chain(fallback_queries(&query, &year, name, folder).into_iter().map(|(q, y)| (q, y, true)))
            .take(MAX_MATCH_QUERIES);
        let mut candidates: Vec<Value> = Vec::new();
        let mut tried = Vec::new();
        for (attempt, attempt_year, fallback) in attempts {
            let mut parameters = vec![("query", attempt.as_str()), ("language", "en-US"), ("include_adult", "false")];
            // Shortened queries are generic: let TMDB narrow them to the release year.
            if fallback {
                parameters.push((if kind == "movie" { "year" } else { "first_air_date_year" }, attempt_year.as_str()));
            }
            let data = self.request(&format!("search/{kind}"), &parameters).await?;
            let hits = search_candidates(&data, kind);
            let mut exact = exact_candidates(&attempt, &attempt_year, &hits);
            // A release often carries the festival year, TMDB the release year (Kill Me Three Times:
            // 2014 against 2015). One title of exactly this name a year either side is the one meant.
            if exact.is_empty() && !fallback {
                if let Ok(year) = attempt_year.parse::<i32>() {
                    let near: Vec<&Value> = exact_candidates(&attempt, "", &hits).into_iter()
                        .filter(|c| c["year"].as_str().and_then(|y| y.parse::<i32>().ok()).is_some_and(|y| (y - year).abs() == 1)).collect();
                    let same_name = exact_candidates(&attempt, "", &hits).len();
                    if near.len() == 1 && same_name == 1 { exact = near; }
                }
            }
            // The release may use one of TMDB's other titles ("F1 The Movie" is F1, "Star Wars
            // Episode IV A New Hope" is Star Wars): the year's candidates are asked for theirs.
            let mut alternative: Option<Value> = None;
            if exact.is_empty() && !fallback && !attempt_year.is_empty() {
                let wanted = normalized(&attempt.replace('&', " and "));
                let mut found = Vec::new();
                for hit in hits.iter().filter(|c| c["year"] == attempt_year.as_str()).take(3) {
                    let Some(id) = hit["provider_id"].as_str() else { continue };
                    let Ok(titles) = self.request(&format!("{kind}/{id}/alternative_titles"), &[]).await else { continue };
                    let names = array(titles.get("titles").or(titles.get("results")).unwrap_or(&Value::Null)).iter()
                        .filter_map(|t| t["title"].as_str()).map(|t| normalized(&t.replace('&', " and "))).collect::<Vec<_>>();
                    if names.contains(&wanted) { found.push(hit.clone()); }
                }
                if found.len() == 1 { alternative = found.pop(); }
            }
            if let Some(hit) = alternative {
                return Ok(json!({"query":query,"year":year,"kind":kind,"candidates":hits.iter().take(8).cloned().collect::<Vec<_>>(),"exact":hit,
                    "matched_query":attempt,"matched_year":attempt_year,"attempts":[attempt]}));
            }
            tried.push(json!(attempt));
            for hit in &hits {
                if candidates.len() < 8 && !candidates.iter().any(|c| c["provider_id"] == hit["provider_id"]) {
                    candidates.push(hit.clone());
                }
            }
            if exact.len() == 1 {
                let exact = exact[0].clone();
                return Ok(json!({"query":query,"year":year,"kind":kind,"candidates":candidates,"exact":exact,
                    "matched_query":attempt,"matched_year":attempt_year,"attempts":tried}));
            }
            // Several titles share the name. When one is far better known than the rest
            // (Heroes 2006 against an obscure 2020 namesake), it is the one meant.
            if exact.len() > 1 {
                let mut ranked = exact.clone();
                ranked.sort_by_key(|c| std::cmp::Reverse(c["votes"].as_u64().unwrap_or(0)));
                let (first, second) = (ranked[0]["votes"].as_u64().unwrap_or(0), ranked[1]["votes"].as_u64().unwrap_or(0));
                // A widely rated film well ahead counts too (The Odyssey 2026: 5,202 votes against 783).
                if (first >= 100 && first >= second.saturating_mul(10)) || (first >= 1000 && first >= second.saturating_mul(5)) {
                    let exact = ranked[0].clone();
                    return Ok(json!({"query":query,"year":year,"kind":kind,"candidates":candidates,"exact":exact,
                        "matched_query":attempt,"matched_year":attempt_year,"attempts":tried}));
                }
                // Otherwise it stays ambiguous; a shorter query cannot resolve that.
                break;
            }
        }
        Ok(json!({"query":query,"year":year,"kind":kind,"candidates":candidates,"exact":null,"matched_query":null,"attempts":tried}))
    }
    /// A series by the title it is known by at home ("九重紫"), with each season's episode
    /// count and how many of them have aired. Null when no series has exactly that title.
    pub async fn series_episodes(&self, name: &str) -> Result<Value> {
        let fold = |text: &str| text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
        let wanted = fold(name);
        if wanted.is_empty() { return Ok(Value::Null); }
        let data = self.request("search/tv", &[("query", name), ("language", "zh-CN"), ("include_adult", "false")]).await?;
        // TMDB lists the best-known first, so the first exact title is the one meant.
        let Some(hit) = array(&data["results"]).iter().find(|hit| [&hit["name"], &hit["original_name"]].iter().any(|title| title.as_str().is_some_and(|t| fold(t) == wanted))) else { return Ok(Value::Null) };
        let Some(id) = hit["id"].as_u64() else { return Ok(Value::Null) };
        let detail = self.request(&format!("tv/{id}"), &[("language", "zh-CN")]).await?;
        let last = &detail["last_episode_to_air"];
        let (reached, number) = (last["season_number"].as_u64().unwrap_or(0), last["episode_number"].as_u64().unwrap_or(0));
        let seasons: Vec<Value> = array(&detail["seasons"]).iter().filter_map(|season| {
            let (index, episodes) = (season["season_number"].as_u64().filter(|n| *n > 0)?, season["episode_count"].as_u64().unwrap_or(0));
            let aired = if index < reached { episodes } else if index == reached { number.min(episodes.max(number)) } else { 0 };
            Some(json!({"season": index, "episodes": episodes.max(aired), "aired": aired}))
        }).collect();
        Ok(json!({"provider": "TMDB", "provider_id": id.to_string(), "title": detail["name"], "original_title": detail["original_name"],
            "year": detail["first_air_date"].as_str().and_then(|date| date.get(..4)), "status": detail["status"], "seasons": seasons}))
    }
    /// The English title of a series known by this title at home, if exactly one is. Answers are cached.
    pub async fn series_english_title(&self, name: &str) -> Option<String> {
        let fold = |text: &str| text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
        let wanted = fold(name);
        if wanted.is_empty() { return None; }
        let data = self.request("search/tv", &[("query", name), ("language", "en-US"), ("include_adult", "false")]).await.ok()?;
        array(&data["results"]).iter().find(|hit| hit["original_name"].as_str().is_some_and(|t| fold(t) == wanted)).and_then(|hit| hit["name"].as_str()).map(str::to_owned)
    }
    pub async fn cast(&self, entity: &Value) -> Result<Value> {
        let kind=entity["kind"].as_str().context("Missing media kind")?;
        anyhow::ensure!(["movie","tv"].contains(&kind), "Invalid media kind");
        let id=entity["provider_id"].as_str().context("Missing metadata ID")?.parse::<u64>()?;
        let route=if kind=="tv" {"aggregate_credits"} else {"credits"};
        let credits=self.request(&format!("{kind}/{id}/{route}"), &[("language","en-US")]).await?;
        Ok(cast_rows(&credits["cast"]))
    }

    /// The title's Chinese name and description, shown when the apps are in Chinese: TMDB's
    /// Simplified Chinese name (or the original title of a Chinese production) and its Simplified
    /// Chinese overview. Each is empty when TMDB has none.
    pub async fn chinese_details(&self, entity: &Value) -> Result<(String, String)> {
        let kind=entity["kind"].as_str().context("Missing media kind")?;
        anyhow::ensure!(["movie","tv"].contains(&kind), "Invalid media kind");
        let id=entity["provider_id"].as_str().context("Missing metadata ID")?.parse::<u64>()?;
        let detail=self.request(&format!("{kind}/{id}"), &[("language","zh-CN")]).await?;
        Ok((chinese_name(&detail, entity), chinese_summary(&detail)))
    }
    /// TMDB's Simplified Chinese episode descriptions for one season of a series, by episode
    /// number; empty text where TMDB has none.
    pub async fn chinese_season(&self, entity: &Value, season: u64) -> Result<Vec<(u64, String)>> {
        anyhow::ensure!(entity["kind"] == "tv", "Not a series");
        let id=entity["provider_id"].as_str().context("Missing metadata ID")?.parse::<u64>()?;
        let data=self.request(&format!("tv/{id}/season/{season}"), &[("language","zh-CN")]).await?;
        Ok(array(&data["episodes"]).iter().filter_map(|episode| Some((episode["episode_number"].as_u64()?, chinese_summary(episode)))).collect())
    }
    /// A series' production status and last air date, for entities saved before these were kept.
    pub async fn series_run(&self, entity: &Value) -> Result<Value> {
        let id=entity["provider_id"].as_str().context("Missing metadata ID")?.parse::<u64>()?;
        let detail=self.request(&format!("tv/{id}"), &[("language","en-US")]).await?;
        let status=detail["status"].as_str().filter(|s|!s.is_empty()).context("TMDB returned no series status")?;
        Ok(json!({"series_status":status,"last_air_date":detail["last_air_date"].as_str().unwrap_or("")}))
    }
    /// Whether a background lookup for this entity failed recently (6 h backoff).
    /// When the earliest failed lookup may be tried again (epoch seconds).
    pub async fn next_retry(&self) -> Option<f64> {
        let failed = self.failed_lookups.lock().await;
        failed.values().map(|at| crate::storage::now() + (Duration::from_secs(6 * 3600).saturating_sub(at.elapsed())).as_secs_f64()).reduce(f64::min)
    }
    pub async fn recently_failed(&self, id: &str) -> bool {
        self.failed_lookups.lock().await.get(id).is_some_and(|at| at.elapsed() < Duration::from_secs(6 * 3600))
    }
    pub async fn record_failure(&self, id: &str) {
        let mut failed = self.failed_lookups.lock().await;
        if failed.len() >= 4096 { failed.retain(|_, at| at.elapsed() < Duration::from_secs(6 * 3600)); }
        failed.insert(id.to_owned(), std::time::Instant::now());
    }

    pub async fn enrich(&self, candidate: &Value) -> Result<Value> {
        if candidate["provider"] != "TMDB" {
            bail!("Choose a TMDB match for standalone metadata");
        }
        let kind = candidate["kind"].as_str().context("Missing media kind")?;
        if !["movie", "tv"].contains(&kind) {
            bail!("Invalid media kind");
        }
        let id = candidate["provider_id"]
            .as_str()
            .context("Missing metadata ID")?
            .parse::<u64>()?;
        let key = format!("tmdb:{kind}:{id}");
        if let Some(entity) = self.cache.lock().await.get(&key) {
            if entity["cast"].is_array() && (kind != "tv" || entity["series_status"].as_str().is_some_and(|s| !s.is_empty()))
                && crate::storage::now() - entity["fetched_at"].as_f64().unwrap_or(0.0) < 86400.0 {
                return Ok(entity.clone());
            }
        }
        let detail = self
            .request(
                &format!("{kind}/{id}"),
                &[
                    ("language", "en-US"),
                    ("append_to_response", if kind=="tv" {"images,release_dates,content_ratings,aggregate_credits"} else {"images,release_dates,content_ratings,credits"}),
                    ("include_image_language", "en,zh,null"),
                ],
            )
            .await?;
        let mut entity = candidate.clone();
        entity["id"] = json!(format!("tmdb:{kind}:{id}"));
        entity["title"] = detail
            .get("title")
            .or(detail.get("name"))
            .cloned()
            .unwrap_or(Value::Null);
        entity["original_title"] = detail
            .get("original_title")
            .or(detail.get("original_name"))
            .cloned()
            .unwrap_or(Value::Null);
        let date = detail["release_date"]
            .as_str()
            .or(detail["first_air_date"].as_str())
            .unwrap_or("");
        entity["release_date"] = json!(date);
        entity["year"] = json!(date.get(..4).unwrap_or(""));
        if kind == "tv" {
            // Lets clients show a series' run: "2018–2025", or "2018–" while it continues.
            // Never record an empty status: absent means the backfill will fetch it.
            if let Some(status) = detail["status"].as_str().filter(|s| !s.is_empty()) {
                entity["series_status"] = json!(status);
                entity["last_air_date"] = json!(detail["last_air_date"].as_str().unwrap_or(""));
            }
        }
        entity["summary"] = detail["overview"].clone();
        // A missing Chinese name or description is filled in by the metadata backfill.
        if let Ok((name, summary)) = self.chinese_details(&entity).await {
            entity["title_zh"] = json!(name);
            set_chinese_summary(&mut entity, &summary);
        }
        entity["cast"] = cast_rows(&detail[if kind=="tv" {"aggregate_credits"} else {"credits"}]["cast"]);
        entity["thumbnail"] = image(&detail["poster_path"], "w500");
        entity["backdrop"] = image(&detail["backdrop_path"], "w1280");
        entity["genres"] = json!(
            array(&detail["genres"])
                .iter()
                .filter_map(|g| g["name"].as_str())
                .collect::<Vec<_>>()
        );
        entity["duration_minutes"] = detail
            .get("runtime")
            .or(array(&detail["episode_run_time"]).first())
            .cloned()
            .unwrap_or(Value::Null);
        entity["logo"] = image(
            &array(&detail["images"]["logos"])
                .first()
                .unwrap_or(&Value::Null)["file_path"],
            "w500",
        );
        if let Some(rating) = array(
            &detail[if kind == "movie" {
                "release_dates"
            } else {
                "content_ratings"
            }]["results"],
        )
        .iter()
        .find(|r| r["iso_3166_1"] == "US")
        {
            entity["content_rating"] = if kind == "tv" {
                rating["rating"].clone()
            } else {
                array(&rating["release_dates"])
                    .iter()
                    .find_map(|r| r["certification"].as_str().filter(|s| !s.is_empty()))
                    .map(|s| json!(s))
                    .unwrap_or(Value::Null)
            };
        }
        if kind == "tv" {
            let mut episodes = Vec::new();
            for season in array(&detail["seasons"]) {
                let Some(number) = season["season_number"].as_u64() else {
                    continue;
                };
                let data = self
                    .request(
                        &format!("tv/{id}/season/{number}"),
                        &[("language", "en-US")],
                    )
                    .await?;
                for episode in array(&data["episodes"]) {
                    episodes.push(json!({"title":episode["name"],"season":episode["season_number"],"episode":episode["episode_number"],"duration_minutes":episode["runtime"],"airdate":episode["air_date"],"summary":episode["overview"],"thumbnail":image(&episode["still_path"],"w500")}));
                }
            }
            entity["episodes"] = json!(episodes);
        }
        entity["fetched_at"] = json!(crate::storage::now());
        self.cache.lock().await.insert(key, entity.clone());
        Ok(entity)
    }
}

// Preserve TMDB's fields and ordering; do not infer roles or aggregate episode counts.
fn cast_rows(value: &Value) -> Value {
    json!(array(value).iter().filter(|person| person["name"].as_str().is_some_and(|name| !name.is_empty())).map(|person| {
        let mut row=json!({"id":person["id"],"name":person["name"]});
        for field in ["character","total_episode_count"] { if !person[field].is_null() { row[field]=person[field].clone(); } }
        if let Some(roles)=person["roles"].as_array() { row["roles"]=json!(roles.iter().map(|role| json!({"character":role["character"],"episode_count":role["episode_count"]})).collect::<Vec<_>>()); }
        row
    }).collect::<Vec<_>>())
}

#[cfg(test)]
mod cast_tests {
    #[test]
    fn chinese_name_prefers_a_chinese_translation_then_a_chinese_original() {
        let show=json!({"original_title":"尚公主"});
        assert_eq!(chinese_name(&json!({"name":"尚公主","original_name":"尚公主"}),&show),"尚公主");
        assert_eq!(chinese_name(&json!({"title":"007：大战皇家赌场","original_title":"Casino Royale"}),&json!({})),"007：大战皇家赌场");
        assert_eq!(chinese_name(&json!({"name":"Tigers Sniff the Rose"}),&show),"尚公主");
        assert_eq!(chinese_name(&json!({"title":"Casino Royale","original_title":"Casino Royale"}),&json!({})),"");
    }

    use super::*;
    #[test]
    fn chinese_summary_comes_from_tmdb_then_translation_and_rechecks() {
        assert_eq!(chinese_summary(&json!({"overview":" 一位公主…  "})),"一位公主…");
        assert_eq!(chinese_summary(&json!({"overview":"An English fallback"})),"");
        let mut entity=json!({"summary":"A princess…"});
        assert!(wants_chinese_summary(&entity, 0.0));
        set_chinese_summary(&mut entity, "");
        assert_eq!(entity["summary_zh_source"],"none");
        assert!(wants_summary_translation(&entity));
        let now=entity["summary_zh_checked"].as_f64().unwrap();
        assert!(!wants_chinese_summary(&entity, now + 60.0));
        assert!(wants_chinese_summary(&entity, now + CHINESE_SUMMARY_RECHECK_SECONDS));
        entity["summary_zh"]=json!("一位公主…"); entity["summary_zh_source"]=json!("machine");
        set_chinese_summary(&mut entity, "");
        assert_eq!((entity["summary_zh"].as_str(), entity["summary_zh_source"].as_str()), (Some("一位公主…"), Some("machine")));
        assert!(!wants_summary_translation(&entity));
        set_chinese_summary(&mut entity, "TMDB 的简介");
        assert_eq!((entity["summary_zh"].as_str(), entity["summary_zh_source"].as_str()), (Some("TMDB 的简介"), Some("tmdb")));
        assert!(!wants_chinese_summary(&entity, f64::MAX));
        assert!(!wants_summary_translation(&json!({"summary":"","summary_zh_source":"none"})));
    }
    #[test]
    fn credits_preserve_supplied_roles_and_counts() {
        let rows=cast_rows(&json!([{"id":1,"name":"Actor","total_episode_count":23,"roles":[{"character":"First role","episode_count":20},{"character":"Second role","episode_count":8}]}]));
        assert_eq!(rows[0]["total_episode_count"],23);
        assert_eq!(rows[0]["roles"][1]["episode_count"],8);
        assert_eq!(cast_rows(&json!([{"name":"Actor","character":"Character"}]))[0]["character"],"Character");
    }
}

#[cfg(test)]
mod numeric_title_tests {
    use super::*;
    #[test]
    fn title_numbers_are_not_release_years() {
        for (name,title,year) in [
            ("2001.A.Space.Odyssey.1968.1080p.BluRay", "2001 A Space Odyssey", "1968"),
            ("2001: A Space Odyssey (1968)", "2001: A Space Odyssey", "1968"),
            ("1917 (2019) 1080p", "1917", "2019"),
            ("Blade Runner 2049 (2017) 1080p", "Blade Runner 2049", "2017"),
            ("2001 A Space Odyssey", "2001 A Space Odyssey", ""),
        ] { let parsed=parse_name(name);assert_eq!((parsed.0.as_str(),parsed.1.as_str()),(title,year)); }
        assert_eq!(normalized("2001: A Space Odyssey"),normalized("2001 A Space Odyssey"));
    }
}

#[cfg(test)]
mod chinese_title_tests {
    use super::*;
    #[test]
    fn forum_tags_are_not_part_of_the_title() {
        for name in ["[BT下载][逐玉][全40集][国语配音/中文字幕]", "逐玉[杜比视界版本][全40集][国语配音/中文字幕].Pursuit.of.Jade.S01.2026.2160p"] {
            let (title,_,kind)=parse_name(name); assert_eq!(title,"逐玉");assert_eq!(kind,"tv");
        }
        assert_eq!(parse_name("流浪地球[国语配音/中文字幕].2019.1080p").0,"流浪地球");
    }
}

#[cfg(test)]
mod response_cache_checks {
    use super::*;
    #[tokio::test]
    async fn persisted_tmdb_response_reuses_content_without_token_or_network() {
        let root=std::env::temp_dir().join(format!("torrbaby-tmdb-cache-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("tmdb-cache")).unwrap();
        let key=tmdb_cache_key("search/tv", &[("query","Pursuit of Jade"),("language","en-US")]);
        assert_eq!(key,tmdb_cache_key("search/tv", &[("language","en-US"),("query"," pursuit  OF jade ")]));
        let expected=json!({"results":[{"id":123,"name":"Pursuit of Jade","poster_path":"/poster.jpg"}]});
        std::fs::write(root.join("tmdb-cache").join(key),serde_json::to_vec(&expected).unwrap()).unwrap();
        for _ in 0..2 {
            let metadata=Metadata::new(&root).unwrap();
            // No token exists: cache misses fail before any remote request.
            assert_eq!(metadata.request("search/tv", &[("query","Pursuit of Jade"),("language","en-US")]).await.unwrap(),expected);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod prefixed_name_tests {
    use super::*;
    /// The 25 films of `007.James.Bond.Complete.Set.1962-2015.1080p.BluRay.x264-ETRG`, as
    /// folder/file names on Diamond, with the TMDB title and year each must resolve to.
    const BOND: [(&str, &str, &str); 25] = [
        ("1.Dr. No.1962/DR.No.1962.1080p.BluRay.x264.AC3-Ozlem.mp4", "Dr. No", "1962"),
        ("2.From.Russia.With.Love.1963/From.Russia.With.Love.1963.1080p.BluRay.x264.AC3-Ozlem.mp4", "From Russia with Love", "1963"),
        ("3.Goldfinger.1964/007.James.Bond.Goldfinger.1964.1080p.BluRay.x264.AC3-Ozlem.mp4", "Goldfinger", "1964"),
        ("4.Thunderball.1965/007.James.Bond.Thunderball.1965.1080p.BluRay.x264.AC3-Ozlem.mp4", "Thunderball", "1965"),
        ("5.You.Only.Live.Twice.1967/You.Only.Live.Twice.1967.1080p.BluRay.x264.AC3-Ozlem.mp4", "You Only Live Twice", "1967"),
        ("6.On.Her.Majesty's.Secret.Service.1969/007.James.Bond.On.Her.Majesty's.Secret.Service.1969.1080p.BluRay.x264.AC3-Ozlem.mp4", "On Her Majesty's Secret Service", "1969"),
        ("7.Diamonds.Are.Forever.1971/Diamonds.Are.Forever.1971.1080p.BluRay.x264.AAC-ETRG.mp4", "Diamonds Are Forever", "1971"),
        ("8.Live.and.Let.Die.1973/007.James.Bond.Live.and.Let.Die.1973.1080p.BluRay.x264.AC3-Ozlem.mp4", "Live and Let Die", "1973"),
        ("9.The.Man.with.the.Golden.Gun.1974/007.James.Bond.The.with.the.Golden.Gun.1974.1080p.BluRay.x264.AC3-Ozlem.mp4", "The Man with the Golden Gun", "1974"),
        ("10.The.Spy.Who.Loved.Me.1977/007.James.Bond.The.Spy.Who.Loved.Me.1977.1080p.BluRay.x264.AC3-Ozlem.mp4", "The Spy Who Loved Me", "1977"),
        ("11.Moonraker.1979/007.James.Bond.Moonraker.1979.1080p.BluRay.x264.AC3-Ozlem.mp4", "Moonraker", "1979"),
        ("12.For.Your.Eyes.Only.1981/007.James.Bond.For.Your.Eyes.Only.1981.1080p.BluRay.x264.AC3-Ozlem.mp4", "For Your Eyes Only", "1981"),
        ("13.Never.Say.Never.Again.1983/007.James.Bond.Never.Say.Never.Again.1983.1080p.BluRay.x264.AC3-Ozlem.mp4", "Never Say Never Again", "1983"),
        ("14.Octopussy.1983/James.Bond.007.Octopussy.1983.1080p.BluRay.x264.AAC-Ozlem.mp4", "Octopussy", "1983"),
        ("15.A.View.to.a.Kill.1985/007.James.Bond.A.View.to.a.Kill.1985.1080p.BluRay.x264.AAC-Ozlem.mp4", "A View to a Kill", "1985"),
        ("16.The.Living.Daylights.1987/007.James.Bond.The.Living.Daylights.1987.1080p.BluRay.x264.AAC-Ozlem.mp4", "The Living Daylights", "1987"),
        ("17.Licence.to.Kill.1989/007.James.Bond.Licence.to.Kill.1989.1080p.BluRay.x264.AAC-Ozlem.mp4", "Licence to Kill", "1989"),
        ("18.GoldenEye.1995/GoldenEye.1995.1080p.BluRay.x264.AAC-Ozlem.mp4", "GoldenEye", "1995"),
        ("19.Tomorrow.Never.Dies.1997/007.James.Bond.Tomorrow.Never.Dies.1997.1080p.BluRay.x264.AAC-Ozlem.mp4", "Tomorrow Never Dies", "1997"),
        ("20.The.World.Is.Not.Enough.1999/The.World.Is.Not.Enough.1999.1080p.BluRay.x264.AAC-Ozlem.mp4", "The World Is Not Enough", "1999"),
        ("21.Die.Another.Day.2002/007.James.Bond.Die.Another.Day.2002.1080p.BluRay.x264.AAC-Ozlem.mp4", "Die Another Day", "2002"),
        ("22.Casino.Royale.2006/Casino.Royale.2006.1080p.BluRay.x264.AAC-Ozlem.mp4", "Casino Royale", "2006"),
        ("23. Quantum.of.Solace.2008/007.James.Bond.Quantum.of.Solace.2008.1080p.BluRay.x264.AAC-Ozlem.mp4", "Quantum of Solace", "2008"),
        ("24.Skyfall.2012/Skyfall.2012.1080p.BluRay.x264.AAC-Ozlem.mp4", "Skyfall", "2012"),
        ("25.Spectre.2015/Spectre.2015.1080p.BRRip.x264.AAC-ETRG.mp4", "Spectre", "2015"),
    ];
    fn split(path: &str) -> (String, String) {
        let path = Path::new(path);
        (path.file_stem().unwrap().to_string_lossy().into_owned(),
         path.parent().unwrap().file_name().unwrap().to_string_lossy().into_owned())
    }
    /// The queries `identify` sends, in order, until one matches.
    fn queries(name: &str, folder: Option<&str>) -> Vec<(String, String)> {
        let (query, year, _) = parse_name(name);
        let mut all = vec![(query.clone(), year.clone())];
        all.extend(fallback_queries(&query, &year, name, folder));
        all
    }

    #[test]
    fn every_bond_film_reaches_its_title_and_year_within_the_query_cap() {
        for (path, title, year) in BOND {
            let (name, folder) = split(path);
            let sent = queries(&name, Some(&folder));
            assert!(sent.len() <= MAX_MATCH_QUERIES, "{path}: {sent:?}");
            let hit = sent.iter().position(|(q, y)| normalized(q) == normalized(title) && y == year);
            // Plain names match on the first query; prefixed names on the folder's title next.
            assert!(hit.is_some_and(|hit| hit <= 1), "{path} does not query {title} ({year}) early: {sent:?}");
        }
    }

    #[test]
    fn prefixed_names_without_a_folder_reach_the_title_by_dropping_leading_words() {
        for (path, title, year) in BOND {
            let (name, _) = split(path);
            if normalized(&name).contains("goldengun") { continue; } // the file name lost "Man"
            let sent = queries(&name, None);
            assert!(sent.len() <= MAX_MATCH_QUERIES, "{path}: {sent:?}");
            assert!(sent.iter().any(|(q, y)| normalized(q) == normalized(title) && y == year), "{path}: {sent:?}");
        }
        // The folder supplies the word the file name dropped.
        let sent = queries("007.James.Bond.The.with.the.Golden.Gun.1974.1080p.BluRay.x264.AC3-Ozlem", Some("9.The.Man.with.the.Golden.Gun.1974"));
        assert_eq!(sent[1], ("The Man with the Golden Gun".to_owned(), "1974".to_owned()));
    }

    #[test]
    fn packs_and_mismatched_folders_get_no_fallbacks() {
        assert_eq!(queries("007.James.Bond.Complete.Set.1962-2015.1080p.BluRay.x264-ETRG", None).len(), 1);
        // A collection folder is not a title, and a folder's different year is another film.
        let sent = queries("007.James.Bond.Moonraker.1979.1080p", Some("007.James.Bond.Complete.Set.1962-2015.1080p"));
        assert_eq!(sent.iter().map(|(q, _)| q.as_str()).collect::<Vec<_>>(),
            ["007 James Bond Moonraker", "James Bond Moonraker", "Bond Moonraker", "Moonraker"]);
        assert!(!queries("Bond.Moonraker.1979.1080p", Some("Moonraker.1980")).iter().any(|(_, y)| y == "1980"));
        // No year in the file or folder: an exact unique title is required as before.
        assert_eq!(queries("Some.Prefix.Movie.1080p", Some("Downloads")).len(), 1);
        // A year-less file in a dated folder can use the folder.
        assert_eq!(queries("movie", Some("The Movie (1999)"))[1], ("The Movie".to_owned(), "1999".to_owned()));
    }

    #[test]
    fn shortened_queries_only_accept_an_exact_title_and_year() {
        let hits = vec![
            json!({"provider_id":"1","title":"Moonraker","original_title":"Moonraker","year":"1979"}),
            json!({"provider_id":"2","title":"Moonraker","original_title":"Moonraker","year":"2011"}),
            json!({"provider_id":"3","title":"Moonraker Revisited","original_title":"","year":"1979"}),
        ];
        let exact = exact_candidates("Moonraker", "1979", &hits);
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0]["provider_id"], "1");
        assert!(exact_candidates("Moonraker", "1980", &hits).is_empty());
        assert!(exact_candidates("Bond Moonraker", "1979", &hits).is_empty());
        assert!(exact_candidates("", "1979", &[json!({"title":"","year":"1979"})]).is_empty());
        let offices = [json!({"title":"The Office","original_title":"The Office","year":"2005"}), json!({"title":"The Office","original_title":"The Office","year":"2001"})];
        assert_eq!(exact_candidates("The Office (US", "2005", &offices).len(), 1, "a country tag with the year");
        assert_eq!(exact_candidates("The Office US", "2001", &offices)[0]["year"], "2001");
        assert!(exact_candidates("The Office (US", "", &offices).is_empty(), "no year: the tag must match");
        // No year: the tag is the show's country ("Dancing.With.The.Stars.US.S35E05").
        let dances = [json!({"provider_id":"1508","title":"Dancing with the Stars","original_title":"Dancing with the Stars","year":"2005","country":"US"}),
                      json!({"provider_id":"9","title":"Dancing with the Stars","original_title":"Dancing with the Stars","year":"2004","country":"AU"})];
        let us = exact_candidates("Dancing With The Stars US", "", &dances);
        assert_eq!((us.len(), us[0]["provider_id"].clone()), (1, json!("1508")));
        assert_eq!(exact_candidates("Strictly UK", "", &[json!({"title":"Strictly","original_title":"Strictly","year":"2004","country":"GB"})]).len(), 1);
        assert!(exact_candidates("Dancing With The Stars NZ", "", &dances).is_empty());
        let deadpool = [json!({"provider_id":"533535","title":"Deadpool & Wolverine","original_title":"Deadpool & Wolverine","year":"2024"})];
        assert_eq!(exact_candidates("Deadpool and Wolverine", "2024", &deadpool).len(), 1, "& is and");
    }

    /// `identify` against recorded TMDB responses: no token, no network.
    #[test]
    fn a_file_holding_two_episodes_is_an_episode_of_its_series() {
        for name in ["Star Trek TNG S01E01E02 Encounter at Farpoint  (1080p x265 10bit Joy)", "Lost.S01E01-E02.Pilot.1080p", "Lost S01E01-02 Pilot"] {
            let (query, _, kind) = parse_name(name);
            assert_eq!(kind, "tv", "{name}");
            assert!(query == "Star Trek TNG" || query == "Lost", "{name}: {query}");
        }
    }
    #[tokio::test]
    async fn identify_stops_at_the_first_exact_match_from_recorded_responses() {
        let root = std::env::temp_dir().join(format!("torrbaby-identify-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("tmdb-cache")).unwrap();
        let record = |query: &[(&str, &str)], results: Value| {
            let mut query = query.to_vec();
            query.extend([("language", "en-US"), ("include_adult", "false")]);
            std::fs::write(root.join("tmdb-cache").join(tmdb_cache_key("search/movie", &query)),
                serde_json::to_vec(&json!({"page":1,"total_pages":1,"results":results})).unwrap()).unwrap();
        };
        // The prefixed name finds nothing; the folder's title finds the film and a namesake.
        record(&[("query", "007 James Bond The with the Golden Gun")], json!([]));
        record(&[("query", "The Man with the Golden Gun"), ("year", "1974")], json!([
            {"id":682,"title":"The Man with the Golden Gun","original_title":"The Man with the Golden Gun","release_date":"1974-12-01"},
            {"id":9,"title":"The Man with the Golden Gun","original_title":"","release_date":"2006-01-01"}]));
        let metadata = Metadata::new(&root).unwrap();
        let found = metadata.identify("007.James.Bond.The.with.the.Golden.Gun.1974.1080p.BluRay.x264.AC3-Ozlem",
            Some("9.The.Man.with.the.Golden.Gun.1974")).await.unwrap();
        assert_eq!(found["exact"]["provider_id"], "682");
        assert_eq!(found["matched_year"], "1974");
        assert_eq!(found["attempts"].as_array().unwrap().len(), 2);
        // Only a same-title film from another year: nothing is selected, and the cap holds.
        record(&[("query", "007 James Bond Moonraker")], json!([]));
        record(&[("query", "James Bond Moonraker"), ("year", "1979")], json!([]));
        record(&[("query", "Bond Moonraker"), ("year", "1979")], json!([]));
        record(&[("query", "Moonraker"), ("year", "1979")], json!([
            {"id":5,"title":"Moonraker","original_title":"Moonraker","release_date":"2011-01-01"}]));
        let found = metadata.identify("007.James.Bond.Moonraker.1979.1080p", None).await.unwrap();
        assert!(found["exact"].is_null());
        assert_eq!(found["attempts"].as_array().unwrap().len(), MAX_MATCH_QUERIES);
        assert_eq!(found["candidates"].as_array().unwrap().len(), 1);
        // The festival year against TMDB's release year: the one film of that name, a year on.
        record(&[("query", "Kill Me Three Times")], json!([
            {"id":274857,"title":"Kill Me Three Times","original_title":"Kill Me Three Times","release_date":"2015-04-10"},
            {"id":7,"title":"Tiny Times 3","original_title":"","release_date":"2014-07-17"}]));
        let found = metadata.identify("Kill.Me.Three.Times.2014.1080p.BluRay.x264.YIFY", None).await.unwrap();
        assert_eq!(found["exact"]["provider_id"], "274857");
        // Two of that name a year either side: no guess.
        record(&[("query", "Twins Apart")], json!([
            {"id":1,"title":"Twins Apart","original_title":"","release_date":"2013-01-01"},
            {"id":2,"title":"Twins Apart","original_title":"","release_date":"2015-01-01"}]));
        record(&[("query", "Apart"), ("year", "2014")], json!([]));
        // Known by another of its titles.
        record(&[("query", "F1 The Movie")], json!([
            {"id":911430,"title":"F1","original_title":"F1","release_date":"2025-06-25"},
            {"id":8,"title":"Silver Spoon","original_title":"","release_date":"2021-01-01"}]));
        std::fs::write(root.join("tmdb-cache").join(tmdb_cache_key("movie/911430/alternative_titles", &[])),
            serde_json::to_vec(&json!({"id":911430,"titles":[{"iso_3166_1":"US","title":"F1 The Movie"}]})).unwrap()).unwrap();
        let found = metadata.identify("F1.The.Movie.2025.2160p.WEB", None).await.unwrap();
        assert_eq!(found["exact"]["provider_id"], "911430");
        assert!(metadata.identify("Twins.Apart.2014.1080p", None).await.unwrap()["exact"].is_null());
        std::fs::remove_dir_all(root).unwrap();
    }
}
