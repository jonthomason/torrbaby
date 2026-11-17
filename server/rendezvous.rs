//! Diamond's standing connections to our rendezvous servers (abt-link's `bronze::ours()`):
//! bronze in Shenzhen, which also relays, and the backups on Shadow and miyu, which only carry
//! what sets up a direct path (the user, 2026-11-18: no single point of failure for direct
//! paths, and no relaying anywhere but bronze). Nothing can reach Diamond from outside China
//! Telecom's carrier NAT, so it connects out to each and stays connected; every stream a server
//! opens on it is a device's, TLS to Diamond's identity from end to end, and is piped to this
//! server's HTTP port (abt-link's `host::pipe_relayed`), where the device's own credentials are
//! checked as always. The servers hold none.
//!
//! The code is ABT's shared abt-link (`keep_homes`); this file starts it, reports which servers
//! hold Diamond, and asks bronze for its relay usage.
use crate::config::RendezvousConfig;
use abt_link::bronze::{Homes, keep_homes, ours, read_secret};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Diamond's name at the rendezvous servers.
const HOME: &str = "diamond";

static HOMES: Mutex<Option<Arc<Homes>>> = Mutex::new(None);
/// Each server's state: label → {"state": "connected" | "connecting", "since"}.
static STATE: Mutex<Option<Value>> = Mutex::new(None);
static STOPPED: AtomicBool = AtomicBool::new(false);

fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

fn held_by(label: &str) -> bool { HOMES.lock().ok().and_then(|h| h.clone()).is_some_and(|h| h.held().iter().any(|l| l == label)) }

/// bronze's account of the relay: the devices on it now (by Torrbaby client id, with the
/// bytes relayed for each) and bytes relayed per day, China time.
pub async fn usage() -> Result<Value> {
    let homes = HOMES.lock().ok().and_then(|h| h.clone()).context("Not connected to bronze")?;
    let reply = homes.ask("bronze", b"TBUSAGE1\n").await.context("bronze did not answer")?;
    Ok(serde_json::from_slice(&reply)?)
}

/// `usage`, with each device's name from `names` (client id → name).
pub async fn usage_named(names: std::collections::HashMap<String, String>) -> Result<Value> {
    let mut usage = usage().await?;
    for device in usage["devices"].as_array_mut().into_iter().flatten() {
        let name = device["client"].as_str().and_then(|c| names.get(c)).cloned();
        device["name"] = json!(name);
    }
    Ok(usage)
}

/// The relay devices are told about (bronze's address and fingerprint) and whether Diamond is
/// connected to it now. Null when no rendezvous is configured.
pub fn advertised() -> Value {
    if STATE.lock().ok().and_then(|s| s.clone()).is_none() { return Value::Null }
    let Some(bronze) = ours().into_iter().find(|r| r.relays) else { return Value::Null };
    json!({"address": bronze.bronze.quic.to_string(), "fingerprint": bronze.bronze.fingerprint, "connected": held_by("bronze")})
}

/// Each server's connection for /v1/status: {label: {"state", "since"}}.
pub fn status() -> Value { STATE.lock().ok().and_then(|held| held.clone()).unwrap_or(Value::Null) }

/// Keep connected to every rendezvous server for as long as the server runs. Call inside the
/// runtime: each server gets a task of its own.
pub fn spawn(config: RendezvousConfig, http_port: u16) {
    let servers = ours();
    crate::punch::set_own_stun(servers.iter().map(|r| r.stun.clone()).collect());
    let secret = match read_secret(&config.secret_file) {
        Ok(secret) => secret,
        Err(error) => return eprintln!("Rendezvous: reading {}: {error:#}", config.secret_file),
    };
    if let Ok(mut state) = STATE.lock() {
        *state = Some(Value::Object(servers.iter().map(|r| (r.bronze.label(), json!({"state": "connecting", "since": now()}))).collect()));
    }
    STOPPED.store(false, Ordering::SeqCst);
    let homes = keep_homes(servers.into_iter().map(|r| r.bronze).collect(), HOME.to_owned(), secret, http_port, None,
        |label, up| if let Ok(mut state) = STATE.lock() && let Some(all) = state.as_mut() {
            all[label] = json!({"state": if up { "connected" } else { "connecting" }, "since": now()});
        },
        || !STOPPED.load(Ordering::SeqCst));
    if let Ok(mut held) = HOMES.lock() { *held = Some(homes) }
}

/// Let go of the servers (the server is stopping).
pub fn stop() { STOPPED.store(true, Ordering::SeqCst) }
