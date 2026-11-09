//! Idempotent upload of device-local watch progress, ordered by observation time.
use crate::storage::{Database, now};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Progress {
    event_id: String,
    client_id: String,
    asset_id: String,
    position: f64,
    duration: f64,
    recorded_at: f64,
    reached_end: bool,
}

pub fn sync(db: &mut Database, body: Value) -> Result<Value> {
    let report: Progress = serde_json::from_value(body).context("Invalid offline progress")?;
    let event = uuid::Uuid::parse_str(&report.event_id)?.to_string();
    let client = uuid::Uuid::parse_str(&report.client_id)?.to_string();
    let time = now();
    ensure!(
        report.position.is_finite()
            && report.duration.is_finite()
            && report.position >= 0.0
            && report.duration > 0.0
            && report.position <= report.duration + 2.0
            && report.recorded_at.is_finite()
            && report.recorded_at > 0.0
            && report.recorded_at <= time + 300.0,
        "Invalid offline playback time or position"
    );
    let tx = db.connection.transaction()?;
    let prior: Option<String> = tx
        .query_row(
            "SELECT response FROM offline_progress_events WHERE client_id=? AND event_id=?",
            params![client, event],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(prior) = prior {
        return Ok(serde_json::from_str(&prior)?);
    }
    let registered: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM clients WHERE id=?)",
        [&client],
        |row| row.get(0),
    )?;
    ensure!(registered, "Register the client before syncing progress");
    let asset: String = tx.query_row(
        "SELECT document FROM library_assets WHERE id=?",
        [&report.asset_id],
        |row| row.get(0),
    )?;
    let asset: Value = serde_json::from_str(&asset)?;
    let hash = asset["torrent_hash"].as_str().unwrap_or(&report.asset_id);
    let index = asset["file_index"].as_u64().unwrap_or(0);
    let length = asset["size"].as_u64().context("Missing media size")?;
    let name = std::path::Path::new(asset["path"].as_str().context("Missing media path")?)
        .file_name()
        .context("Missing media name")?
        .to_string_lossy();
    let media = format!("{hash}:{index}");
    tx.execute("INSERT INTO media VALUES(?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,length=excluded.length",
        params![media,hash,i64::try_from(index)?,name,i64::try_from(length)?])?;
    tx.execute(
        "INSERT OR IGNORE INTO playback(media_id) VALUES(?)",
        [&media],
    )?;
    let latest: f64 = tx.query_row(
        "SELECT last_played FROM playback WHERE media_id=?",
        [&media],
        |row| row.get(0),
    )?;
    let playing: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM playback p JOIN playback_sessions s ON s.id=p.active_session WHERE p.media_id=? AND s.ended=0 AND p.last_played>?)",
        params![media,time-30.0], |row| row.get(0))?;
    if playing && report.recorded_at > latest {
        // Leave this report pending while another device is actively watching.
        return Ok(json!({"acknowledged":false,"accepted":false,"reason":"playback_active"}));
    }
    // Clamp small clock skew; larger future dates fail validation and remain local.
    let observed = report.recorded_at.min(time);
    let accepted = observed > latest;
    let watched = report.reached_end || report.position >= report.duration * 0.95;
    if accepted {
        tx.execute("UPDATE playback SET position=?,duration=?,watched=?,last_played=?,client_id=?,active_session=NULL WHERE media_id=?",
            params![report.position.min(report.duration),report.duration,watched,observed,client,media])?;
    }
    let response = json!({"acknowledged":true,"accepted":accepted,"reason":if accepted {"applied"} else {"newer_server_progress"}});
    tx.execute(
        "INSERT INTO offline_progress_events(client_id,event_id,response,received) VALUES(?,?,?,?)",
        params![client, event, response.to_string(), time],
    )?;
    tx.execute(
        "UPDATE clients SET last_seen=? WHERE id=?",
        params![time, client],
    )?;
    tx.commit()?;
    Ok(response)
}
