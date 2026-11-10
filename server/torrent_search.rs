#[path = "chinese_index.rs"]
mod chinese;
// Read-only indexer search. Adding a result remains an explicit Transmission action.
use anyhow::{Context, Result, ensure};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Market { World, China }
impl Default for Market { fn default() -> Self { Self::World } }
impl Market {
    fn defaults(&self) -> Vec<String> { let sources: &[&str] = match self { Self::World => &["tpb", "csv"], Self::China => &["1lou"] }; sources.iter().map(|v| (*v).into()).collect() }
    fn allowed(&self) -> &[&str] { match self { Self::World => &["tpb", "csv", "nyaa"], Self::China => &["1lou", "dmhy", "mikan"] } }
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Query {
    #[serde(default)]
    pub market: Market,
    pub query: String,
    #[serde(default)]
    pub year: Option<u16>,
    #[serde(default)]
    pub season: Option<u16>,
    #[serde(default)]
    pub episode: Option<u16>,
    #[serde(default)]
    pub sources: Vec<String>,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Source {
    pub provider: String,
    pub id: String,
    pub url: String,
}
#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Hints {
    pub year: Option<u16>,
    pub season: Option<u16>,
    pub season_end: Option<u16>,
    pub episode: Option<u16>,
    pub episode_end: Option<u16>,
    pub episode_count: Option<u16>,
    pub resolution: Option<String>,
    pub video_codec: Option<String>,
    pub release_source: Option<String>,
    pub audio: Vec<String>,
    pub languages: Vec<String>,
    pub evidence: String,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Item {
    pub info_hash: String,
    pub name: String,
    pub magnet: String,
    pub size_bytes: Option<u64>,
    pub size_label: Option<String>,
    pub seeders: Option<u64>,
    pub leechers: Option<u64>,
    pub added: Option<String>,
    pub sources: Vec<Source>,
    pub hints: Hints,
    pub match_score: u16,
    pub match_notes: Vec<String>,
}
#[derive(Clone, Serialize)]
pub struct ProviderStatus {
    pub provider: String,
    pub count: usize,
    pub error: Option<String>,
}
#[derive(Clone, Serialize)]
pub struct SearchResult {
    pub query: String,
    pub items: Vec<Item>,
    pub providers: Vec<ProviderStatus>,
    pub cached: bool,
}
#[derive(Debug)]
struct RateLimited(u64);
impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f,"Source rate limited") }
}
impl std::error::Error for RateLimited {}

pub struct Search {
    lou: tokio::sync::Mutex<(Instant, HashMap<String, (Instant, Vec<Item>)>)>,
    http: reqwest::Client,
    cache: Mutex<HashMap<String, (Instant, SearchResult)>>,
    gate: tokio::sync::Semaphore,
    details: Mutex<HashMap<String, (Instant, Value)>>,
}
impl Search {
    pub fn new() -> Result<Self> {
        let mut builder = reqwest::Client::builder();
        if let Ok(proxy) = std::env::var("TORRBABY_SEARCH_PROXY") {
            ensure!(!proxy.trim().is_empty(), "TORRBABY_SEARCH_PROXY must not be empty");
            builder = builder.proxy(reqwest::Proxy::all(&proxy).map_err(|_| anyhow::anyhow!("Invalid search proxy configuration"))?);
        }
        Ok(Self {
            lou: tokio::sync::Mutex::new((Instant::now(), HashMap::new())),
            http: builder
                .user_agent("Torrbaby/0.3 (personal media library)")
                .connect_timeout(Duration::from_secs(8))
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            cache: Mutex::new(HashMap::new()),
            gate: tokio::sync::Semaphore::new(4),
            details: Mutex::new(HashMap::new()),
        })
    }
    async fn read(&self, url: reqwest::Url) -> Result<String> {
        Ok(String::from_utf8(self.read_bytes(url).await?)?)
    }
    async fn read_bytes(&self, url: reqwest::Url) -> Result<Vec<u8>> {
        match self.read_once(url.clone()).await {
            Ok(bytes) => Ok(bytes),
            Err(error) if error.downcast_ref::<reqwest::Error>().is_some_and(|e| e.is_connect() || e.is_body() || e.is_request()) => {
                tokio::time::sleep(Duration::from_millis(250)).await;
                self.read_once(url).await
            }
            Err(error) => Err(error),
        }
    }
    async fn read_once(&self, url: reqwest::Url) -> Result<Vec<u8>> {
        let mut request = self.http.get(url.clone());
        if url.host_str() == Some("www.1lou.me") { request = request.header(reqwest::header::CONNECTION, "close"); }
        let response = request.send().await?;
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let delay=response.headers().get(reqwest::header::RETRY_AFTER).and_then(|v|v.to_str().ok()).and_then(|s|s.parse::<u64>().ok()).unwrap_or(15).clamp(1,3600);
            return Err(RateLimited(delay).into());
        }
        let mut response = response.error_for_status()?;
        ensure!(
            response.content_length().is_none_or(|n| n <= 4_194_304),
            "Indexer response too large"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 4_194_304,
                "Indexer response too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    async fn search_lou(&self, terms: &str) -> Result<Vec<Item>> {
        // Serialize this source and share raw results across filters and clients.
        let mut state=self.lou.lock().await;
        state.1.retain(|_,(at,_)|at.elapsed()<Duration::from_secs(600));
        if let Some((_,items))=state.1.get(terms) { return Ok(items.clone()); }
        let wait=state.0.saturating_duration_since(Instant::now());
        if wait>Duration::from_secs(5) { return Err(RateLimited(wait.as_secs()+1).into()); }
        tokio::time::sleep(wait).await;
        let mut url=reqwest::Url::parse("http://www.1lou.me/search/api/search.php")?;
        url.query_pairs_mut().append_pair("q",terms).append_pair("sort","newest").append_pair("track","0");
        let result=match self.read(url).await { Ok(raw)=>chinese::listings(&raw),Err(e)=>Err(e) };
        let delay=result.as_ref().err().and_then(|e|e.downcast_ref::<RateLimited>()).map(|e|e.0).unwrap_or(5);
        state.0=Instant::now()+Duration::from_secs(delay);
        match &result {
            Ok(items)=> { if state.1.len()>=32 {state.1.clear();} state.1.insert(terms.to_owned(),(Instant::now(),items.clone())); },
            Err(error)=> {
                let class=if error.downcast_ref::<RateLimited>().is_some() {"rate_limited"}
                    else if let Some(e)=error.downcast_ref::<reqwest::Error>() {if e.is_timeout(){"timeout"}else if e.is_connect(){"connection"}else if e.status().is_some(){"http_status"}else{"transport"}}
                    else {"invalid_response"};
                eprintln!("Torrent search provider=1lou failure={class}");
            }
        }
        result
    }
    pub async fn search(&self, mut query: Query) -> Result<SearchResult> {
        query.query = query.query.split_whitespace().collect::<Vec<_>>().join(" ");
        ensure!(
            (2..=200).contains(&query.query.chars().count()),
            "Enter between 2 and 200 characters"
        );
        ensure!(
            query.year.is_none_or(|n| (1880..=2100).contains(&n)),
            "Invalid year"
        );
        ensure!(
            query.episode.is_none() || query.season.is_some(),
            "Choose a season with an episode"
        );
        if query.sources.is_empty() {
            query.sources = query.market.defaults();
        }
        query.sources.sort();
        query.sources.dedup();
        ensure!(
            query
                .sources
                .iter()
                .all(|p| query.market.allowed().contains(&p.as_str())),
            "Unsupported search source"
        );
        let key = serde_json::to_string(&query)?;
        if let Some((at, result)) = self
            .cache
            .lock()
            .map_err(|_| anyhow::anyhow!("Search cache unavailable"))?
            .get(&key)
        {
            if at.elapsed() < Duration::from_secs(120) {
                let mut result = result.clone();
                result.cached = true;
                return Ok(result);
            }
        }
        let _permit = self
            .gate
            .try_acquire()
            .context("Search is busy. Try again shortly.")?;
        // Keep title searches broad: requiring S02 would hide "Season 2" and
        // S01-S04 packs. Rank the requested season/episode locally instead.
        let terms = query.query.clone();
        let responses = futures_util::future::join_all(query.sources.iter().map(|provider| {
            let terms = &terms;
            async move {
                let result = if provider == "tpb" {
                    let mut url = reqwest::Url::parse("https://apibay.org/q.php").unwrap();
                    url.query_pairs_mut()
                        .append_pair("q", &terms)
                        .append_pair("cat", "0");
                    match self.read(url).await {
                        Ok(raw) => parse_tpb(&raw),
                        Err(e) => Err(e),
                    }
                } else if provider == "csv" {
                    let mut url =
                        reqwest::Url::parse("https://torrents-csv.com/service/search").unwrap();
                    url.query_pairs_mut()
                        .append_pair("q", terms)
                        .append_pair("size", "100");
                    match self.read(url).await {
                        Ok(raw) => parse_csv(&raw),
                        Err(e) => Err(e),
                    }
                } else if provider == "1lou" {
                    self.search_lou(terms).await
                } else if provider == "dmhy" || provider == "mikan" {
                    let (base, key) = if provider == "dmhy" {
                        ("https://share.dmhy.org/topics/rss/rss.xml", "keyword")
                    } else { ("https://mikanani.me/RSS/Search", "searchstr") };
                    let mut url = reqwest::Url::parse(base).unwrap();
                    url.query_pairs_mut().append_pair(key, terms);
                    match self.read(url).await {
                        Ok(raw) => parse_chinese_feed(&raw, provider),
                        Err(e) => Err(e),
                    }
                } else {
                    let mut url = reqwest::Url::parse("https://nyaa.si/").unwrap();
                    url.query_pairs_mut()
                        .append_pair("page", "rss")
                        .append_pair("q", &terms)
                        .append_pair("c", "0_0")
                        .append_pair("f", "0");
                    match self.read(url).await {
                        Ok(raw) => parse_nyaa(&raw),
                        Err(e) => Err(e),
                    }
                };
                (provider.clone(), result)
            }
        }))
        .await;
        let mut providers = Vec::new();
        let mut unique = HashMap::<String, Item>::new();
        for (provider, response) in responses {
            match response {
                Ok(items) => {
                    providers.push(ProviderStatus {
                        provider,
                        count: items.len(),
                        error: None,
                    });
                    for mut item in items {
                        score(&mut item, &query);
                        let key = if item.info_hash.is_empty() { format!("{}:{}", item.sources[0].provider, item.sources[0].id) } else { item.info_hash.clone() };
                        if let Some(existing) = unique.get_mut(&key) {
                            if item.match_score > existing.match_score
                                || (item.match_score == existing.match_score
                                    && item.name.len() > existing.name.len())
                            {
                                std::mem::swap(existing, &mut item);
                            }
                            existing.sources.extend(item.sources);
                            existing.seeders = existing.seeders.max(item.seeders);
                            existing.leechers = existing.leechers.max(item.leechers);
                            if existing.size_bytes.is_none() {
                                existing.size_bytes = item.size_bytes;
                            }
                        } else {
                            unique.insert(key, item);
                        }
                    }
                }
                Err(error) => {
                    // Keep remote bodies/URLs out of client errors and logs.
                    let message = if error.downcast_ref::<RateLimited>().is_some() {
                        "Source is rate limiting searches. Please wait briefly and try again."
                    } else if error
                        .downcast_ref::<reqwest::Error>()
                        .is_some_and(|e| e.is_timeout())
                    {
                        "Source timed out"
                    } else {
                        "Source is unavailable"
                    };
                    providers.push(ProviderStatus {
                        provider,
                        count: 0,
                        error: Some(message.into()),
                    });
                }
            }
        }
        let mut items: Vec<_> = unique.into_values().collect();
        items.sort_by(|a, b| {
            b.match_score
                .cmp(&a.match_score)
                .then_with(|| b.seeders.cmp(&a.seeders))
                .then_with(|| a.name.cmp(&b.name))
        });
        items.truncate(200);
        let result = SearchResult {
            query: query.query,
            items,
            providers,
            cached: false,
        };
        if result.providers.iter().any(|p| p.error.is_none()) {
            let mut cache = self
                .cache
                .lock()
                .map_err(|_| anyhow::anyhow!("Search cache unavailable"))?;
            cache.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(120));
            if cache.len() >= 32 {
                cache.clear();
            }
            cache.insert(key, (Instant::now(), result.clone()));
        }
        Ok(result)
    }
    pub async fn detail(&self, provider: &str, id: &str) -> Result<Value> {
        if provider == "1lou" {
            ensure!(!id.is_empty() && id.len() <= 12 && id.bytes().all(|b| b.is_ascii_digit()), "Invalid listing ID");
            if let Some((at, value)) = self.details.lock().map_err(|_| anyhow::anyhow!("Search cache unavailable"))?.get(id) {
                if at.elapsed() < Duration::from_secs(600) { return Ok(value.clone()); }
            }
            let _permit = self.gate.try_acquire().context("Search is busy. Try again shortly.")?;
            let url = format!("http://www.1lou.me/thread-{id}.htm");
            let raw = self.read(reqwest::Url::parse(&url)?).await?;
            let (description, attachments) = chinese::thread(&raw)?;
            ensure!(!attachments.is_empty(), "This listing has no downloadable torrent attachment");
            use futures_util::StreamExt;
            ensure!(attachments.len() <= 8, "This post contains too many attachments; open a more specific listing");
            let releases = futures_util::stream::iter(attachments.into_iter().map(|attachment| {
                let url = &url;
                async move {
                    let bytes = self.read_bytes(reqwest::Url::parse(&format!("http://www.1lou.me/attach-download-{attachment}.htm"))?).await?;
                    chinese::torrent(&bytes, id, url)
                }
            })).buffered(4).collect::<Vec<Result<Value>>>().await.into_iter().collect::<Result<Vec<_>>>()?;
            let value = json!({"description": description, "releases":releases, "files":null, "evidence":"torrent_metadata", "media_verified":false});
            let mut cache = self.details.lock().map_err(|_| anyhow::anyhow!("Search cache unavailable"))?;
            cache.retain(|_,(at,_)| at.elapsed() < Duration::from_secs(600));
            if cache.len() >= 32 { cache.clear(); }
            cache.insert(id.to_owned(), (Instant::now(), value.clone()));
            return Ok(value);
        }
        if provider == "dmhy" || provider == "mikan" {
            ensure!(hash(id).is_some(), "Invalid listing ID");
            return Ok(json!({"description":"", "files":null,"evidence":"source_listing","media_verified":false}));
        }
        ensure!(
            !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit()),
            "Invalid listing ID"
        );
        ensure!(
            ["tpb", "nyaa", "csv"].contains(&provider),
            "Unsupported search source"
        );
        let _permit = self
            .gate
            .try_acquire()
            .context("Search is busy. Try again shortly.")?;
        if provider == "csv" {
            return Ok(
                json!({"description":"This index does not provide a description or file list.",
                "files":null,"evidence":"source_listing","media_verified":false}),
            );
        }
        if provider == "nyaa" {
            let raw = self
                .read(reqwest::Url::parse(&format!("https://nyaa.si/view/{id}"))?)
                .await?;
            return parse_nyaa_detail(&raw);
        }
        let mut url = reqwest::Url::parse("https://apibay.org/t.php")?;
        url.query_pairs_mut().append_pair("id", id);
        let raw = self.read(url).await?;
        let listing: Value = serde_json::from_str(&raw)?;
        let item = tpb_item(&listing).context("Listing is no longer available")?;
        let mut url = reqwest::Url::parse("https://apibay.org/f.php")?;
        url.query_pairs_mut().append_pair("id", id);
        let files = match self.read(url).await {
            Ok(raw) => parse_tpb_files(&raw).ok(),
            Err(_) => None,
        };
        Ok(
            json!({"item":item,"description":field(&listing,"descr"),"audio_language":listing["language"],
            "subtitle_language":listing["textlanguage"],"files":files,"evidence":"source_listing",
            "media_verified":false}),
        )
    }
}
fn field(v: &Value, key: &str) -> String {
    match &v[key] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}
fn number(v: &Value, key: &str) -> Option<u64> {
    v[key].as_u64().or_else(|| v[key].as_str()?.parse().ok())
}
fn hash(value: &str) -> Option<String> {
    (value.len() == 40
        && value.bytes().all(|b| b.is_ascii_hexdigit())
        && value.bytes().any(|b| b != b'0'))
    .then(|| value.to_ascii_lowercase())
}
fn item(name: String, info_hash: String, source: Source) -> Item {
    let mut encoded_name = reqwest::Url::parse("magnet:?").unwrap();
    encoded_name.query_pairs_mut().append_pair("dn", &name);
    // Transmission expects the canonical, unescaped xt URN. Encode only display text.
    let magnet = format!(
        "magnet:?xt=urn:btih:{info_hash}&{}",
        encoded_name.query().unwrap_or("")
    );
    Item {
        hints: hints(&name),
        name,
        info_hash,
        magnet,
        sources: vec![source],
        size_bytes: None,
        size_label: None,
        seeders: None,
        leechers: None,
        added: None,
        match_score: 0,
        match_notes: Vec::new(),
    }
}
fn tpb_item(v: &Value) -> Option<Item> {
    let info_hash = hash(&field(v, "info_hash"))?;
    let id = field(v, "id");
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let name = field(v, "name");
    if name.is_empty() || name.len() > 4096 {
        return None;
    }
    let mut row = item(
        name,
        info_hash,
        Source {
            provider: "tpb".into(),
            url: format!("https://thepiratebay.org/description.php?id={id}"),
            id,
        },
    );
    row.size_bytes = number(v, "size");
    row.seeders = number(v, "seeders");
    row.leechers = number(v, "leechers");
    row.added = number(v, "added").map(|n| n.to_string());
    Some(row)
}
fn parse_tpb(raw: &str) -> Result<Vec<Item>> {
    let data: Vec<Value> = serde_json::from_str(raw)?;
    Ok(data.iter().take(200).filter_map(tpb_item).collect())
}
fn parse_csv(raw: &str) -> Result<Vec<Item>> {
    let data: Value = serde_json::from_str(raw)?;
    let rows = data["torrents"]
        .as_array()
        .context("Invalid index response")?;
    Ok(rows
        .iter()
        .take(200)
        .filter_map(|v| {
            let info_hash = hash(&field(v, "infohash"))?;
            let id = number(v, "id")?.to_string();
            let name = field(v, "name");
            if name.is_empty() || name.len() > 4096 {
                return None;
            }
            let mut row = item(
                name,
                info_hash,
                Source {
                    provider: "csv".into(),
                    id,
                    url: "https://torrents-csv.com".into(),
                },
            );
            row.size_bytes = number(v, "size_bytes");
            row.seeders = number(v, "seeders");
            row.leechers = number(v, "leechers");
            row.added = number(v, "created_unix").map(|n| n.to_string());
            Some(row)
        })
        .collect())
}
fn parse_tpb_files(raw: &str) -> Result<Vec<Value>> {
    let data: Vec<Value> = serde_json::from_str(raw)?;
    let mut files = Vec::new();
    for group in data.iter().take(2000) {
        let names = group["name"].as_array().context("Invalid file list")?;
        let sizes = group["size"].as_array().context("Invalid file sizes")?;
        for (name, size) in names.iter().zip(sizes).take(2000 - files.len()) {
            if let Some(name) = name.as_str().filter(|n| n.len() <= 4096) {
                files.push(json!({"name":name,"size_bytes":size.as_u64(),"hints":hints(name)}));
            }
        }
    }
    Ok(files)
}
#[derive(Deserialize)]
struct Feed {
    channel: Channel,
}
#[derive(Deserialize)]
struct Channel {
    #[serde(default, rename = "item")]
    items: Vec<RssItem>,
}
#[derive(Deserialize)]
struct RssItem {
    title: String,
    guid: String,
    #[serde(rename = "infoHash")]
    info_hash: String,
    #[serde(default)]
    seeders: Option<u64>,
    #[serde(default)]
    leechers: Option<u64>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default, rename = "pubDate")]
    added: Option<String>,
}
fn parse_nyaa(raw: &str) -> Result<Vec<Item>> {
    let feed: Feed = quick_xml::de::from_str(raw)?;
    Ok(feed
        .channel
        .items
        .into_iter()
        .take(200)
        .filter_map(|r| {
            let info_hash = hash(&r.info_hash)?;
            let url = reqwest::Url::parse(&r.guid).ok()?;
            if url.host_str() != Some("nyaa.si") {
                return None;
            }
            let id = url.path().strip_prefix("/view/")?.to_owned();
            if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) || r.title.len() > 4096 {
                return None;
            }
            let mut row = item(
                r.title,
                info_hash,
                Source {
                    provider: "nyaa".into(),
                    id,
                    url: r.guid,
                },
            );
            row.seeders = r.seeders;
            row.leechers = r.leechers;
            row.size_bytes = r.size.as_deref().and_then(parse_size);
            row.size_label = r.size;
            row.added = r.added;
            Some(row)
        })
        .collect())
}
// Public RSS feeds: normalize hashes without downloading torrent payloads.
#[derive(Deserialize)]
struct ChineseFeed { channel: ChineseChannel }
#[derive(Deserialize)]
struct ChineseChannel { #[serde(default, rename="item")] items: Vec<ChineseRow> }
#[derive(Deserialize)]
struct ChineseRow {
    title: String, link: String, enclosure: Enclosure,
    #[serde(default, rename="pubDate")] added: Option<String>,
}
#[derive(Deserialize)]
struct Enclosure {
    #[serde(rename="@url")] url: String,
    #[serde(default, rename="@length")] length: Option<u64>,
}
fn magnet_hash(value: &str) -> Option<String> {
    let url = reqwest::Url::parse(value).ok()?;
    if url.scheme() != "magnet" { return None; }
    let xt = url.query_pairs().find(|(k,_)| k == "xt")?.1.into_owned();
    let value = xt.strip_prefix("urn:btih:")?;
    if let Some(h) = hash(value) { return Some(h); }
    if value.len() != 32 { return None; }
    let mut bits = 0u32; let mut count = 0; let mut bytes = Vec::new();
    for c in value.bytes() {
        let n = match c.to_ascii_uppercase() { b'A'..=b'Z' => c.to_ascii_uppercase()-b'A', b'2'..=b'7' => c-b'2'+26, _=>return None };
        bits = (bits << 5) | u32::from(n); count += 5;
        if count >= 8 { count -= 8; bytes.push((bits >> count) as u8); }
    }
    hash(&bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
}
fn parse_chinese_feed(raw: &str, provider: &str) -> Result<Vec<Item>> {
    let feed: ChineseFeed = quick_xml::de::from_str(raw)?;
    Ok(feed.channel.items.into_iter().take(200).filter_map(|r| {
        if r.title.is_empty() || r.title.len() > 4096 { return None; }
        let mut url = reqwest::Url::parse(&r.link).ok()?;
        let expected = if provider == "dmhy" { "share.dmhy.org" } else { "mikanani.me" };
        if url.host_str() != Some(expected) || !["https","http"].contains(&url.scheme()) { return None; }
        url.set_scheme("https").ok()?;
        let info_hash = if provider == "dmhy" { magnet_hash(&r.enclosure.url)? }
            else { hash(url.path().strip_prefix("/Home/Episode/")?)? };
        let mut row = item(r.title, info_hash.clone(), Source { provider:provider.into(), id:info_hash, url:url.to_string() });
        if provider == "dmhy" { row.magnet = r.enclosure.url; }
        row.size_bytes = r.enclosure.length.filter(|n| *n>0);
        row.added = r.added;
        Some(row)
    }).collect())
}
fn html_text(raw: &str) -> String {
    let raw = raw.replace("&nbsp;", " ");
    quick_xml::escape::unescape(&raw)
        .map(|s| s.into_owned())
        .unwrap_or(raw)
}
fn parse_nyaa_detail(raw: &str) -> Result<Value> {
    let dom = tl::parse(raw, tl::ParserOptions::default())?;
    let parser = dom.parser();
    let description = dom
        .get_element_by_id("torrent-description")
        .and_then(|n| n.get(parser))
        .map(|n| {
            html_text(&n.inner_text(parser))
                .chars()
                .take(32000)
                .collect::<String>()
        })
        .context("Source details missing")?;
    let root = dom
        .query_selector(".torrent-file-list")
        .and_then(|mut it| it.next())
        .and_then(|n| n.get(parser))
        .context("Source file list missing")?;
    fn walk(
        node: &tl::Node<'_>,
        parser: &tl::Parser<'_>,
        prefix: &str,
        depth: usize,
        files: &mut Vec<Value>,
    ) {
        if depth > 32 || files.len() >= 2000 {
            return;
        }
        let Some(tag) = node.as_tag() else {
            return;
        };
        if tag.name().as_utf8_str() == "li" {
            let mut name = String::new();
            let mut size = None;
            let mut folders = Vec::new();
            for child in tag.children().top().iter().filter_map(|h| h.get(parser)) {
                if let Some(t) = child.as_tag() {
                    if t.name().as_utf8_str() == "ul" {
                        folders.push(child);
                        continue;
                    }
                    if t.attributes().get("class").flatten().is_some_and(|v| {
                        v.as_utf8_str().split_whitespace().any(|c| c == "file-size")
                    }) {
                        let text = child.inner_text(parser);
                        size = parse_size(text.trim().trim_matches(|c| c == '(' || c == ')'));
                        continue;
                    }
                }
                name.push_str(&child.inner_text(parser));
            }
            let name = html_text(name.trim());
            let path = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if folders.is_empty() {
                if !path.is_empty() {
                    files.push(json!({"name":path,"size_bytes":size,"hints":hints(&path)}));
                }
            } else {
                for folder in folders {
                    walk(folder, parser, &path, depth + 1, files);
                }
            }
        } else {
            for child in tag.children().top().iter().filter_map(|h| h.get(parser)) {
                walk(child, parser, prefix, depth + 1, files);
            }
        }
    }
    let mut files = Vec::new();
    walk(root, parser, "", 0, &mut files);
    Ok(
        json!({"description":description,"files":files,"evidence":"source_listing","media_verified":false}),
    )
}

fn parse_size(raw: &str) -> Option<u64> {
    let mut fields = raw.split_whitespace();
    let amount: f64 = fields.next()?.parse().ok()?;
    let scale = match fields.next()? {
        "B" => 1.,
        "KiB" => 1024.,
        "MiB" => 1_048_576.,
        "GiB" => 1_073_741_824.,
        "TiB" => 1_099_511_627_776.,
        _ => return None,
    };
    (amount.is_finite() && amount >= 0. && amount * scale <= u64::MAX as f64)
        .then_some((amount * scale) as u64)
}
fn hints(name: &str) -> Hints {
    static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b((?:19|20)\d{2})\b").unwrap());
    static EPISODE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\bS(\d{1,2})(?:[ ._-]*E(\d{1,3}))?\b").unwrap());
    static SEASON_WORD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\bSeasons?[ ._-]+(\d{1,2})(?:[ ._-]*(?:to|-)[ ._-]*(\d{1,2}))?\b").unwrap()
    });
    static SEASON_RANGE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\bS(\d{1,2})[ ._-]*(?:-|to)[ ._-]*S?(\d{1,2})\b").unwrap()
    });
    static EPISODE_RANGE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\bS\d{1,2}[ ._-]*E(\d{1,3})(?:[ ._-]*-[ ._-]*E?|E)(\d{1,3})\b").unwrap()
    });
    static RES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\b(2160p|1080p|1080i|720p|480p|4k)\b").unwrap());
    static CODEC: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(x265|h[ .]?265|hevc|x264|h[ .]?264|av1|xvid)\b").unwrap()
    });
    static SOURCE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(WEB[ ._-]?DL|WEBRip|BluRay|BDRip|Remux|HDTV|DVD(?:Rip)?)\b").unwrap()
    });
    static AUDIO: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\b(AAC(?:[ .]?[257]\.[01])?|E[ ._-]?AC[ ._-]?3|AC[ ._-]?3|DTS(?:[ ._-]HD)?(?:[ ._-]MA)?|TrueHD|Atmos|FLAC|DDP?(?:[ .]?[257]\.1)?)\b",
        )
        .unwrap()
    });
    static LANG: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\b(ENG|CHS|CHT|JPN|MULTI|DUAL[ ._-]AUDIO)\b").unwrap());
    static COMPLETE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"全(\d{1,3})集").unwrap());
    static CHINESE_EPISODES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"第(\d{1,3})(?:[-—至](\d{1,3}))?集").unwrap());
    let episode_count = COMPLETE.captures(name).and_then(|c| c[1].parse::<u16>().ok()).filter(|n| *n > 0);
    let chinese_episodes = CHINESE_EPISODES.captures(name);
    let mut languages: Vec<String> = LANG.find_iter(name).map(|m|m.as_str().to_ascii_uppercase()).collect();
    if name.contains("国语") { languages.push("Mandarin audio".into()); }
    if name.contains("粤语") { languages.push("Cantonese audio".into()); }
    if name.contains("简繁英字幕") { languages.push("Chinese (Simplified/Traditional) + English subtitles".into()); }
    else if name.contains("简繁字幕") { languages.push("Chinese (Simplified/Traditional) subtitles".into()); }
    else if name.contains("中文字幕") { languages.push("Chinese subtitles".into()); }
    let ep = EPISODE.captures(name);
    let season_range = SEASON_RANGE
        .captures(name)
        .or_else(|| SEASON_WORD.captures(name));
    let episode_range = EPISODE_RANGE.captures(name);
    let season = season_range
        .as_ref()
        .and_then(|c| c[1].parse().ok())
        .or_else(|| ep.as_ref().and_then(|c| c[1].parse().ok()));
    let episode = episode_range
        .as_ref()
        .and_then(|c| c[1].parse().ok())
        .or_else(|| ep.as_ref().and_then(|c| c.get(2)?.as_str().parse().ok()))
        .or_else(|| chinese_episodes.as_ref().and_then(|c| c[1].parse().ok()));
    Hints {
        year: YEAR.captures(name).and_then(|c| c[1].parse().ok()),
        season,
        season_end: season_range
            .as_ref()
            .and_then(|c| c.get(2)?.as_str().parse().ok())
            .filter(|end| season.is_some_and(|start| *end >= start)),
        episode,
        episode_count,
        episode_end: episode_range
            .as_ref()
            .and_then(|c| c[2].parse().ok())
            .or_else(|| chinese_episodes.as_ref().and_then(|c| c.get(2)?.as_str().parse().ok()))
            .filter(|end| episode.is_some_and(|start| *end >= start)),
        resolution: RES.find(name).map(|m| m.as_str().to_ascii_lowercase()),
        video_codec: CODEC.find(name).map(|m| m.as_str().to_ascii_uppercase()),
        release_source: SOURCE.find(name).map(|m| m.as_str().to_owned()),
        audio: AUDIO
            .find_iter(name)
            .map(|m| m.as_str().to_ascii_uppercase())
            .collect(),
        languages,
        evidence: "release_name".into(),
    }
}
fn words(s: &str) -> HashSet<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect()
}
fn score(item: &mut Item, query: &Query) {
    let wanted = words(&query.query);
    let found = words(&item.name);
    let overlap = wanted.intersection(&found).count();
    let mut score = (70 * overlap / wanted.len().max(1)) as i32;
    if item.hints.episode_count.is_some() && query.episode.is_none() { score += 5; }
    if overlap == wanted.len() {
        item.match_notes.push("Title words match".into());
    }
    // Prefer the requested title over a sequel or collection even when an
    // indexer mislabels its year (e.g. "The Matrix Reloaded (1999)").
    static TITLE_END: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(?:(?:19|20)\d{2}|S\d{1,2}(?:E\d+)?|Seasons?\s+\d|2160p|1080p|720p)\b")
            .unwrap()
    });
    if let Some(marker) = TITLE_END.find(&item.name) {
        let title_words = words(&item.name[..marker.start()]);
        let extra = title_words.difference(&wanted).count();
        score -= (extra * 8).min(24) as i32;
    }
    for (label, want, got, end) in [
        ("Year", query.year, item.hints.year, None),
        (
            "Season",
            query.season,
            item.hints.season,
            item.hints.season_end,
        ),
        (
            "Episode",
            query.episode,
            item.hints.episode,
            item.hints.episode_end,
        ),
    ] {
        if let Some(want) = want {
            match got {
                Some(got) if got == want => {
                    score += 10;
                    item.match_notes.push(format!("{label} matches"));
                }
                Some(start) if end.is_some_and(|end| (start..=end).contains(&want)) => {
                    score += 8;
                    item.match_notes.push(format!("{label} included in pack"));
                }
                Some(_) => {
                    score -= 45;
                    item.match_notes
                        .push(format!("Different {}", label.to_lowercase()));
                }
                None => item.match_notes.push(format!("{} not stated", label)),
            }
        }
    }
    item.match_score = score.clamp(0, 100) as u16;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chinese_release_hints_distinguish_complete_and_partial_packs() {
        let complete = hints("逐玉[全40集][国语音轨/简繁英字幕].Pursuit.of.Jade.S01.2026.1080p.IQ.WEB-DL.H264.AAC-BlackTV");
        assert_eq!(complete.episode_count, Some(40));
        assert_eq!(complete.season, Some(1));
        assert!(complete.languages.contains(&"Mandarin audio".into()));
        let partial = hints("逐玉[第39-40集][中文字幕].Pursuit.of.Jade.S01.1080p");
        assert_eq!(partial.episode_count, None);
        assert_eq!(partial.episode, Some(39));
        assert_eq!(partial.episode_end, Some(40));
    }
    #[test]
    fn chinese_feeds_and_market_partition() {
        let old: Query = serde_json::from_str(r#"{"query":"example"}"#).unwrap();
        assert_eq!(old.market.defaults(), vec!["tpb", "csv"]);
        assert!(!Market::China.allowed().contains(&"tpb"));
        assert!(!Market::World.allowed().contains(&"dmhy"));
        let raw = r#"<rss><channel><item><title>三体 1080p AAC</title><link>http://share.dmhy.org/topics/view/637444_example.html</link><enclosure url="magnet:?xt=urn:btih:WD2Z2SVVPK66W2UXOPWWLAF3R233GF5D" length="123"/></item></channel></rss>"#;
        let rows = parse_chinese_feed(raw, "dmhy").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].info_hash, "b0f59d4ab57abdeb6a9773ed6580bb8eb7b317a3");
        assert_eq!(rows[0].size_bytes, Some(123));
        assert!(rows[0].sources[0].url.starts_with("https://share.dmhy.org/"));
        assert!(parse_chinese_feed(&raw.replace("share.dmhy.org", "example.com"), "dmhy").unwrap().is_empty());
        let raw = r#"<rss><channel><item><title>三体</title><link>https://mikanani.me/Home/Episode/27e60d74cc5b47ba2d9110450f66065cbee3173e</link><enclosure url="https://mikanani.me/Download/file.torrent" length="343366688"/></item></channel></rss>"#;
        let rows = parse_chinese_feed(raw, "mikan").unwrap();
        assert_eq!(rows[0].info_hash, "27e60d74cc5b47ba2d9110450f66065cbee3173e");
        assert_eq!(rows[0].size_bytes, Some(343366688));
    }
    #[test]
    fn matching_does_not_promote_the_wrong_episode() {
        let source = Source {
            provider: "tpb".into(),
            id: "1".into(),
            url: "https://example.com".into(),
        };
        let q = Query {
            market: Market::World,
            query: "Example Show".into(),
            year: None,
            season: Some(2),
            episode: Some(3),
            sources: vec![],
        };
        let mut right = item(
            "Example.Show.S02E03.1080p.WEB-DL.DDP5.1.x265".into(),
            "a".repeat(40),
            source.clone(),
        );
        let mut pack = item(
            "Example Show S01-S04 Complete".into(),
            "c".repeat(40),
            source.clone(),
        );
        let mut word_pack = item(
            "Example Show Season 1 to 8".into(),
            "d".repeat(40),
            source.clone(),
        );
        let mut wrong = item("Example.Show.S01E03.1080p".into(), "b".repeat(40), source);
        score(&mut pack, &q);
        score(&mut word_pack, &q);
        score(&mut right, &q);
        score(&mut wrong, &q);
        assert!(right.match_score > wrong.match_score);
        assert!(pack.match_score > wrong.match_score);
        assert!(word_pack.match_score > wrong.match_score);
        assert_eq!(hints("Example S02E03-E05").episode_end, Some(5));
        let mut sequel = right.clone();
        sequel.name = "Example Show Returns S02E03 1080p".into();
        score(&mut sequel, &q);
        assert!(right.match_score > sequel.match_score);
        assert_eq!(right.hints.episode, Some(3));
        assert_eq!(right.hints.audio, vec!["DDP5.1"]);
        assert_eq!(hints("Film.2020.AAC2.0.E-AC-3.TrueHD.Atmos").audio,
            vec!["AAC2.0", "E-AC-3", "TRUEHD", "ATMOS"]);
        let url = reqwest::Url::parse(&right.magnet).unwrap();
        assert_eq!(url.scheme(), "magnet");
        assert!(right.magnet.starts_with("magnet:?xt=urn:btih:"));
        assert!(
            url.query_pairs()
                .any(|(k, v)| k == "xt" && v == format!("urn:btih:{}", "a".repeat(40)))
        );
    }
    #[test]
    fn feeds_validate_hashes_and_preserve_file_metadata() {
        assert!(parse_tpb(r#"[{"id":"0","name":"No results","info_hash":"0000000000000000000000000000000000000000"}]"#).unwrap().is_empty());
        let csv = parse_csv(r#"{"torrents":[{"id":1,"name":"Sample","infohash":"abcdefabcdefabcdefabcdefabcdefabcdefabcd","seeders":2,"size_bytes":300}]}"#).unwrap();
        assert_eq!(csv[0].seeders, Some(2));
        assert_eq!(csv[0].sources[0].provider, "csv");
        let xml = r#"<rss xmlns:nyaa="https://nyaa.si/xmlns/nyaa"><channel><item><title>Example &amp; Sample</title><guid>https://nyaa.si/view/123</guid><nyaa:infoHash>abcdefabcdefabcdefabcdefabcdefabcdefabcd</nyaa:infoHash><nyaa:seeders>8</nyaa:seeders><nyaa:size>1.5 GiB</nyaa:size></item></channel></rss>"#;
        let rows = parse_nyaa(xml).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].size_bytes, Some(1_610_612_736));
        assert_eq!(rows[0].name, "Example & Sample");
        assert_eq!(
            parse_tpb_files(r#"[{"name":["movie.en.srt"],"size":[300]}]"#).unwrap()[0]["size_bytes"],
            300
        );
        let html = r#"<div id="torrent-description">English &amp; Japanese</div><div class="torrent-file-list"><ul><li><a>Season 1</a><ul><li><i></i>Example.S01E01.mkv <span class="file-size">(1.5 GiB)</span></li></ul></li></ul></div>"#;
        let detail = parse_nyaa_detail(html).unwrap();
        assert_eq!(detail["description"], "English & Japanese");
        assert_eq!(detail["files"][0]["name"], "Season 1/Example.S01E01.mkv");
        assert_eq!(detail["files"][0]["size_bytes"], 1_610_612_736u64);
    }
}
