#[path = "chinese_index.rs"]
mod chinese;
// Read-only indexer search. Adding a result remains an explicit Transmission action.
use anyhow::{Context, Result, ensure};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Market { World, China }
impl Default for Market { fn default() -> Self { Self::World } }
impl Market {
    fn defaults(&self) -> Vec<String> { let sources: &[&str] = match self { Self::World => &["tpb", "csv"], Self::China => &["1lou"] }; sources.iter().map(|v| (*v).into()).collect() }
    fn allowed(&self) -> &[&str] { match self { Self::World => &["tpb", "csv", "nyaa"], Self::China => &["1lou", "dmhy", "mikan", "pt"] } }
}
/// A private site the user belongs to, of the NexusPHP kind most Chinese private
/// trackers run: its address and the member's passkey. The passkey authenticates the
/// site's search feed and torrent downloads; it stays on this server and is never
/// returned to a client, logged, or placed in a result.
#[derive(Clone, Deserialize, Serialize)]
pub struct PrivateSite {
    pub base_url: String,
    passkey: String,
    /// The site's own minimums for a download to count as properly seeded.
    #[serde(default = "PrivateSite::usual_hours")]
    pub min_seed_hours: f64,
    #[serde(default = "PrivateSite::usual_ratio")]
    pub min_ratio: f64,
}
/// Transmission label on torrents added from the member's private site.
pub const PRIVATE_LABEL: &str = "private-site";
impl PrivateSite {
    fn usual_hours() -> f64 { 72.0 }
    fn usual_ratio() -> f64 { 0.8 }
    /// Such sites meter their member feed; HHanClub's rule is one request every five
    /// minutes, and it enforces it. This is the site's limit, not one of Torrbaby's.
    pub const FEED_INTERVAL: Duration = Duration::from_secs(310);
    /// How far past the site's minimums its torrents are seeded.
    pub const MARGIN: f64 = 1.25;
    pub fn with_minimums(mut self, hours: f64, ratio: f64) -> Result<Self> {
        ensure!(hours.is_finite() && (0.0..=8760.0).contains(&hours), "Minimum seeding time must be between 0 and 8760 hours");
        ensure!(ratio.is_finite() && (0.0..=100.0).contains(&ratio), "Minimum ratio must be between 0 and 100");
        self.min_seed_hours = hours; self.min_ratio = ratio;
        Ok(self)
    }
    pub fn passkey(&self) -> &str { &self.passkey }
    /// Seconds and ratio the site's torrents are seeded to: both, each a quarter over the minimum.
    pub fn seeding_targets(&self) -> (f64, f64) { (self.min_seed_hours * 3600.0 * Self::MARGIN, self.min_ratio * Self::MARGIN) }
    pub fn new(base_url: &str, passkey: &str) -> Result<Self> {
        let mut url = reqwest::Url::parse(base_url.trim()).context("Enter the site's address, starting with https://")?;
        ensure!(url.scheme() == "https" && url.host_str().is_some_and(|h| h.contains('.')), "Enter the site's address, starting with https://");
        ensure!(url.username().is_empty() && url.password().is_none() && url.query().is_none() && url.fragment().is_none(), "Enter only the site's address");
        if !url.path().ends_with('/') { let path = format!("{}/", url.path()); url.set_path(&path); }
        let passkey = passkey.trim();
        ensure!((16..=64).contains(&passkey.len()) && passkey.bytes().all(|b| b.is_ascii_alphanumeric()), "That does not look like a passkey");
        Ok(Self { base_url: url.to_string(), passkey: passkey.to_owned(), min_seed_hours: Self::usual_hours(), min_ratio: Self::usual_ratio() })
    }
    pub fn load(path: &std::path::Path) -> Option<Self> {
        let saved: Self = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        Self::new(&saved.base_url, &saved.passkey).ok()?.with_minimums(saved.min_seed_hours, saved.min_ratio).ok()
    }
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        use std::os::unix::fs::OpenOptionsExt; use std::io::Write;
        let staged = path.with_extension("json.new");
        let mut file = std::fs::OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(&staged)?;
        file.write_all(&serde_json::to_vec(self)?)?; file.sync_all()?;
        Ok(std::fs::rename(staged, path)?)
    }
    fn page(&self, name: &str) -> Result<reqwest::Url> { Ok(reqwest::Url::parse(&self.base_url)?.join(name)?) }
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
    /// The release's best audio track in plain terms: lossless_atmos, dtsx,
    /// lossless, atmos, surround, stereo or mono. `audio` keeps the raw tags.
    #[serde(default)]
    pub audio_kind: Option<String>,
    /// Speaker layout of that track when the name states it: "5.1", "7.1", "2.0"…
    #[serde(default)]
    pub audio_channels: Option<String>,
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

fn release_key(item: &Item) -> String {
    if !item.info_hash.is_empty() { return item.info_hash.to_ascii_lowercase(); }
    // Group identical listings, not just identical display titles. Keep encodes/sizes distinct.
    format!("listing:{}:{}:{:?}:{:?}", item.sources.first().map(|s|s.provider.as_str()).unwrap_or(""),
        item.name.split_whitespace().collect::<Vec<_>>().join(" "), item.size_bytes, item.size_label)
}

pub struct Search {
    lou: tokio::sync::Mutex<(Instant, HashMap<String, (Instant, Vec<Item>)>)>,
    /// When the private site's feed may next be asked, and what it last answered per search.
    private_feed: tokio::sync::Mutex<(Option<Instant>, HashMap<String, (Instant, Vec<Item>)>)>,
    http: reqwest::Client,
    cache_dir: Option<std::path::PathBuf>,
    private_site: Option<std::path::PathBuf>,
    cache: Mutex<HashMap<String, (Instant, SearchResult)>>,
    details: Mutex<HashMap<String, (Instant, Value)>>,
    reads: Mutex<HashMap<String, Arc<tokio::sync::Mutex<Option<(Instant, Vec<u8>)>>>>>,
}
impl Search {
    pub fn new() -> Result<Self> {
        let mut builder = reqwest::Client::builder();
        if let Ok(proxy) = std::env::var("TORRBABY_SEARCH_PROXY") {
            ensure!(!proxy.trim().is_empty(), "TORRBABY_SEARCH_PROXY must not be empty");
            builder = builder.proxy(reqwest::Proxy::all(&proxy).map_err(|_| anyhow::anyhow!("Invalid search proxy configuration"))?);
        }
        Ok(Self {
            cache_dir: None,
            private_site: None,
            lou: tokio::sync::Mutex::new((Instant::now(), HashMap::new())),
            private_feed: tokio::sync::Mutex::new((None, HashMap::new())),
            http: builder
                .user_agent("Torrbaby/0.3 (personal media library)")
                .connect_timeout(Duration::from_secs(8))
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            cache: Mutex::new(HashMap::new()),
            details: Mutex::new(HashMap::new()),
            reads: Mutex::new(HashMap::new()),
        })
    }
    pub fn with_cache_dir(mut self, dir: std::path::PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&dir)?;
        self.cache_dir=Some(dir);
        self.trim_disk_cache();
        Ok(self)
    }
    /// Where the member's private site and passkey are kept.
    pub fn with_private_site(mut self, path: std::path::PathBuf) -> Self { self.private_site = Some(path); self }
    pub fn private_site_path(&self) -> Option<&std::path::Path> { self.private_site.as_deref() }
    pub fn private_site(&self) -> Option<PrivateSite> { PrivateSite::load(self.private_site.as_deref()?) }
    /// Forget cached results after the site or passkey changes.
    pub fn private_site_changed(&self) {
        if let Ok(mut cache) = self.cache.lock() { cache.clear(); }
        if let Ok(mut feed) = self.private_feed.try_lock() { feed.1.clear(); }
    }
    /// One request to the private site. Errors never carry the address: it holds the passkey.
    async fn read_private(&self, url: reqwest::Url) -> Result<Vec<u8>> {
        let failed = |e: reqwest::Error| {
            let kind = if e.is_timeout() { "timed out" } else if e.is_connect() { "could not be reached" } else { "did not answer as expected" };
            anyhow::anyhow!("The private site {kind}")
        };
        let response = self.http.get(url).send().await.map_err(failed)?;
        ensure!(response.status().is_success(), "The private site refused the request ({}). Check the passkey in Settings.", response.status().as_u16());
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(failed)? {
            ensure!(bytes.len() + chunk.len() <= 4_194_304, "The private site's answer was too large");
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    async fn search_private(&self, terms: &str) -> Result<Vec<Item>> {
        let site = self.private_site().context("Add your private site and passkey in Settings")?;
        // Answers are reused for half an hour, and requests are spaced as the site requires.
        let mut feed = self.private_feed.lock().await;
        feed.1.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(1800));
        if let Some((_, items)) = feed.1.get(terms) { return Ok(items.clone()); }
        if let Some(wait) = feed.0.map(|next| next.saturating_duration_since(Instant::now())).filter(|wait| !wait.is_zero()) {
            anyhow::bail!("The private site allows one search every 5 minutes. Try again in {} minutes.", wait.as_secs() / 60 + 1);
        }
        feed.0 = Some(Instant::now() + PrivateSite::FEED_INTERVAL);
        let items = self.fetch_private(&site, terms).await?;
        if feed.1.len() >= 64 { feed.1.clear(); }
        feed.1.insert(terms.to_owned(), (Instant::now(), items.clone()));
        Ok(items)
    }
    async fn fetch_private(&self, site: &PrivateSite, terms: &str) -> Result<Vec<Item>> {
        // NexusPHP's own search, through the member's feed: one request, no page scraping.
        let mut url = site.page("torrentrss.php")?;
        url.query_pairs_mut().append_pair("passkey", &site.passkey).append_pair("rows", "50").append_pair("linktype", "dl")
            .append_pair("isize", "1").append_pair("search", terms).append_pair("search_mode", "0");
        parse_private_feed(&String::from_utf8(self.read_private(url).await?).context("The private site's answer could not be read")?, site)
    }
    /// The .torrent for one of the site's listings, fetched with the passkey at Add time.
    pub async fn private_metainfo(&self, id: &str) -> Result<Vec<u8>> {
        ensure!(!id.is_empty() && id.len() <= 12 && id.bytes().all(|b| b.is_ascii_digit()), "Invalid listing ID");
        let site = self.private_site().context("Add your private site and passkey in Settings")?;
        let mut url = site.page("download.php")?;
        url.query_pairs_mut().append_pair("id", id).append_pair("passkey", &site.passkey);
        let bytes = self.read_private(url).await?;
        // A login page or an error message instead of a torrent must not reach Transmission.
        chinese::torrent(&bytes, id, "").map_err(|_| anyhow::anyhow!("The private site did not return a torrent. Check the passkey in Settings."))?;
        Ok(bytes)
    }
    fn trim_disk_cache(&self) {
        let Some(dir)=&self.cache_dir else {return};
        let mut entries=std::fs::read_dir(dir).into_iter().flatten().filter_map(Result::ok)
            .filter_map(|e| { let m=e.metadata().ok()?; if !m.is_file(){return None}; Some((m.modified().ok()?,m.len(),e.path())) }).collect::<Vec<_>>();
        entries.sort_by_key(|e|e.0);
        let mut total:u64=entries.iter().map(|e|e.1).sum();
        for (modified,len,path) in entries {
            if total<=64*1024*1024 && modified.elapsed().unwrap_or_default()<Duration::from_secs(86400) {continue;}
            if std::fs::remove_file(path).is_ok(){total=total.saturating_sub(len);}
        }
    }
    async fn read(&self, url: reqwest::Url) -> Result<String> {
        Ok(String::from_utf8(self.read_bytes(url).await?)?)
    }
    async fn read_bytes(&self, url: reqwest::Url) -> Result<Vec<u8>> {
        // Row sizes, details and Add share one fetch for each public attachment/page.
        if url.host_str() == Some("www.1lou.me") {
            let slot = {
                let mut reads=self.reads.lock().map_err(|_|anyhow::anyhow!("Source cache unavailable"))?;
                if reads.len()>=256 { reads.retain(|_,slot| Arc::strong_count(slot)>1); }
                reads.entry(url.to_string()).or_insert_with(||Arc::new(tokio::sync::Mutex::new(None))).clone()
            };
            let mut cached=slot.lock().await;
            let ttl=Duration::from_secs(if url.path().contains("/search/"){600}else{86400});
            if let Some((at,bytes))=cached.as_ref() { if at.elapsed()<ttl {return Ok(bytes.clone());} }
            use sha2::{Digest,Sha256};
            let disk=self.cache_dir.as_ref().map(|dir|dir.join(format!("{}.cache",Sha256::digest(url.as_str().as_bytes()).iter().map(|b|format!("{b:02x}")).collect::<String>())));
            if let Some(path)=&disk {
                if let Ok(meta)=std::fs::metadata(path) {
                    let age=meta.modified().ok().and_then(|t|t.elapsed().ok());
                    if meta.len()<=4_194_304 && age.is_some_and(|a|a<ttl) {
                        if let Ok(bytes)=std::fs::read(path) {
                            *cached=Some((Instant::now()-age.unwrap_or_default(),bytes.clone()));
                            return Ok(bytes);
                        }
                    }
                }
            }
            let bytes=self.fetch_bytes(url).await?;
            if let Some(path)=&disk {
                let tmp=path.with_extension("tmp");
                if std::fs::write(&tmp,&bytes).is_ok() {let _=std::fs::rename(tmp,path);}
                self.trim_disk_cache();
            }
            *cached=Some((Instant::now(),bytes.clone()));
            return Ok(bytes);
        }
        self.fetch_bytes(url).await
    }
    async fn fetch_bytes(&self, url: reqwest::Url) -> Result<Vec<u8>> {
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
            let delay=response.headers().get(reqwest::header::RETRY_AFTER).and_then(|v|v.to_str().ok()).and_then(|s|s.parse::<u64>().ok()).unwrap_or(0).min(3600);
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
        if !wait.is_zero() { anyhow::bail!("Source requested a retry delay; try again in {} seconds", wait.as_secs()+1); }
        let mut url=reqwest::Url::parse("http://www.1lou.me/search/api/search.php")?;
        url.query_pairs_mut().append_pair("q",terms).append_pair("sort","newest").append_pair("track","0");
        let result=match self.read(url).await { Ok(raw)=>chinese::listings(&raw),Err(e)=>Err(e) };
        let delay=result.as_ref().err().and_then(|e|e.downcast_ref::<RateLimited>()).map(|e|e.0).unwrap_or(0);
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
            // A member's private site is searched by default alongside the public index.
            if matches!(query.market, Market::China) && self.private_site().is_some() { query.sources.push("pt".into()); }
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
        // Keep title searches broad: requiring S02 would hide "Season 2" and
        // S01-S04 packs. Rank the requested season/episode locally instead.
        let terms = query.query.clone();
        let responses = futures_util::future::join_all(query.sources.iter().map(|provider| {
            let terms = &terms;
            async move {
                let result = if provider == "tpb" {
                    let mut url = reqwest::Url::parse("https://apibay.org/q.php").expect("valid URL");
                    url.query_pairs_mut()
                        .append_pair("q", &terms)
                        .append_pair("cat", "0");
                    match self.read(url).await {
                        Ok(raw) => parse_tpb(&raw),
                        Err(e) => Err(e),
                    }
                } else if provider == "csv" {
                    let mut url =
                        reqwest::Url::parse("https://torrents-csv.com/service/search").expect("valid URL");
                    url.query_pairs_mut()
                        .append_pair("q", terms)
                        .append_pair("size", "100");
                    match self.read(url).await {
                        Ok(raw) => parse_csv(&raw),
                        Err(e) => Err(e),
                    }
                } else if provider == "1lou" {
                    self.search_lou(terms).await
                } else if provider == "pt" {
                    self.search_private(terms).await
                } else if provider == "dmhy" || provider == "mikan" {
                    let (base, key) = if provider == "dmhy" {
                        ("https://share.dmhy.org/topics/rss/rss.xml", "keyword")
                    } else { ("https://mikanani.me/RSS/Search", "searchstr") };
                    let mut url = reqwest::Url::parse(base).expect("valid URL");
                    url.query_pairs_mut().append_pair(key, terms);
                    match self.read(url).await {
                        Ok(raw) => parse_chinese_feed(&raw, provider),
                        Err(e) => Err(e),
                    }
                } else {
                    let mut url = reqwest::Url::parse("https://nyaa.si/").expect("valid URL");
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
                    // Some indexes answer a query they cannot search (Chinese text, for
                    // one) with their list of popular torrents. A result sharing no word
                    // with the query is not a result. The Chinese forum is searched in its
                    // own language by its own engine, so its matches are kept as given.
                    let items: Vec<Item> = if provider == "1lou" || provider == "pt" { items } else {
                        items.into_iter().filter(|item| relevant(&item.name, &query.query)).collect()
                    };
                    providers.push(ProviderStatus {
                        provider,
                        count: items.len(),
                        error: None,
                    });
                    for mut item in items {
                        score(&mut item, &query);
                        let key = release_key(&item);
                        if let Some(existing) = unique.get_mut(&key) {
                            if item.match_score > existing.match_score
                                || (item.match_score == existing.match_score
                                    && item.name.len() > existing.name.len())
                            {
                                std::mem::swap(existing, &mut item);
                            }
                            existing.sources.extend(item.sources);
                            let mut seen=HashSet::new();
                            existing.sources.retain(|s|seen.insert((s.provider.clone(),s.id.clone())));
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
                        "The search source refused this request (rate limit). Please try again later."
                    } else if error.to_string().starts_with("Source requested a retry delay") {
                        "Waiting for the search source’s requested retry time."
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
    pub async fn resolve(&self, provider: &str, id: &str, attachment: Option<&str>) -> Result<Value> {
        ensure!(provider=="1lou", "This source requires a magnet link");
        ensure!(!id.is_empty() && id.len()<=12 && id.bytes().all(|b|b.is_ascii_digit()), "Invalid listing ID");
        let url=format!("http://www.1lou.me/thread-{id}.htm");
        let raw=self.read(reqwest::Url::parse(&url)?).await?;
        let choices=chinese::attachment_choices(&raw)?;
        ensure!(!choices.is_empty(), "This listing has no downloadable torrent");
        let selected=match attachment {
            Some(selected)=>choices.iter().find(|(id,_)|id==selected).context("Release no longer available")?,
            None if choices.len()==1=>&choices[0],
            None=>return Ok(json!({"choices":choices.iter().map(|(id,name)|json!({"id":id,"name":name})).collect::<Vec<_>>()})),
        };
        let bytes=self.read_bytes(reqwest::Url::parse(&format!("http://www.1lou.me/attach-download-{}.htm",selected.0))?).await?;
        let release=chinese::torrent(&bytes,id,&url)?;
        use base64::Engine;
        Ok(json!({"item":release["item"],"metainfo":base64::engine::general_purpose::STANDARD.encode(bytes)}))
    }
    pub async fn summary(&self, provider: &str, id: &str) -> Result<Value> {
        ensure!(provider == "1lou", "Unsupported listing source");
        ensure!(!id.is_empty() && id.len() <= 12 && id.bytes().all(|b| b.is_ascii_digit()), "Invalid listing ID");
        let raw=self.read(reqwest::Url::parse(&format!("http://www.1lou.me/thread-{id}.htm"))?).await?;
        let choices=chinese::attachment_choices(&raw)?.into_iter().map(|(id,name)| {
            json!({"url":format!("http://www.1lou.me/attach-download-{id}.htm"),"id":id,"name":name})
        }).collect::<Vec<_>>();
        Ok(json!({"size_label":chinese::summary_size(&raw)?,"choices":choices}))
    }
    pub async fn detail(&self, provider: &str, id: &str) -> Result<Value> {
        if provider == "1lou" {
            ensure!(!id.is_empty() && id.len() <= 12 && id.bytes().all(|b| b.is_ascii_digit()), "Invalid listing ID");
            if let Some((at, value)) = self.details.lock().map_err(|_| anyhow::anyhow!("Search cache unavailable"))?.get(id) {
                if at.elapsed() < Duration::from_secs(600) { return Ok(value.clone()); }
            }
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
            })).buffered(2).collect::<Vec<Result<Value>>>().await.into_iter().filter_map(Result::ok).collect::<Vec<_>>();
            ensure!(!releases.is_empty(), "Could not load torrent files; try Add torrent to retry the selected release");
            let mut seen=HashSet::new();
            let releases=releases.into_iter().filter(|r|seen.insert(r["item"]["info_hash"].as_str().unwrap_or_default().to_owned())).collect::<Vec<_>>();
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
    let mut encoded_name = reqwest::Url::parse("magnet:?").expect("valid URL");
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
#[derive(Deserialize)]
struct PrivateFeed { channel: PrivateChannel }
#[derive(Deserialize)]
struct PrivateChannel { #[serde(default, rename="item")] items: Vec<PrivateRow> }
#[derive(Deserialize)]
struct PrivateRow {
    title: String,
    #[serde(default)] link: Option<String>,
    #[serde(default)] enclosure: Option<Enclosure>,
    #[serde(default)] guid: Option<PrivateGuid>,
    #[serde(default, rename="pubDate")] added: Option<String>,
}
#[derive(Deserialize)]
struct PrivateGuid { #[serde(default, rename="$text")] value: String }
/// A NexusPHP member feed. Each row is a listing on the member's own site; the download
/// address in the feed (which carries the passkey) is read only for its listing number.
fn parse_private_feed(raw: &str, site: &PrivateSite) -> Result<Vec<Item>> {
    static SIZE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s*\[\s*([0-9]+(?:\.[0-9]+)?)\s*(TiB|GiB|MiB|KiB|TB|GB|MB|KB)\s*\]\s*$").expect("valid regex"));
    let feed: PrivateFeed = quick_xml::de::from_str(raw).context("The private site did not return a search feed. Check the passkey in Settings.")?;
    let host = reqwest::Url::parse(&site.base_url)?.host_str().map(str::to_owned);
    let listing = |address: &str| -> Option<String> {
        let url = reqwest::Url::parse(address).ok()?;
        if url.host_str().map(str::to_owned) != host { return None; }
        let id = url.query_pairs().find(|(k, _)| k == "id")?.1.into_owned();
        (!id.is_empty() && id.len() <= 12 && id.bytes().all(|b| b.is_ascii_digit())).then_some(id)
    };
    Ok(feed.channel.items.into_iter().take(100).filter_map(|row| {
        let id = row.link.as_deref().and_then(listing).or_else(|| row.enclosure.as_ref().and_then(|e| listing(&e.url)))?;
        let mut name = row.title.trim().to_owned();
        let size_label = SIZE.captures(&name).map(|c| format!("{} {}", &c[1], c[2].to_ascii_uppercase().replace("IB", "iB")));
        if let Some(found) = SIZE.find(&name) { name.truncate(found.start()); }
        if name.is_empty() || name.len() > 4096 { return None; }
        let info_hash = row.guid.as_ref().and_then(|g| hash(g.value.trim())).unwrap_or_default();
        let details = site.page("details.php").ok().map(|mut url| { url.query_pairs_mut().append_pair("id", &id); url.to_string() })?;
        let mut item = item(name.clone(), info_hash.clone(), Source { provider: "pt".into(), id: id.clone(), url: details });
        // A private torrent cannot be added from its hash alone. The marker tells this
        // server to fetch the listing's .torrent, with the passkey, when it is added.
        let mut link = reqwest::Url::parse("magnet:?").expect("valid URL");
        link.query_pairs_mut().append_pair("dn", &name).append_pair("x.pt", &id);
        item.magnet = if info_hash.is_empty() { link.to_string() } else { format!("magnet:?xt=urn:btih:{info_hash}&{}", link.query().unwrap_or("")) };
        item.size_bytes = row.enclosure.as_ref().and_then(|e| e.length).filter(|n| *n > 0);
        item.size_label = size_label;
        item.added = row.added;
        Some(item)
    }).collect())
}
/// The listing number carried by a private site's result, if this link is one.
pub fn private_listing(magnet: &str) -> Option<String> {
    let url = reqwest::Url::parse(magnet).ok()?;
    if url.scheme() != "magnet" { return None; }
    url.query_pairs().find(|(k, _)| k == "x.pt").map(|(_, v)| v.into_owned())
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
/// Reduce the many audio tags of release names to one plain description of the
/// best track. Tags are only read after the year or resolution, where release
/// details start, so words in the title ("Opus", "DD") are not mistaken for them.
fn audio_summary(name: &str) -> (Option<String>, Option<String>) {
    static TAGS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(
            r"(?i)(dts[ ._:-]?x|dts[ ._-]?hd[ ._-]?(?:ma|master[ ._-]?audio)|dts[ ._-]?hd[ ._-]?(?:hra?|high[ ._-]?res(?:olution)?)|dts[ ._-]?hd|dts[ ._-]?es|dts[ ._-]?ma|dts",
            r"|true[ ._-]?hd|ddpa|ddp|dd\+|e[ ._-]?ac[ ._-]?3|dolby[ ._-]?digital[ ._-]?plus|dolby[ ._-]?digital|dd|ac[ ._-]?3",
            r"|he[ ._-]?aac|aac(?:[ ._-]?lc)?|flac|l?pcm|opus|mp3|atmos)",
            r"(?:[ ._-]?([125678])[ .]([01]))?"
        )).expect("valid regex")
    });
    static START: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(?:(?:19|20)\d{2}|2160p|1080[pi]|720p|480p|4k|S\d{1,2}(?:E\d+)?)\b").expect("valid regex")
    });
    static LAYOUT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|[^0-9A-Za-z.])([57])\.(1)(?:[^0-9]|$)").expect("valid regex"));
    let from = START.find(name).map_or(0, |m| m.start());
    let tail = &name[from..];
    // (rank, kind, layout): the highest rank describes the release.
    let mut best: Option<(u8, &str, Option<String>)> = None;
    let mut atmos = name.contains("全景声");
    let mut tracks: Vec<(String, Option<String>)> = Vec::new();
    for found in TAGS.captures_iter(tail) {
        let whole = found.get(1).expect("tag group");
        // Whole tags only: not letters inside a longer word.
        if tail[..whole.start()].chars().next_back().is_some_and(|c| c.is_alphanumeric()) { continue; }
        let end = found.get(0).expect("match").end();
        if found.get(2).is_none() && tail[whole.end()..].chars().next().is_some_and(|c| c.is_alphabetic()) { continue; }
        if tail[end..].chars().next().is_some_and(|c| c.is_ascii_digit()) && found.get(2).is_some() { continue; }
        let tag: String = whole.as_str().to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '+').collect();
        let layout = found.get(2).map(|a| format!("{}.{}", a.as_str(), &found[3]));
        if tag == "atmos" || tag == "ddpa" { atmos = true; }
        if tag != "atmos" { tracks.push((tag, layout)); }
    }
    let stated = LAYOUT.captures(tail).map(|c| format!("{}.{}", &c[1], &c[2]));
    for (tag, layout) in tracks {
        let lossless = matches!(tag.as_str(), "dtshdma" | "dtshdmasteraudio" | "dtsma" | "truehd" | "flac" | "pcm" | "lpcm");
        let dolby = matches!(tag.as_str(), "ddpa" | "ddp" | "dd+" | "eac3" | "dolbydigitalplus" | "dolbydigital" | "dd" | "ac3" | "truehd");
        let small = matches!(tag.as_str(), "aac" | "aaclc" | "heaac" | "mp3" | "opus" | "flac" | "pcm" | "lpcm");
        // Disc and broadcast formats are surround unless the name says otherwise; AAC, MP3 and the like are stereo.
        let layout = layout.or_else(|| if small { None } else { stated.clone() });
        let wide = match layout.as_deref() { Some(l) => !matches!(l, "1.0" | "2.0" | "2.1"), None => !small };
        let candidate = if tag == "dtsx" { (6, "dtsx", layout) }
            else if atmos && tag == "truehd" { (7, "lossless_atmos", layout) }
            else if lossless { (5, "lossless", layout) }
            else if atmos && dolby { (4, "atmos", layout) }
            else if wide { (3, "surround", layout) }
            else if layout.as_deref() == Some("1.0") { (1, "mono", None) }
            else { (2, "stereo", None) };
        if best.as_ref().is_none_or(|b| candidate.0 > b.0) { best = Some(candidate); }
    }
    // "Atmos" on its own, with no codec named, is the streaming form.
    if best.is_none() && atmos { best = Some((4, "atmos", stated)); }
    match best { Some((_, kind, layout)) => (Some(kind.to_owned()), layout), None => (None, None) }
}

fn hints(name: &str) -> Hints {
    static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b((?:19|20)\d{2})\b").expect("valid regex"));
    static EPISODE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\bS(\d{1,2})(?:[ ._-]*E(\d{1,3}))?\b").expect("valid regex"));
    static SEASON_WORD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\bSeasons?[ ._-]+(\d{1,2})(?:[ ._-]*(?:to|-)[ ._-]*(\d{1,2}))?\b").expect("valid regex")
    });
    static SEASON_RANGE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\bS(\d{1,2})[ ._-]*(?:-|to)[ ._-]*S?(\d{1,2})\b").expect("valid regex")
    });
    static EPISODE_RANGE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\bS\d{1,2}[ ._-]*E(\d{1,3})(?:[ ._-]*-[ ._-]*E?|E)(\d{1,3})\b").expect("valid regex")
    });
    static RES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\b(2160p|1080p|1080i|720p|480p|4k)\b").expect("valid regex"));
    static CODEC: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(x265|h[ .]?265|hevc|x264|h[ .]?264|av1|xvid)\b").expect("valid regex")
    });
    static SOURCE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(WEB[ ._-]?DL|WEBRip|BluRay|BDRip|Remux|HDTV|DVD(?:Rip)?)\b").expect("valid regex")
    });
    static AUDIO: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\b(AAC(?:[ .]?[257]\.[01])?|E[ ._-]?AC[ ._-]?3|AC[ ._-]?3|DTS(?:[ ._-]HD)?(?:[ ._-]MA)?|TrueHD|Atmos|FLAC|DDP?(?:[ .]?[257]\.1)?)\b",
        )
        .expect("valid regex")
    });
    static LANG: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\b(ENG|CHS|CHT|JPN|MULTI|DUAL[ ._-]AUDIO)\b").expect("valid regex"));
    static COMPLETE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"全(\d{1,3})集").expect("valid regex"));
    static CHINESE_EPISODES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"第(\d{1,3})(?:[-—至](\d{1,3}))?集").expect("valid regex"));
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
        resolution: RES.find(name).map(|m| { let value=m.as_str().to_ascii_lowercase(); if value=="4k" {"2160p".to_owned()} else {value} }),
        video_codec: CODEC.find(name).map(|m| m.as_str().to_ascii_uppercase()),
        release_source: SOURCE.find(name).map(|m| m.as_str().to_owned()),
        audio: AUDIO
            .find_iter(name)
            .map(|m| m.as_str().to_ascii_uppercase())
            .collect(),
        audio_kind: audio_summary(name).0,
        audio_channels: audio_summary(name).1,
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
/// Whether a release name shares at least one word with the query. Chinese and
/// other unspaced text is matched as a run inside the name.
fn relevant(name: &str, query: &str) -> bool {
    let (wanted, found) = (words(query), words(name));
    let lowered = name.to_lowercase();
    wanted.is_empty() || wanted.iter().any(|word| found.contains(word) || (!word.is_ascii() && lowered.contains(word.as_str())))
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
            .expect("valid regex")
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
    #[tokio::test]
    async fn persistent_bt_cache_avoids_network_after_restart() {
        use sha2::{Digest,Sha256};
        let dir=std::env::temp_dir().join(format!("torrbaby-search-cache-test-{}",uuid::Uuid::new_v4()));
        let url=reqwest::Url::parse("http://www.1lou.me/thread-123.htm").unwrap();
        let name=format!("{}.cache",Sha256::digest(url.as_str().as_bytes()).iter().map(|b|format!("{b:02x}")).collect::<String>());
        let first=Search::new().unwrap().with_cache_dir(dir.clone()).unwrap();
        std::fs::write(dir.join(name),b"saved fixture page").unwrap();
        drop(first);
        let mut restarted=Search::new().unwrap().with_cache_dir(dir.clone()).unwrap();
        // Even a regression must never contact a real indexer during this test.
        restarted.http=reqwest::Client::builder().proxy(reqwest::Proxy::all("http://127.0.0.1:9").unwrap()).build().unwrap();
        assert_eq!(restarted.read_bytes(url).await.unwrap(),b"saved fixture page");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn private_site_results_never_carry_the_passkey() {
        let site = PrivateSite::new("https://pt.example.org", "0123456789abcdef0123456789abcdef").unwrap();
        assert_eq!(site.base_url, "https://pt.example.org/");
        assert_eq!(site.seeding_targets(), (72.0 * 3600.0 * 1.25, 1.0), "a quarter past 72 hours and past a ratio of 0.8");
        assert_eq!(site.clone().with_minimums(48.0, 1.0).unwrap().seeding_targets(), (48.0 * 3600.0 * 1.25, 1.25));
        assert!(site.clone().with_minimums(-1.0, 1.0).is_err());
        assert!(PrivateSite::new("http://pt.example.org", "0123456789abcdef0123456789abcdef").is_err());
        assert!(PrivateSite::new("https://pt.example.org/?x=1", "0123456789abcdef0123456789abcdef").is_err());
        assert!(PrivateSite::new("https://pt.example.org", "short").is_err());
        let raw = r#"<?xml version="1.0" encoding="utf-8"?><rss version="2.0"><channel><title>PT</title>
            <item><title><![CDATA[流浪地球2 The Wandering Earth II 2023 2160p WEB-DL H265 DDP5.1[18.52 GB]]]></title>
              <link>https://pt.example.org/details.php?id=12345</link>
              <enclosure url="https://pt.example.org/download.php?id=12345&amp;passkey=0123456789abcdef0123456789abcdef" length="19885482393" type="application/x-bittorrent" />
              <guid isPermaLink="false">0123456789abcdef0123456789abcdef01234567</guid>
              <pubDate>Thu, 12 Nov 2026 02:00:00 +0000</pubDate></item>
            <item><title>Elsewhere</title><link>https://other.example.org/details.php?id=9</link>
              <enclosure url="https://other.example.org/download.php?id=9" length="1" type="application/x-bittorrent" /></item>
            </channel></rss>"#;
        let rows = parse_private_feed(raw, &site).unwrap();
        assert_eq!(rows.len(), 1, "listings from another host are dropped");
        let row = &rows[0];
        assert_eq!(row.name, "流浪地球2 The Wandering Earth II 2023 2160p WEB-DL H265 DDP5.1");
        assert_eq!((row.size_bytes, row.size_label.as_deref()), (Some(19885482393), Some("18.52 GB")));
        assert_eq!(row.info_hash, "0123456789abcdef0123456789abcdef01234567");
        assert_eq!((row.sources[0].provider.as_str(), row.sources[0].id.as_str()), ("pt", "12345"));
        assert_eq!(private_listing(&row.magnet).as_deref(), Some("12345"));
        assert!(row.magnet.starts_with("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&"));
        assert!(!serde_json::to_string(row).unwrap().contains("0123456789abcdef0123456789abcdef\""), "no passkey in a result");
        assert!(!serde_json::to_string(row).unwrap().contains("passkey"));
        assert_eq!(private_listing("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=x"), None);
        assert!(Market::China.allowed().contains(&"pt") && !Market::World.allowed().contains(&"pt"));
    }
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
        assert!(relevant("[ 高清剧集网 ]知否知否应是绿肥红瘦[第01-73集].The.Story.of.Ming.Lan.2018", "知否知否应是绿肥红瘦"));
        assert!(relevant("某站 知否知否应是绿肥红瘦全集 1080p", "知否"));
        assert!(!relevant("The Odyssey (2026) [1080p] [BluRay] [5.1]", "知否知否应是绿肥红瘦"));
        assert!(relevant("Example.Show.S02E03.1080p", "example show") && !relevant("Other.Thing.2020", "example show"));
        let audio = |name: &str| { let h = hints(name); (h.audio_kind.unwrap_or_default(), h.audio_channels.unwrap_or_default()) };
        let expect = |name: &str, kind: &str, layout: &str| assert_eq!(audio(name), (kind.to_owned(), layout.to_owned()), "{name}");
        expect("Film.2023.2160p.NF.WEB-DL.DDP5.1.Atmos.DV.H.265-GRP", "atmos", "5.1");
        expect("Film.2023.1080p.ATVP.WEB-DL.DDPA5.1.H.264-GRP", "atmos", "5.1");
        expect("Film.2019.2160p.UHD.BluRay.Remux.HEVC.TrueHD.7.1.Atmos-GRP", "lossless_atmos", "7.1");
        expect("Film 2019 2160p BluRay DTS-X 7.1 x265", "dtsx", "7.1");
        expect("8 Mile [2002] COMPLETE 1080p BDRip x265 DTS-HD MA 5.1 Kira [SEV]", "lossless", "5.1");
        expect("Film.2002.1080p.BluRay.DTS-HD.MA.5.1.x264", "lossless", "5.1");
        expect("Film.2010.1080p.BluRay.FLAC.2.0.x264", "lossless", "2.0");
        expect("Example.Show.S02E03.1080p.WEB-DL.DDP5.1.x265", "surround", "5.1");
        expect("Film.2015.1080p.BluRay.x264.DTS-GRP", "surround", "");
        expect("Film.2015.1080p.BluRay.DD+ 7.1.x264", "surround", "7.1");
        expect("Film.2015.720p.HDTV.AC3.2.0.x264", "stereo", "");
        expect("Film.2015.1080p.WEB-DL.AAC2.0.H.264-GRP", "stereo", "");
        expect("Film.2015.1080p.WEB-DL.AAC5.1.H.264-GRP", "surround", "5.1");
        expect("Film.2015.1080p.WEBRip.x265.AAC-GRP", "stereo", "");
        expect("Film.1950.480p.DVDRip.AAC.1.0", "mono", "");
        expect("逐玉[全40集][国语音轨/简繁英字幕].Pursuit.of.Jade.S01.2026.1080p.IQ.WEB-DL.H264.AAC-BlackTV", "stereo", "");
        expect("某电影.2024.2160p.WEB-DL.杜比全景声.H265", "atmos", "");
        // The best track names the release; title words are not audio tags.
        expect("Film.2020.1080p.BluRay.TrueHD.5.1.AC3.5.1", "lossless", "5.1");
        expect("Mr.Hollands.Opus.1995.1080p.BluRay.x264", "", "");
        expect("Daddy.DD.Day.Care.1080p", "", "");
        expect("Film.2020.1080p.H.264", "", "");
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

#[cfg(test)]
mod listing_dedup_tests {
    use super::*;
    #[test]
    fn identical_posts_group_but_different_encodes_do_not() {
        let a=item("逐玉 2160p HDR".into(),String::new(),Source{provider:"1lou".into(),id:"1".into(),url:String::new()});
        let mut b=a.clone();b.sources[0].id="2".into();assert_eq!(release_key(&a),release_key(&b));
        b.name="逐玉 2160p Dolby Vision".into();assert_ne!(release_key(&a),release_key(&b));
        b=a.clone();b.size_bytes=Some(123);assert_ne!(release_key(&a),release_key(&b));
    }
}
