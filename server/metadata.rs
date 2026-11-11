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
static SEASON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:s\d{1,2}(?:[ ._-]*e\d{1,3})?|season[ ._-]*\d{1,2}|\d{1,2}x\d{2})(?:[^a-z0-9]|$)").expect("valid regex")
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
            artwork_dir: directory.join("search-artwork"),
            response_dir: directory.join("tmdb-cache"),
            responses: Mutex::new(std::collections::HashMap::new()),
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
        static PACK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(collection|trilogy|anthology|\d+[ -]+\d+[ ._-]+pack)\b").expect("valid regex"));
        let (query, year, kind) = parse_name(name);
        // Movie titles without a release year and collections are too ambiguous.
        if query.is_empty() || (kind == "movie" && year.is_empty())
            || PACK.is_match(name) {
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
                if let Some(size) = cache.size.clone() {
                    let image_key=format!("{size}{path}");
                    use sha2::{Digest,Sha256};
                    let file=self.artwork_dir.join(Sha256::digest(image_key.as_bytes()).iter().map(|b|format!("{b:02x}")).collect::<String>());
                    let encoded=if let Some(value)=cache.images.get(&image_key) {value.clone()} else {
                        let stored=std::fs::metadata(&file).ok().filter(|m|m.len()<=512*1024)
                            .and_then(|_|std::fs::read(&file).ok());
                        let bytes=if let Some(bytes)=stored {bytes} else {
                            let mut response=self.client.get(format!("https://image.tmdb.org/t/p/{image_key}"))
                                .send().await?.error_for_status()?;
                            let mut bytes=Vec::new();
                            while let Some(chunk)=response.chunk().await? {
                                anyhow::ensure!(bytes.len()+chunk.len()<=512*1024,"Thumbnail too large");
                                bytes.extend_from_slice(&chunk);
                            }
                            if std::fs::create_dir_all(&self.artwork_dir).is_ok() {
                                let tmp=file.with_extension("tmp");
                                if std::fs::write(&tmp,&bytes).is_ok(){let _=std::fs::rename(tmp,&file);}
                                let mut files=std::fs::read_dir(&self.artwork_dir).into_iter().flatten().filter_map(Result::ok)
                                    .filter_map(|e|{let m=e.metadata().ok()?;Some((m.modified().ok()?,m.len(),e.path()))}).collect::<Vec<_>>();
                                files.sort_by_key(|e|e.0);let mut total:u64=files.iter().map(|e|e.1).sum();
                                for (_,len,path) in files {if total<=64*1024*1024 {break;}if std::fs::remove_file(path).is_ok(){total=total.saturating_sub(len);}}
                            }
                            bytes
                        };
                        let encoded=base64::engine::general_purpose::STANDARD.encode(bytes);
                        if cache.images.len()>=128 {cache.images.clear();}
                        cache.images.insert(image_key,encoded.clone());encoded
                    };
                    result = json!({"data":encoded,
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
    pub async fn cast(&self, entity: &Value) -> Result<Value> {
        let kind=entity["kind"].as_str().context("Missing media kind")?;
        anyhow::ensure!(["movie","tv"].contains(&kind), "Invalid media kind");
        let id=entity["provider_id"].as_str().context("Missing metadata ID")?.parse::<u64>()?;
        let route=if kind=="tv" {"aggregate_credits"} else {"credits"};
        let credits=self.request(&format!("{kind}/{id}/{route}"), &[("language","en-US")]).await?;
        Ok(cast_rows(&credits["cast"]))
    }

    /// A series' production status and last air date, for entities saved before these were kept.
    pub async fn series_run(&self, entity: &Value) -> Result<Value> {
        let id=entity["provider_id"].as_str().context("Missing metadata ID")?.parse::<u64>()?;
        let detail=self.request(&format!("tv/{id}"), &[("language","en-US")]).await?;
        Ok(json!({"series_status":detail["status"].as_str().unwrap_or(""),"last_air_date":detail["last_air_date"].as_str().unwrap_or("")}))
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
            if entity["cast"].is_array() && (kind != "tv" || entity["series_status"].is_string())
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
            entity["series_status"] = json!(detail["status"].as_str().unwrap_or(""));
            entity["last_air_date"] = json!(detail["last_air_date"].as_str().unwrap_or(""));
        }
        entity["summary"] = detail["overview"].clone();
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
    use super::*;
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
