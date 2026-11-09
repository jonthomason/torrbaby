use anyhow::{Context, Result, bail};
use regex::Regex;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
    time::Duration,
};
use tokio::sync::Mutex;

static YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|\D)((?:19|20)\d{2})(?:\D|$)").unwrap());
static SEASON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:s\d{1,2}(?:[ ._-]*e\d{1,3})?|season[ ._-]*\d{1,2}|\d{1,2}x\d{2})(?:[^a-z0-9]|$)").unwrap()
});
pub static SERIES_PACK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bcomplete[ ._-]+(?:tv[ ._-]+)?series\b").unwrap()
});
static TECHNICAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:complete|collection|trilogy|1080p|2160p|720p|bluray|webrip|web-dl|hdtv)\b",
    )
    .unwrap()
});
static PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^www\.[^ ]+\s*[- ]*").unwrap());

pub fn parse_name(name: &str) -> (String, String, &'static str) {
    let name = PREFIX.replace(name, "");
    let season = SEASON.find(&name);
    let year = YEAR.captures(&name);
    let cutoff = [
        season.as_ref().map(|m| m.start()),
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
        title[..cutoff]
            .trim_matches(|c: char| c.is_whitespace() || "-()[]".contains(c))
            .to_owned(),
        year.and_then(|c| c.get(1).map(|m| m.as_str().to_owned()))
            .unwrap_or_default(),
        if season.is_some() || SERIES_PACK.is_match(&name) { "tv" } else { "movie" },
    )
}
fn normalized(value: &str) -> String {
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

#[derive(Default)]
struct SearchArtworkCache {
    size: Option<String>,
    entries: std::collections::HashMap<String, (std::time::Instant, Value)>,
}

pub struct Metadata {
    client: reqwest::Client,
    token: PathBuf,
    host: Mutex<String>,
    cache: Mutex<std::collections::HashMap<String, Value>>,
    search_artwork: Mutex<SearchArtworkCache>,
}
impl Metadata {
    pub fn new(directory: &Path) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            token: directory.join("tmdb.token"),
            host: Mutex::new("api.tmdb.org".to_owned()),
            cache: Mutex::new(std::collections::HashMap::new()),
            search_artwork: Mutex::new(SearchArtworkCache::default()),
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
        let token = tokio::fs::read_to_string(&self.token)
            .await
            .context("Configure a TMDB token on Diamond")?;
        self.request_with_token(path, query, token.trim()).await
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
            let response = match request.send().await {
                Ok(response) => response,
                Err(_) => continue,
            };
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
    pub async fn search_artwork(&self, name: &str) -> Result<Value> {
        use base64::Engine;
        if !self.configured() { return Ok(json!({"data":null})); }
        anyhow::ensure!(name.len() <= 4096, "Release name too long");
        let (query, year, kind) = parse_name(name);
        // Movie titles without a release year and collections are too ambiguous.
        if query.is_empty() || (kind == "movie" && year.is_empty())
            || Regex::new(r"(?i)\b(collection|trilogy|anthology|\d+[ -]+\d+[ ._-]+pack)\b").unwrap().is_match(name) {
            return Ok(json!({"data":null}));
        }
        let key = format!("{kind}:{}:{year}", normalized(&query));
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
            if let Some(path) = exact[0]["poster_path"].as_str().filter(|p|
                p.starts_with('/') && p.len() < 256 && !p.contains("..")
                    && p.chars().all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))) {
                if cache.size.is_none() {
                    let config = self.request("configuration", &[]).await?;
                    cache.size = array(&config["images"]["poster_sizes"]).iter()
                        .filter_map(Value::as_str)
                        .filter_map(|size| Some((size.strip_prefix('w')?.parse::<u32>().ok()?, size)))
                        .min_by_key(|(width, _)| *width).map(|(_, size)| size.to_owned());
                }
                if let Some(size) = &cache.size {
                    let mut response = self.client.get(format!("https://image.tmdb.org/t/p/{size}{path}"))
                        .send().await?.error_for_status()?;
                    let mut bytes = Vec::new();
                    while let Some(chunk) = response.chunk().await? {
                        anyhow::ensure!(bytes.len() + chunk.len() <= 512 * 1024, "Thumbnail too large");
                        bytes.extend_from_slice(&chunk);
                    }
                    result = json!({"data":base64::engine::general_purpose::STANDARD.encode(bytes),
                        "provider":"TMDB","provider_id":exact[0]["id"],"size":size,
                        "confidence":if year.is_empty() {"exact_unique_title"} else {"exact_title_year"}});
                }
            }
        }
        if cache.entries.len() >= 128 { cache.entries.clear(); }
        cache.entries.insert(key, (std::time::Instant::now(), result.clone()));
        Ok(result)
    }
    pub async fn search(&self, name: &str) -> Result<Value> {
        let (query, year, kind) = parse_name(name);
        if query.is_empty() {
            bail!("Could not infer a title from the filename");
        }
        let data = self
            .request(
                &format!("search/{kind}"),
                &[
                    ("query", &query),
                    ("language", "en-US"),
                    ("include_adult", "false"),
                ],
            )
            .await?;
        let candidates:Vec<Value>=array(&data["results"]).iter().take(8).map(|hit|{
            let date=hit["release_date"].as_str().or(hit["first_air_date"].as_str()).unwrap_or("");
            let id=hit["id"].as_u64().unwrap_or(0).to_string();
            json!({"provider":"TMDB","provider_id":id,"kind":kind,"title":hit.get("title").or(hit.get("name")),
                "original_title":hit.get("original_title").or(hit.get("original_name")),"year":date.get(..4).unwrap_or(""),
                "genres":[],"content_rating":null,"duration_minutes":null,"summary":hit["overview"],"thumbnail":image(&hit["poster_path"],"w500"),
                "logo":null,"source_url":format!("https://www.themoviedb.org/{kind}/{id}"),"episodes":[]})
        }).collect();
        let exact: Vec<_> = candidates
            .iter()
            .filter(|c| {
                (normalized(&query) == normalized(c["title"].as_str().unwrap_or(""))
                    || normalized(&query) == normalized(c["original_title"].as_str().unwrap_or("")))
                    && (year.is_empty() || c["year"] == year)
            })
            .collect();
        let selected = if exact.len() == 1 {
            let mut selected = self.enrich(exact[0]).await?;
            selected["match_confidence"] = json!(if year.is_empty() {
                "exact_unique_title"
            } else {
                "exact_title_year"
            });
            selected
        } else {
            Value::Null
        };
        Ok(
            json!({"status":if !selected.is_null(){"matched"}else if candidates.is_empty(){"no_match"}else{"review"},"query":query,"candidates":candidates,"selected":selected}),
        )
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
            if crate::storage::now() - entity["fetched_at"].as_f64().unwrap_or(0.0) < 86400.0 {
                return Ok(entity.clone());
            }
        }
        let detail = self
            .request(
                &format!("{kind}/{id}"),
                &[
                    ("language", "en-US"),
                    ("append_to_response", "images,release_dates,content_ratings"),
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
        entity["summary"] = detail["overview"].clone();
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
