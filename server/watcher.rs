//! Watches shows for episodes the library lacks, and fetches them as they appear.
//!
//! A show is watched when the member says so, or by itself when it is in the library
//! and is airing now. Twice a day, around midday and in the evening, each watched show's
//! missing aired episodes are looked for where the show came from: cloud-drive shares for
//! a "china" show, torrents for a "world" one. What is found goes through the ordinary
//! download paths, so the usual rules about keeping the best copy apply. The evening run
//! is timed so a phone's overnight refresh finds the day's episodes waiting.
use crate::api::Service;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// Seconds after local midnight at which the shows are checked: 12:30 and 21:30.
const SLOTS: [u64; 2] = [12 * 3600 + 1800, 21 * 3600 + 1800];
/// A show airs "now" when it has an episode dated within this many days either way.
const CURRENT_DAYS: i64 = 60;
/// At most this many episodes of one show are sought as torrents in one check.
const TORRENTS_PER_CHECK: usize = 8;

#[derive(Clone, Default, Serialize, Deserialize)]
struct Show {
    /// "china" (cloud-drive shares) or "world" (torrents).
    market: String,
    /// Added by the server because the show is airing, not by the member.
    #[serde(default)]
    auto: bool,
    added: f64,
    #[serde(default)]
    checked: f64,
    /// Episodes queued by the last check, and what went wrong if anything did.
    #[serde(default)]
    found: u64,
    #[serde(default)]
    error: Option<String>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct State {
    shows: BTreeMap<String, Show>,
    /// Shows the member took off the list; they are not added again by themselves.
    #[serde(default)]
    dismissed: Vec<String>,
    #[serde(default)]
    last_run: f64,
}
pub struct Watcher {
    file: PathBuf,
    state: Mutex<State>,
    wake: tokio::sync::Notify,
    running: Mutex<bool>,
}

fn now() -> f64 { crate::storage::now() }
/// A moment as the server's local date ("2026-11-12") and its seconds since local midnight.
fn local(seconds: i64) -> (String, u64) {
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    let moment = seconds as libc::time_t;
    unsafe { libc::localtime_r(&moment, &mut parts) };
    (format!("{:04}-{:02}-{:02}", parts.tm_year + 1900, parts.tm_mon + 1, parts.tm_mday), (parts.tm_hour * 3600 + parts.tm_min * 60 + parts.tm_sec) as u64)
}
/// Seconds from a time of day until the next check.
fn until_next(of_day: u64) -> u64 {
    SLOTS.iter().find(|slot| **slot > of_day).map_or(86400 - of_day + SLOTS[0], |slot| slot - of_day)
}
/// Which of a show's aired episodes to look for: the gaps in seasons the library has
/// some of, and a following season once it has begun. Seasons the library never had are
/// not fetched unasked: watching a show must not pull in its back catalogue.
fn wanted(missing: &Value) -> Vec<(u64, Vec<u64>)> {
    let seasons = missing["seasons"].as_array().cloned().unwrap_or_default();
    let owned = |season: &Value| season["aired"].as_u64().unwrap_or(0) > season["missing"].as_array().map_or(0, Vec::len) as u64;
    let newest = seasons.iter().filter(|s| owned(s)).filter_map(|s| s["season"].as_u64()).max();
    seasons.iter().filter(|s| owned(s) || (newest.is_some() && s["season"].as_u64() == newest.map(|n| n + 1))).filter_map(|s| {
        let episodes: Vec<u64> = s["missing"].as_array()?.iter().filter_map(|e| e["episode"].as_u64()).collect();
        (!episodes.is_empty()).then(|| Some((s["season"].as_u64()?, episodes))).flatten()
    }).collect()
}

impl Watcher {
    pub fn new(data: &Path) -> Self {
        let file = data.join("watch.json");
        let state = std::fs::read(&file).ok().and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or_default();
        Self { file, state: Mutex::new(state), wake: tokio::sync::Notify::new(), running: Mutex::new(false) }
    }
    fn save(&self, state: &State) {
        if let Ok(raw) = serde_json::to_vec(state) {
            let staged = self.file.with_extension("json.new");
            if std::fs::write(&staged, raw).is_ok() { let _ = std::fs::rename(staged, &self.file); }
        }
    }
    fn entities(s: &Service) -> Vec<Value> { s.db.lock().ok().and_then(|db| db.documents("entities").ok()).unwrap_or_default() }
    fn assets(s: &Service) -> Vec<Value> { s.db.lock().ok().and_then(|db| db.documents("assets").ok()).unwrap_or_default() }
    /// Whether a show has an episode dated close to today: it is airing, or about to.
    fn current(entity: &Value) -> bool {
        let today = now() as i64;
        let (from, to) = (local(today - CURRENT_DAYS * 86400).0, local(today + CURRENT_DAYS * 86400).0);
        entity["episodes"].as_array().into_iter().flatten().filter_map(|e| e["airdate"].as_str()).any(|date| date.len() == 10 && date >= from.as_str() && date <= to.as_str())
    }
    /// Where a show's files came from: loose files in the library are cloud downloads.
    fn market(id: &str, assets: &[Value]) -> &'static str {
        let mine: Vec<&Value> = assets.iter().filter(|a| a["entity_id"] == id && a["available"] == true).collect();
        if mine.iter().any(|a| a["torrent_hash"].as_str().is_none_or(str::is_empty)) { "china" } else { "world" }
    }
    /// Put shows that are airing now on the list. Nothing is downloaded here.
    fn adopt(&self, s: &Service) {
        let (entities, assets) = (Self::entities(s), Self::assets(s));
        let Ok(mut state) = self.state.lock() else { return };
        let mut changed = false;
        for entity in entities.iter().filter(|e| e["kind"] == "tv") {
            let Some(id) = entity["id"].as_str() else { continue };
            if state.shows.contains_key(id) || state.dismissed.iter().any(|gone| gone == id) || !Self::current(entity) { continue; }
            if !assets.iter().any(|a| a["entity_id"] == id && a["available"] == true) { continue; }
            state.shows.insert(id.to_owned(), Show { market: Self::market(id, &assets).into(), auto: true, added: now(), ..Show::default() });
            changed = true;
        }
        // A show added by itself leaves by itself, once it has stopped airing and nothing is missing.
        let finished: Vec<String> = state.shows.iter().filter(|(_, show)| show.auto).filter_map(|(id, _)| {
            let entity = entities.iter().find(|e| e["id"] == id.as_str())?;
            let missing = s.db.lock().ok()?.missing_episodes(id).ok()?;
            (!Self::current(entity) && wanted(&missing).is_empty()).then(|| id.clone())
        }).collect();
        for id in finished { state.shows.remove(&id); changed = true; }
        if changed { self.save(&state); }
    }
    pub fn list(&self, s: &Service) -> Value {
        self.adopt(s);
        let entities = Self::entities(s);
        let state = self.state.lock().map(|state| state.clone()).unwrap_or_default();
        let shows: Vec<Value> = state.shows.iter().filter_map(|(id, show)| {
            let entity = entities.iter().find(|e| e["id"] == id.as_str())?;
            let missing = s.db.lock().ok()?.missing_episodes(id).unwrap_or(Value::Null);
            let seasons: Vec<Value> = wanted(&missing).into_iter().map(|(season, episodes)| json!({"season": season, "episodes": episodes})).collect();
            Some(json!({"id": id, "title": entity["title"], "original_title": entity["original_title"], "year": entity["year"], "poster": entity["poster"],
                "market": show.market, "auto": show.auto, "airing": Self::current(entity), "missing": seasons,
                "checked": if show.checked > 0.0 { json!(show.checked) } else { Value::Null }, "found": show.found, "error": show.error}))
        }).collect();
        let of_day = local(now() as i64).1;
        json!({"shows": shows, "next_check": now() + until_next(of_day) as f64, "checking": self.running.lock().map(|r| *r).unwrap_or(false)})
    }
    pub fn add(&self, s: &Service, id: &str, market: Option<&str>) -> Result<Value> {
        let entity = Self::entities(s).into_iter().find(|e| e["id"] == id && e["kind"] == "tv").context("Only a TV show in the library can be watched")?;
        let market = match market { Some("china") => "china", Some("world") => "world", Some(_) => bail!("Market must be world or china"), None => Self::market(id, &Self::assets(s)) };
        let mut state = self.state.lock().map_err(|_| anyhow::anyhow!("Watch list unavailable"))?;
        state.dismissed.retain(|gone| gone != id);
        let show = state.shows.entry(id.to_owned()).or_insert_with(|| Show { added: now(), ..Show::default() });
        show.market = market.into(); show.auto = false;
        self.save(&state);
        Ok(json!({"watching": true, "title": entity["title"], "market": market}))
    }
    pub fn remove(&self, id: &str) -> Result<Value> {
        let mut state = self.state.lock().map_err(|_| anyhow::anyhow!("Watch list unavailable"))?;
        let removed = state.shows.remove(id).is_some();
        if !state.dismissed.iter().any(|gone| gone == id) { state.dismissed.push(id.to_owned()); }
        self.save(&state);
        Ok(json!({"removed": removed}))
    }
    /// Start a check now, without waiting for the next time of day.
    pub fn check_now(&self) -> Value { self.wake.notify_one(); json!({"started": true}) }

    /// Look for one show's missing episodes. Returns how many were queued.
    async fn check(&self, s: &Service, id: &str, market: &str) -> Result<u64> {
        let entity = Self::entities(s).into_iter().find(|e| e["id"] == id).context("The show is no longer in the library")?;
        let missing = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?.missing_episodes(id)?;
        let targets = wanted(&missing);
        if targets.is_empty() { return Ok(0); }
        let han = |text: &str| text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
        let (title, original) = (entity["title"].as_str().unwrap_or(""), entity["original_title"].as_str().unwrap_or(""));
        if market == "china" {
            let query = if han(original) { original } else { title };
            // Files land beside the show's existing loose files, under the folder it already has.
            let folder = Self::assets(s).iter().filter(|a| a["entity_id"] == id && a["torrent_hash"].as_str().is_none_or(str::is_empty))
                .find_map(|a| Path::new(a["path"].as_str()?).parent()?.file_name()?.to_str().map(str::to_owned)).unwrap_or_else(|| query.to_owned());
            if s.cloud.busy_with(&folder) { return Ok(0); }
            let found = crate::api::cloud_search(s, query).await?;
            let mut queued = 0u64;
            for (season, mut episodes) in targets {
                // The best copies first; each gives what it has of the episodes still wanted.
                let mut copies: Vec<(&Value, &Value)> = found["shares"].as_array().into_iter().flatten()
                    .flat_map(|share| share["copies"].as_array().into_iter().flatten().map(move |copy| (share, copy))).filter(|(_, copy)| copy["beyond"] != true).collect();
                copies.sort_by_key(|(_, copy)| crate::cloud::preference(copy));
                for (share, copy) in copies.into_iter().take(12) {
                    if episodes.is_empty() { break; }
                    let files: Vec<(&str, u64)> = share["files"].as_array().into_iter().flatten().filter(|file| file["copy"] == copy["key"])
                        .filter(|file| file["season"].as_u64().or(copy["season"].as_u64()).unwrap_or(1) == season)
                        .filter_map(|file| Some((file["path"].as_str()?, file["episode"].as_u64().filter(|n| episodes.contains(n))?))).collect();
                    if files.is_empty() { continue; }
                    let Some(url) = share["url"].as_str() else { continue };
                    s.cloud.add(s, &json!({"url": url, "title": folder, "season": season, "kind": "tv", "files": files.iter().map(|(path, _)| path).collect::<Vec<_>>()})).await?;
                    episodes.retain(|n| !files.iter().any(|(_, held)| held == n));
                    queued += files.len() as u64;
                    // One download per show per check: its copies must not race for the same folder.
                    return Ok(queued);
                }
            }
            return Ok(queued);
        }
        let year = entity["year"].as_str().and_then(|y| y.parse::<u16>().ok());
        let mut queued = 0u64;
        for (season, episode) in targets.into_iter().flat_map(|(season, episodes)| episodes.into_iter().map(move |episode| (season, episode))).take(TORRENTS_PER_CHECK) {
            let query = crate::torrent_search::Query { market: crate::torrent_search::Market::World, query: title.to_owned(), year, season: Some(season as u16), episode: Some(episode as u16), sources: Vec::new() };
            let Ok(result) = s.torrent_search.search(query).await else { continue };
            // One episode, with seeds; 1080p when there is one, then the best seeded.
            let best = result.items.iter().filter(|item| !item.magnet.is_empty() && item.seeders.unwrap_or(0) > 0
                && item.hints.season == Some(season as u16) && item.hints.episode == Some(episode as u16) && item.hints.episode_end.is_none() && item.hints.episode_count.is_none())
                .max_by_key(|item| (item.hints.resolution.as_deref() == Some("1080p"), item.seeders.unwrap_or(0)));
            let Some(best) = best else { continue };
            if crate::organizer::add(s, &json!({"magnet": best.magnet})).await.is_ok() { queued += 1; }
        }
        Ok(queued)
    }
    async fn run(&self, s: &Service) {
        if let Ok(mut running) = self.running.lock() { if *running { return; } *running = true; }
        self.adopt(s);
        let shows: Vec<(String, String)> = self.state.lock().map(|state| state.shows.iter().map(|(id, show)| (id.clone(), show.market.clone())).collect()).unwrap_or_default();
        for (id, market) in shows {
            let outcome = self.check(s, &id, &market).await;
            if let Err(error) = &outcome { tracing::warn!(show = id, %error, "Could not check a watched show for new episodes"); }
            if let Ok(mut state) = self.state.lock() {
                if let Some(show) = state.shows.get_mut(&id) {
                    show.checked = now();
                    match outcome { Ok(found) => { show.found = found; show.error = None; } Err(error) => { show.found = 0; show.error = Some(error.to_string()); } }
                }
                state.last_run = now();
                self.save(&state);
            }
        }
        if let Ok(mut running) = self.running.lock() { *running = false; }
    }
    /// Check at each time of day, and whenever asked to.
    pub async fn work(&self, s: &Service) {
        self.adopt(s);
        loop {
            let wait = until_next(local(now() as i64).1);
            tokio::select! { _ = self.wake.notified() => {}, _ = tokio::time::sleep(Duration::from_secs(wait + 1)) => {} }
            self.run(s).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checks_fall_at_midday_and_in_the_evening() {
        assert_eq!(until_next(0), 12 * 3600 + 1800);
        assert_eq!(until_next(12 * 3600 + 1800), 9 * 3600);
        assert_eq!(until_next(13 * 3600), 8 * 3600 + 1800);
        assert_eq!(until_next(22 * 3600), 2 * 3600 + 12 * 3600 + 1800);
    }
    #[test]
    fn only_seasons_the_library_has_begun_are_filled() {
        let missing = json!({"seasons": [
            {"season": 1, "aired": 10, "missing": []},
            {"season": 2, "aired": 10, "missing": [{"episode": 4}, {"episode": 9}]},
            {"season": 3, "aired": 2, "missing": [{"episode": 1}, {"episode": 2}]},
            {"season": 4, "aired": 1, "missing": [{"episode": 1}]}]});
        // Season 2's gaps and the season after the newest owned; never a season further on.
        assert_eq!(wanted(&missing), vec![(2, vec![4, 9]), (3, vec![1, 2])]);
        // Seasons the library never had are not fetched because season 1 is complete... except the next one.
        let sampled = json!({"seasons": [{"season": 1, "aired": 10, "missing": []}, {"season": 2, "aired": 3, "missing": [{"episode": 1}, {"episode": 2}, {"episode": 3}]}, {"season": 3, "aired": 1, "missing": [{"episode": 1}]}]});
        assert_eq!(wanted(&sampled), vec![(2, vec![1, 2, 3])]);
        assert!(wanted(&json!({"seasons": [{"season": 1, "aired": 0, "missing": []}]})).is_empty());
    }
}
