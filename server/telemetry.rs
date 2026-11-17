//! Transfer rates and torrent activity, sampled every 5 s from Transmission.
//!
//! The samples are averaged in memory and written once a minute: one row for the whole
//! transfer and one for each torrent that moved data in that minute. (Every sample used to be
//! written for every torrent, 105 of them idle, with the ETA smoother's state, every 5 s.)
//! Activity (complete, seeding goal reached) is a row per torrent, written when it changes.
use crate::storage::{Database, now};
use anyhow::Result;
use rusqlite::params;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default, Clone, Copy)]
struct Sum { download: f64, upload: f64, samples: u32 }
impl Sum {
    fn add(&mut self, download: f64, upload: f64) { self.download += download; self.upload += upload; self.samples += 1; }
    fn average(&self) -> (f64, f64) { let n = f64::from(self.samples.max(1)); (self.download / n, self.upload / n) }
    fn moved(&self) -> bool { self.download > 0.0 || self.upload > 0.0 }
}
/// The ETA smoother: when it started, last sampled, last saw data, and its rate.
#[derive(Clone, Copy)]
struct Eta { started: f64, updated: f64, activity: f64, rate: f64 }

#[derive(Default)]
struct Minute { start: i64, total: Sum, torrents: HashMap<String, Sum> }
static MINUTE: Mutex<Option<Minute>> = Mutex::new(None);
static ETA: std::sync::LazyLock<Mutex<HashMap<String, Eta>>> = std::sync::LazyLock::new(Default::default);
/// Each torrent's (complete, goal reached) as last stored; loaded once.
static ACTIVITY: Mutex<Option<HashMap<String, (bool, bool)>>> = Mutex::new(None);

const KEEP: i64 = 25 * 3600;

impl Database {
    /// Adds this sample to the minute's averages, writing the minute before once it is over,
    /// and gives each torrent its smoothed ETA.
    pub fn sample_torrents(&mut self, torrents: &mut [Value]) -> Result<()> {
        let time = now();
        let minute = (time as i64 / 60) * 60;
        let finished = {
            let mut current = MINUTE.lock().map_err(|_| anyhow::anyhow!("Rates unavailable"))?;
            let finished = current.take_if(|m| m.start != minute);
            let bucket = current.get_or_insert_with(|| Minute { start: minute, ..Default::default() });
            let mut eta = ETA.lock().map_err(|_| anyhow::anyhow!("Rates unavailable"))?;
            let mut seen = std::collections::HashSet::new();
            for torrent in torrents.iter_mut() {
                let Some(hash) = torrent["hash_string"].as_str().map(str::to_owned) else { continue };
                let download = torrent["rate_download"].as_f64().unwrap_or(0.0).max(0.0);
                let upload = torrent["rate_upload"].as_f64().unwrap_or(0.0).max(0.0);
                bucket.total.download += download;
                bucket.total.upload += upload;
                bucket.torrents.entry(hash.clone()).or_default().add(download, upload);
                torrent["smoothed_eta"] = json!(estimate(&mut eta, &hash, torrent, download, time));
                seen.insert(hash);
            }
            bucket.total.samples += 1;
            eta.retain(|hash, _| seen.contains(hash));
            finished
        };
        if let Some(done) = finished { self.write_minute(&done)?; }
        Ok(())
    }

    fn write_minute(&mut self, minute: &Minute) -> Result<()> {
        let tx = self.connection.transaction()?;
        let (download, upload) = minute.total.average();
        tx.execute("INSERT OR REPLACE INTO transfer_rates VALUES(?,?,?)", params![minute.start, download, upload])?;
        for (hash, sum) in minute.torrents.iter().filter(|(_, sum)| sum.moved()) {
            let (download, upload) = sum.average();
            tx.execute("INSERT OR REPLACE INTO torrent_rates(hash,time,download,upload,samples) VALUES(?,?,?,?,?)", params![hash, minute.start, download, upload, sum.samples])?;
        }
        tx.execute("DELETE FROM transfer_rates WHERE time<?", [minute.start - KEEP])?;
        tx.execute("DELETE FROM torrent_rates WHERE time<?", [minute.start - KEEP])?;
        tx.commit()?;
        Ok(())
    }

    /// A torrent's minute averages for the last day, a zero for each minute it moved nothing.
    pub fn torrent_rates(&self, hash: &str) -> Result<Vec<Value>> {
        let since = (now() as i64 / 60) * 60 - 86400;
        let stored = self.rows("SELECT time,download,upload FROM torrent_rates WHERE hash=? AND time>=? ORDER BY time", params![hash, since])?;
        let mut by_minute: std::collections::BTreeMap<i64, Value> = stored.into_iter().filter_map(|r| Some((r["time"].as_i64()?, r))).collect();
        if let Ok(current) = MINUTE.lock() && let Some(m) = current.as_ref() && let Some(sum) = m.torrents.get(hash) {
            let (download, upload) = sum.average();
            by_minute.insert(m.start, json!({"time":m.start,"download":download,"upload":upload}));
        }
        let Some((&first, _)) = by_minute.first_key_value() else { return Ok(Vec::new()) };
        let last = (now() as i64 / 60) * 60;
        Ok((first..=last).step_by(60).map(|t| by_minute.remove(&t).unwrap_or_else(|| json!({"time":t,"download":0.0,"upload":0.0}))).collect())
    }

    /// The whole transfer's minute averages for the last day, with the minute under way.
    pub fn transfer_rates(&self) -> Result<Vec<Value>> {
        let mut rows = self.rows("SELECT CAST(time/60 AS INTEGER)*60 AS time,AVG(download) AS download,AVG(upload) AS upload FROM transfer_rates WHERE time>=? GROUP BY CAST(time/60 AS INTEGER) ORDER BY time", [now() as i64 - 86400])?;
        if let Ok(current) = MINUTE.lock() && let Some(m) = current.as_ref() && m.total.samples > 0 {
            let (download, upload) = m.total.average();
            rows.retain(|r| r["time"].as_i64() != Some(m.start));
            rows.push(json!({"time":m.start,"download":download,"upload":upload}));
        }
        Ok(rows)
    }
}

/// Seconds left at the smoothed rate, or -1 while there is no fair estimate.
fn estimate(eta: &mut HashMap<String, Eta>, hash: &str, torrent: &Value, speed: f64, time: f64) -> i64 {
    let downloading = torrent["status"].as_i64() == Some(4) && torrent["percent_done"].as_f64().unwrap_or(0.0) < 1.0;
    if !downloading { eta.remove(hash); return -1; }
    let next = match eta.get(hash) {
        Some(old) if time >= old.updated && time - old.updated < 60.0 => Eta {
            started: old.started,
            updated: time,
            activity: if speed > 0.0 { time } else { old.activity },
            rate: old.rate + (1.0 - (-(time - old.updated) / 120.0).exp()) * (speed - old.rate),
        },
        _ => Eta { started: time, updated: time, activity: time, rate: speed },
    };
    eta.insert(hash.to_owned(), next);
    let remaining = torrent["left_until_done"].as_f64().unwrap_or(0.0).max(0.0);
    let seconds = remaining / next.rate.max(1.0);
    if time - next.started >= 15.0 && time - next.activity < 30.0 && next.rate > 0.0 && torrent["is_stalled"] != true
        && seconds.is_finite() && seconds < i64::MAX as f64 { seconds.round() as i64 } else { -1 }
}

impl Database {
    /// Events for a download finishing and a seeding goal being reached. Only a torrent whose
    /// state changed is written.
    pub fn record_activity(&mut self, torrents: &[Value], policy: Option<&Value>) -> Result<()> {
        let mut known = ACTIVITY.lock().map_err(|_| anyhow::anyhow!("Activity unavailable"))?;
        if known.is_none() {
            let rows = self.rows("SELECT hash,complete,reached FROM torrent_activity", [])?;
            *known = Some(rows.iter().filter_map(|r| Some((r["hash"].as_str()?.to_owned(), (r["complete"].as_i64() == Some(1), r["reached"].as_i64() == Some(1))))).collect());
        }
        let known = known.as_mut().expect("loaded above");
        let mut changed = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for t in torrents {
            let Some(hash) = t["hash_string"].as_str() else { continue };
            seen.insert(hash.to_owned());
            let complete = t["percent_done"].as_f64().unwrap_or(0.0) >= 1.0 && t["left_until_done"].as_u64() == Some(0);
            let ratio = t["upload_ratio"].as_f64().unwrap_or(0.0);
            let goal = match t["seed_ratio_mode"].as_u64().unwrap_or(0) {
                1 => t["seed_ratio_limit"].as_f64(),
                0 => policy.filter(|p| p["seed_ratio_limited"] == true).and_then(|p| p["seed_ratio_limit"].as_f64()),
                _ => None,
            };
            let reached = complete && goal.is_some_and(|g| g > 0.0 && ratio >= g);
            let old = known.get(hash).copied();
            if old == Some((complete, reached)) { continue; }
            // A torrent seen for the first time is recorded without events, as before.
            if let Some((was_complete, was_reached)) = old {
                if !was_complete && complete {
                    self.add_event(&json!({"kind":"download_complete","time":now(),"hash":hash,"name":t["name"]}))?;
                }
                if !was_reached && reached {
                    self.add_event(&json!({"kind":"seeding_ratio_reached","time":now(),"hash":hash,"name":t["name"],"ratio":ratio,"goal":goal}))?;
                }
            }
            known.insert(hash.to_owned(), (complete, reached));
            changed.push((hash.to_owned(), complete, reached));
        }
        let gone: Vec<String> = known.keys().filter(|h| !seen.contains(*h)).cloned().collect();
        if changed.is_empty() && gone.is_empty() { return Ok(()); }
        let tx = self.connection.transaction()?;
        for (hash, complete, reached) in changed {
            tx.execute("INSERT INTO torrent_activity VALUES(?,?,?) ON CONFLICT(hash) DO UPDATE SET complete=excluded.complete,reached=excluded.reached", params![hash, complete, reached])?;
        }
        for hash in gone {
            tx.execute("DELETE FROM torrent_activity WHERE hash=?", [&hash])?;
            known.remove(&hash);
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn db() -> Database {
        let db = Database { connection: rusqlite::Connection::open_in_memory().unwrap() };
        db.connection.execute_batch(include_str!("schema.sql")).unwrap();
        db
    }
    #[test]
    fn activity_is_written_only_when_it_changes_and_finishing_is_an_event() {
        let mut db = db();
        *ACTIVITY.lock().unwrap() = None;
        let downloading = json!({"hash_string":"h","name":"Film","percent_done":0.5,"left_until_done":5});
        let done = json!({"hash_string":"h","name":"Film","percent_done":1.0,"left_until_done":0});
        db.record_activity(std::slice::from_ref(&downloading), None).unwrap();
        db.record_activity(std::slice::from_ref(&downloading), None).unwrap();
        assert_eq!(db.events().unwrap().as_array().unwrap().len(), 0);
        db.record_activity(std::slice::from_ref(&done), None).unwrap();
        let events = db.events().unwrap();
        assert_eq!(events[0]["kind"], "download_complete");
        let stored: i64 = db.connection.query_row("SELECT complete FROM torrent_activity WHERE hash='h'", [], |r| r.get(0)).unwrap();
        assert_eq!(stored, 1);
        // Gone from Transmission: forgotten.
        db.record_activity(&[], None).unwrap();
        let rows: i64 = db.connection.query_row("SELECT count(*) FROM torrent_activity", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 0);
    }
}
