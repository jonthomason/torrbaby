use crate::storage::{Database, now};
use anyhow::Result;
use rusqlite::params;
use serde_json::{Value, json};

impl Database {
    /// Persist minute averages, plus the smoother state, in one short transaction.
    pub fn sample_torrents(&mut self, torrents: &mut [Value]) -> Result<()> {
        let time = now();
        let minute = (time as i64 / 60) * 60;
        let tx = self.connection.transaction()?;
        let mut download_total = 0.0;
        let mut upload_total = 0.0;
        for torrent in torrents {
            let Some(hash) = torrent["hash_string"].as_str().map(str::to_owned) else {
                continue;
            };
            let speed = torrent["rate_download"].as_f64().unwrap_or(0.0).max(0.0);
            let upload = torrent["rate_upload"].as_f64().unwrap_or(0.0).max(0.0);
            download_total += speed;
            upload_total += upload;
            tx.execute("INSERT INTO torrent_rates(hash,time,download,upload,samples) VALUES(?,?,?,?,1) ON CONFLICT(hash,time) DO UPDATE SET download=(download*samples+excluded.download)/(samples+1),upload=(upload*samples+excluded.upload)/(samples+1),samples=samples+1",params![hash,minute,speed,upload])?;
            let downloading = torrent["status"].as_i64() == Some(4)
                && torrent["percent_done"].as_f64().unwrap_or(0.0) < 1.0;
            let mut estimate = -1_i64;
            if downloading {
                use rusqlite::OptionalExtension;
                let old: Option<(f64, f64, f64, f64)> = tx
                    .query_row(
                        "SELECT started,updated,activity,rate FROM eta_state WHERE hash=?",
                        [&hash],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .optional()?;
                let (started, activity, rate) = match old {
                    Some((started, updated, activity, rate))
                        if time >= updated && time - updated < 60.0 =>
                    {
                        (
                            started,
                            if speed > 0.0 { time } else { activity },
                            rate + (1.0 - (-(time - updated) / 120.0).exp()) * (speed - rate),
                        )
                    }
                    _ => (time, time, speed),
                };
                let remaining = torrent["left_until_done"].as_f64().unwrap_or(0.0).max(0.0);
                let seconds = remaining / rate.max(1.0);
                if time - started >= 15.0
                    && time - activity < 30.0
                    && rate > 0.0
                    && torrent["is_stalled"] != true
                    && seconds.is_finite()
                    && seconds < i64::MAX as f64
                {
                    estimate = seconds.round() as i64;
                }
                tx.execute("INSERT INTO eta_state VALUES(?,?,?,?,?) ON CONFLICT(hash) DO UPDATE SET started=excluded.started,updated=excluded.updated,activity=excluded.activity,rate=excluded.rate",params![hash,started,time,activity,rate])?;
            } else {
                tx.execute("DELETE FROM eta_state WHERE hash=?", [&hash])?;
            }
            torrent["smoothed_eta"] = json!(estimate);
        }
        tx.execute(
            "INSERT OR REPLACE INTO transfer_rates VALUES(?,?,?)",
            params![time as i64, download_total, upload_total],
        )?;
        tx.execute(
            "DELETE FROM transfer_rates WHERE time<?",
            [time as i64 - 25 * 3600],
        )?;
        tx.execute(
            "DELETE FROM torrent_rates WHERE time<?",
            [minute - 25 * 3600],
        )?;
        tx.execute("DELETE FROM eta_state WHERE updated<?", [time - 86400.0])?;
        tx.commit()?;
        Ok(())
    }
    pub fn torrent_rates(&self, hash: &str) -> Result<Vec<Value>> {
        self.rows(
            "SELECT time,download,upload FROM torrent_rates WHERE hash=? AND time>=? ORDER BY time",
            params![hash, now() as i64 - 86400],
        )
    }
}
