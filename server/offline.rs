//! Idempotent upload of device-local watch progress, ordered by observation time.
use crate::storage::{Database, now};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};

pub fn shared_progress(db: &Database, id: &str, profile: &str) -> Result<Value> {
    db.require_profile(profile)?;
    let asset = db.asset(id)?;
    let hash = asset["torrent_hash"].as_str().unwrap_or(id);
    let index = asset["file_index"].as_u64().unwrap_or(0);
    let media = format!("{hash}:{index}");
    let row: Option<(f64, f64, bool, f64)> = db
        .connection
        .query_row(
            "SELECT position,duration,watched,last_played FROM profile_playback WHERE profile_id=? AND media_id=?",
            params![profile,media],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let (position, duration, watched, last_played) = row.unwrap_or((0.0, 0.0, false, 0.0));
    Ok(json!({"position":position,"duration":duration,"watched":watched,"last_played":last_played}))
}

#[derive(Deserialize)]
struct Progress {
    event_id: String,
    client_id: String,
    asset_id: String,
    position: f64,
    duration: f64,
    recorded_at: f64,
    reached_end: bool,
    watched_eligible: Option<bool>,
    #[serde(default)]
    profile_id: Option<String>,
}

pub fn sync(db: &mut Database, body: Value) -> Result<Value> {
    let report: Progress = serde_json::from_value(body).context("Invalid offline progress")?;
    let event = uuid::Uuid::parse_str(&report.event_id)?.to_string();
    let client = uuid::Uuid::parse_str(&report.client_id)?.to_string();
    let profile = report.profile_id.as_deref().unwrap_or(crate::profiles::LEGACY_PROFILE);
    db.require_profile(profile)?;
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
        "INSERT OR IGNORE INTO profile_playback(profile_id,media_id) VALUES(?,?)",
        params![profile,media],
    )?;
    let latest: f64 = tx.query_row(
        "SELECT last_played FROM profile_playback WHERE profile_id=? AND media_id=?",
        params![profile,media],
        |row| row.get(0),
    )?;
    let playing: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM profile_playback p JOIN playback_sessions s ON s.id=p.active_session WHERE p.profile_id=? AND p.media_id=? AND s.ended=0 AND p.last_played>?)",
        params![profile,media,time-30.0], |row| row.get(0))?;
    if playing && report.recorded_at > latest {
        // Leave this report pending while another device is actively watching.
        return Ok(json!({"acknowledged":false,"accepted":false,"reason":"playback_active"}));
    }
    // Clamp small clock skew; larger future dates fail validation and remain local.
    let observed = report.recorded_at.min(time);
    let accepted = observed > latest;
    let prior_watched: bool = tx.query_row("SELECT watched FROM profile_playback WHERE profile_id=? AND media_id=?", params![profile,media], |r|r.get(0))?;
    let watched = prior_watched || report.reached_end || (report.watched_eligible.unwrap_or(true)
        && crate::profiles::finished(report.position, report.duration, crate::profiles::closing_credits(&tx, &media, report.duration)?));
    if accepted {
        tx.execute("DELETE FROM profile_hidden_titles WHERE profile_id=? AND hidden_at<? AND title_id IN (SELECT json_extract(document,'$.entity_id') FROM library_assets WHERE json_extract(document,'$.torrent_hash')=? AND COALESCE(json_extract(document,'$.file_index'),0)=?)",params![profile,observed,hash,i64::try_from(index)?])?;
        tx.execute("UPDATE profile_playback SET position=?,duration=?,watched=?,last_played=?,client_id=?,active_session=NULL WHERE profile_id=? AND media_id=?",
            params![report.position.min(report.duration),report.duration,watched,observed,client,profile,media])?;
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
