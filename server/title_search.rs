//! Find by title: the title is chosen first (TMDB: a film or a series, World or China), then
//! what is wanted of it, and Diamond chooses the words. The indexes match whole words and
//! answer at most 100 results, the most seeded first, so a typed title buried what was
//! wanted under a long-running show's newest episodes ("Dancing With The Stars": seasons
//! 29-35 only). Asked for "S20", they return season packs, not "S20E05"; asked for
//! "S20E05", that episode. The name releases use is learned per title (the first of the
//! title, its original and TMDB's alternative titles that finds anything: "Doctor X Mafia in
//! White", not "Doctor X: Age of the White Mafia") and kept in `search_names`.
use crate::api::Service;
use crate::torrent_search::{Item, Market, ProviderStatus, Query, SearchResult, release_key, words};
use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use serde_json::Value;

/// At most this many single-episode searches for one season (a daily show has hundreds).
const EPISODE_SEARCHES: u64 = 40;
/// Country tags release names carry after a show's name ("Dancing With The Stars US").
const COUNTRIES: [&str; 9] = ["us", "uk", "au", "nz", "ca", "ie", "es", "de", "fr"];

fn parse_id(id: &str) -> Result<(&str, u64)> {
    let mut parts = id.splitn(3, ':');
    let (Some("tmdb"), Some(kind), Some(number)) = (parts.next(), parts.next(), parts.next()) else { bail!("Unknown title") };
    anyhow::ensure!(["movie", "tv"].contains(&kind), "Unknown title");
    Ok((kind, number.parse().context("Unknown title")?))
}

/// The title's facts from TMDB (see `Metadata::title_facts`).
pub async fn facts(s: &Service, id: &str) -> Result<Value> {
    let (kind, number) = parse_id(id)?;
    s.metadata.title_facts(kind, number).await
}

/// A name as release names write it: no apostrophes, "and" for "&", punctuation as spaces.
pub(crate) fn release_words(name: &str) -> String {
    name.replace(['\'', '’'], "").replace('&', " and ")
        .chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>()
        .split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether a release is this title: every word of the name, and no other country's tag.
fn belongs(item: &Item, name: &str, country: &str) -> bool {
    let found = words(&item.name);
    let wanted = words(name);
    if !wanted.iter().all(|w| found.contains(w)) { return false }
    let home = match country.to_ascii_lowercase().as_str() { "gb" => "uk".to_owned(), other => other.to_owned() };
    !COUNTRIES.iter().any(|tag| *tag != home && !wanted.contains(*tag) && found.contains(*tag))
}

/// Whether a release is what was asked: a whole season (or several), or one season's episodes.
fn wanted(item: &Item, kind: &str, want: &str, season: Option<u16>, year: Option<u16>) -> bool {
    // A film's release states its year; another year is another film (Dune 1984, Part Two).
    if kind == "movie" { return item.hints.season.is_none() && item.hints.episode.is_none()
        && item.hints.year.zip(year).is_none_or(|(stated, known)| stated.abs_diff(known) <= 1) }
    let h = &item.hints;
    let covers = |n: u16| h.season.is_some_and(|start| (start..=h.season_end.unwrap_or(start).max(start)).contains(&n));
    match want {
        "episodes" => h.episode.is_some() && h.episode_count.is_none() && season.is_none_or(|n| h.season == Some(n)),
        _ => (h.episode.is_none() || h.episode_count.is_some()) && (h.season.is_some() || h.episode_count.is_some())
            && season.is_none_or(|n| covers(n) || (h.season.is_none() && n == 1)),
    }
}

/// The searches for one name beyond the name alone (which `search` asks first, to learn
/// whether releases use that name).
fn plan(name: &str, kind: &str, want: &str, season: Option<u16>, year: Option<u16>, aired: u64, sources: &[String]) -> Vec<Query> {
    let query = |terms: String, season: Option<u16>, episode: Option<u16>, sources: Vec<String>|
        Query { market: Market::World, query: terms, year: if kind == "movie" { year } else { None }, season, episode, sources };
    let all = sources.to_vec();
    if kind == "movie" { return vec![query(name.to_owned(), None, None, all)] }
    match (want, season) {
        ("episodes", Some(n)) => {
            // Each aired episode by its own tag, from The Pirate Bay (Torrent CSV answers the
            // name's search with its newest episodes anyway).
            let tpb: Vec<String> = if all.is_empty() || all.iter().any(|s| s == "tpb") { vec!["tpb".into()] } else { all.clone() };
            std::iter::once(query(format!("{name} S{n:02}"), Some(n), None, all))
                .chain((1..=aired.min(EPISODE_SEARCHES) as u16).map(|e| query(format!("{name} S{n:02}E{e:02}"), Some(n), Some(e), tpb.clone())))
                .collect()
        }
        ("episodes", None) => vec![],
        (_, Some(n)) => vec![query(format!("{name} S{n:02}"), Some(n), None, all.clone()), query(format!("{name} Season {n}"), Some(n), None, all.clone()),
            query(format!("{name} Complete"), Some(n), None, all)],
        (_, None) => vec![query(format!("{name} Complete"), None, None, all.clone()), query(format!("{name} Season"), None, None, all)],
    }
}

fn merge(into: &mut Vec<Item>, providers: &mut Vec<ProviderStatus>, result: SearchResult) {
    for status in result.providers {
        match providers.iter_mut().find(|p| p.provider == status.provider) {
            Some(known) => { known.count += status.count; if known.error.is_none() { known.error = status.error } }
            None => providers.push(status),
        }
    }
    for item in result.items {
        let key = release_key(&item);
        match into.iter_mut().find(|known| release_key(known) == key) {
            Some(known) => {
                known.match_score = known.match_score.max(item.match_score);
                known.seeders = known.seeders.max(item.seeders);
                for source in item.sources { if !known.sources.iter().any(|s| s.provider == source.provider && s.id == source.id) { known.sources.push(source) } }
            }
            None => into.push(item),
        }
    }
}

/// POST /v1/search/torrents with `title` (a TMDB id from /v1/search/titles), `want`
/// ("season": whole seasons; "episodes": one season's episodes), `season` and `market`.
pub async fn search(s: &std::sync::Arc<Service>, body: &Value) -> Result<SearchResult> {
    let id = body["title"].as_str().context("Choose a title")?;
    let (kind, _) = parse_id(id)?;
    let facts = facts(s, id).await?;
    let want = body["want"].as_str().unwrap_or("season");
    let season = body["season"].as_u64().map(|n| n as u16);
    let country = facts["country"].as_str().unwrap_or("");
    let year = facts["year"].as_str().and_then(|y| y.parse::<u16>().ok());
    if body["market"] == "china" {
        // Cloud shares are found by the Chinese name, a later season by its own ("第2季").
        let name = facts["title_zh"].as_str().filter(|t| !t.is_empty()).or(facts["title"].as_str()).unwrap_or("").to_owned();
        let query = match season { Some(n) if n > 1 && kind == "tv" => format!("{name} 第{n}季"), _ => name };
        let mut result = crate::api::china_search(s, &query).await?;
        if kind == "tv" { result.items.retain(|item| wanted(item, kind, want, season, year)) }
        return Ok(result);
    }
    let sources: Vec<String> = body["sources"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect();
    let aired = season.and_then(|n| facts["seasons"].as_array()?.iter().find(|s| s["season"] == n)?.get("aired")?.as_u64()).unwrap_or(0);
    let learned: Option<String> = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?
        .connection.query_row("SELECT name FROM search_names WHERE title_id=?", [id], |r| r.get(0)).ok();
    let mut names: Vec<String> = learned.into_iter().chain(facts["names"].as_array().into_iter().flatten().filter_map(|n| n.as_str().map(release_words))).collect();
    let mut seen = std::collections::HashSet::new();
    names.retain(|n| !n.is_empty() && seen.insert(n.to_lowercase()));
    anyhow::ensure!(!names.is_empty(), "This title has no name to search for");
    let (mut items, mut providers) = (Vec::new(), Vec::new());
    // The first name whose first search finds this title is the one releases use.
    let mut chosen = None;
    for name in names.iter().take(4) {
        let queries = plan(name, kind, want, season, year, aired, &sources);
        // A film by its name and year, as its releases are named; a series by its name.
        let terms = match year { Some(year) if kind == "movie" => format!("{name} {year}"), _ => name.clone() };
        let alone = Query { market: Market::World, query: terms, year: if kind == "movie" { year } else { None }, season, episode: None, sources: sources.clone() };
        let result = s.torrent_search.search(alone).await?;
        let found = result.items.iter().any(|item| belongs(item, name, country));
        merge(&mut items, &mut providers, result);
        if found { chosen = Some((name.clone(), queries)); break }
    }
    let used = chosen.as_ref().map(|(name, _)| name.clone()).unwrap_or_else(|| names[0].clone());
    if let Some((name, queries)) = chosen {
        if s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?
            .connection.execute("INSERT INTO search_names(title_id,name) VALUES(?,?) ON CONFLICT(title_id) DO UPDATE SET name=excluded.name WHERE name!=excluded.name", [id, &name])? > 0 {
            tracing::info!(title = id, name, "Find: releases name this title");
        }
        // The rest at four at a time: the indexes limit how fast they may be asked.
        let mut rest = futures_util::stream::iter(queries.into_iter().map(|query| s.torrent_search.search(query))).buffer_unordered(4);
        while let Some(result) = rest.next().await {
            match result {
                Ok(result) => merge(&mut items, &mut providers, result),
                Err(error) => tracing::warn!(title = id, %error, "Find: one of a title's searches failed"),
            }
        }
    }
    items.retain(|item| belongs(item, &used, country) && wanted(item, kind, want, season, year));
    items.sort_by(|a, b| b.match_score.cmp(&a.match_score).then(b.seeders.unwrap_or(0).cmp(&a.seeders.unwrap_or(0))));
    Ok(SearchResult { query: used, items, providers, cached: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item(name: &str) -> Item {
        let mut item = crate::torrent_search::cloud_item(name.into(), "x", "https://pan.quark.cn/s/x", None, None, 0, "", None);
        item.hints = crate::torrent_search::hints(name);
        item
    }
    #[test]
    fn release_words_are_written_as_release_names_write_them() {
        assert_eq!(release_words("Grey's Anatomy"), "Greys Anatomy");
        assert_eq!(release_words("Doctor X: Age of the White Mafia"), "Doctor X Age of the White Mafia");
        assert_eq!(release_words("Law & Order"), "Law and Order");
    }
    #[test]
    fn another_countrys_show_is_not_this_one() {
        let name = "Dancing with the Stars";
        assert!(belongs(&item("Dancing With The Stars US S34E05 720p WEB h264-EDITH"), name, "US"));
        assert!(belongs(&item("Dancing.With.The.Stars.S34E05.720p"), name, "US"));
        assert!(!belongs(&item("Dancing With The Stars AU S21E05 HDTV x264-FQM"), name, "US"));
        assert!(!belongs(&item("Dancing On Ice S01E01"), name, "US"));
    }
    #[test]
    fn whole_seasons_and_episodes_are_told_apart() {
        let pack = item("Dancing With The Stars US S20 1080p WEB");
        let range = item("Friends S01-S10 Complete 1080p");
        let episode = item("Dancing With The Stars US S20E05 720p");
        assert!(wanted(&pack, "tv", "season", Some(20), None) && !wanted(&pack, "tv", "episodes", Some(20), None));
        assert!(wanted(&range, "tv", "season", Some(4), None) && wanted(&range, "tv", "season", None, None));
        assert!(wanted(&episode, "tv", "episodes", Some(20), None) && !wanted(&episode, "tv", "episodes", Some(21), None));
        assert!(!wanted(&episode, "tv", "season", Some(20), None));
    }
    #[test]
    fn another_years_film_is_another_film() {
        assert!(wanted(&item("Dune.2021.2160p.BluRay.REMUX"), "movie", "", None, Some(2021)));
        assert!(wanted(&item("Dune 1080p WEB"), "movie", "", None, Some(2021)));
        assert!(!wanted(&item("Dune Part Two 2024 1080p"), "movie", "", None, Some(2021)));
        assert!(!wanted(&item("Dune 1984 1080p BluRay"), "movie", "", None, Some(2021)));
    }
    #[test]
    fn a_season_asks_for_its_pack_and_each_aired_episode() {
        let queries = plan("Dancing With The Stars", "tv", "episodes", Some(34), None, 3, &[]);
        let terms: Vec<&str> = queries.iter().map(|q| q.query.as_str()).collect();
        assert_eq!(terms, ["Dancing With The Stars S34", "Dancing With The Stars S34E01", "Dancing With The Stars S34E02", "Dancing With The Stars S34E03"]);
        assert_eq!(queries[1].sources, ["tpb"]);
        let packs = plan("Friends", "tv", "season", None, None, 0, &[]);
        assert_eq!(packs.iter().map(|q| q.query.as_str()).collect::<Vec<_>>(), ["Friends Complete", "Friends Season"]);
    }
}
