//! One-use pairing grants and independently revocable device credentials.
use crate::storage::{Database, now};
use anyhow::{Result, bail};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn authorized(db: &Database, bearer: &str) -> bool {
    let Some(token) = bearer.strip_prefix("Bearer ") else {
        return false;
    };
    if token.len() != 64 {
        return false;
    }
    db.connection
        .query_row(
            "SELECT 1 FROM device_credentials WHERE digest=?",
            [digest(token)],
            |_| Ok(()),
        )
        .optional()
        .ok()
        .flatten()
        .is_some()
}

pub fn handle(db: &mut Database, method: &str, path: &str, body: &Value) -> Result<Value> {
    if method == "POST" && path == "/v1/pairings" {
        db.connection
            .execute("DELETE FROM pairing_grants WHERE expires<?", [now()])?;
        let count: i64 =
            db.connection
                .query_row("SELECT COUNT(*) FROM pairing_grants", [], |r| r.get(0))?;
        if count >= 16 {
            bail!("Too many pairing codes. Close an existing code or wait five minutes.")
        }
        let code = secret();
        let id = uuid::Uuid::new_v4().to_string();
        let expires = now() + 300.0;
        db.connection.execute(
            "INSERT INTO pairing_grants(id,digest,expires) VALUES(?,?,?)",
            params![id, digest(&code), expires],
        )?;
        return Ok(json!({"id":id,"code":code,"expires":expires}));
    }
    if method == "POST" && path == "/v1/pairings/redeem" {
        let code = body["code"].as_str().unwrap_or("");
        if code.len() != 64 {
            bail!("Invalid or expired connection code")
        }
        let name = body["name"].as_str().unwrap_or("New device").trim();
        let platform = body["platform"].as_str().unwrap_or("unknown");
        if name.is_empty() || name.len() > 200 || platform.len() > 40 {
            bail!("Invalid device name")
        }
        let tx = db.connection.transaction()?;
        let id: Option<String> = tx
            .query_row(
                "SELECT id FROM pairing_grants WHERE digest=? AND expires>? AND client_id IS NULL",
                params![digest(code), now()],
                |r| r.get(0),
            )
            .optional()?;
        let Some(id) = id else {
            bail!("Invalid or expired connection code")
        };
        let client = uuid::Uuid::new_v4().to_string();
        let token = secret();
        tx.execute(
            "INSERT INTO clients(id,name,platform,created,last_seen) VALUES(?,?,?,?,?)",
            params![client, name, platform, now(), now()],
        )?;
        tx.execute(
            "INSERT INTO device_credentials(client_id,digest) VALUES(?,?)",
            params![client, digest(&token)],
        )?;
        tx.execute(
            "UPDATE pairing_grants SET client_id=? WHERE id=?",
            params![client, id],
        )?;
        tx.commit()?;
        return Ok(json!({"client_id":client,"token":token}));
    }
    if let Some(id) = path.strip_prefix("/v1/pairings/") {
        if method == "DELETE" {
            db.connection
                .execute("DELETE FROM pairing_grants WHERE id=?", [id])?;
            return Ok(json!({"ok":true}));
        }
        if method == "GET" {
            let result: Option<(f64, Option<String>)> = db
                .connection
                .query_row(
                    "SELECT expires,client_id FROM pairing_grants WHERE id=?",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((expires, client)) = result else {
                bail!("Connection code no longer exists")
            };
            return Ok(json!({"expires":expires,"paired":client.is_some(),"client_id":client}));
        }
    }
    if method == "DELETE" {
        if let Some(id) = path.strip_prefix("/v1/device-credentials/") {
            db.connection
                .execute("DELETE FROM device_credentials WHERE client_id=?", [id])?;
            return Ok(json!({"ok":true}));
        }
    }
    bail!("Unknown pairing action")
}
