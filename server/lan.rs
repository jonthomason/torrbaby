//! Explicit trusted-LAN bootstrap, based on the socket peer (never forwarded headers).
use crate::storage::{Database, now};
use anyhow::{Result, ensure};
use rusqlite::params;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
    sync::Mutex,
    time::{Duration, Instant},
};

pub fn network(value: &str) -> Result<(u32, u32)> {
    let (address, bits) = value
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("Trusted LAN must be IPv4 CIDR"))?;
    let address: Ipv4Addr = address.parse()?;
    let bits: u32 = bits.parse()?;
    ensure!(
        address.is_private() || address.is_loopback(),
        "Only private LAN or loopback ranges may be trusted"
    );
    ensure!((8..=32).contains(&bits), "Invalid trusted LAN prefix");
    let mask = u32::MAX << (32 - bits);
    let first = u32::from(address) & mask;
    let last = first | !mask;
    ensure!(
        (Ipv4Addr::from(first).is_private() && Ipv4Addr::from(last).is_private())
            || (Ipv4Addr::from(first).is_loopback() && Ipv4Addr::from(last).is_loopback()),
        "Trusted range must stay within private addresses"
    );
    Ok((first, mask))
}
pub fn trusted(peer: IpAddr, ranges: &[String]) -> bool {
    let address = match peer {
        IpAddr::V4(a) => a,
        IpAddr::V6(a) => match a.to_ipv4_mapped() {
            Some(a) => a,
            None => return false,
        },
    };
    ranges
        .iter()
        .any(|r| network(r).is_ok_and(|(net, mask)| u32::from(address) & mask == net))
}
#[derive(Default)]
pub struct Limiter(Mutex<HashMap<IpAddr, (Instant, u32)>>);
impl Limiter {
    pub fn admit(&self, peer: IpAddr) -> Result<()> {
        let mut entries = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("LAN registration unavailable"))?;
        entries.retain(|_, (start, _)| start.elapsed() < Duration::from_secs(60));
        ensure!(
            entries.len() < 256 || entries.contains_key(&peer),
            "LAN registration busy"
        );
        let entry = entries.entry(peer).or_insert((Instant::now(), 0));
        ensure!(
            entry.1 < 10,
            "Too many registration attempts; try again in a minute"
        );
        entry.1 += 1;
        Ok(())
    }
}
pub fn register(db: &mut Database, body: &Value) -> Result<Value> {
    let name = body["name"].as_str().unwrap_or("TV").trim();
    let platform = body["platform"].as_str().unwrap_or("TV").trim();
    ensure!(
        !name.is_empty() && name.len() <= 200 && !platform.is_empty() && platform.len() <= 40,
        "Invalid device name"
    );
    let count: i64 =
        db.connection
            .query_row("SELECT COUNT(*) FROM device_credentials", [], |r| r.get(0))?;
    ensure!(
        count < 256,
        "Device limit reached; remove unused devices in Settings"
    );
    let id = uuid::Uuid::new_v4().to_string();
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let digest: String = Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let tx = db.connection.transaction()?;
    tx.execute(
        "INSERT INTO clients(id,name,platform,created,last_seen) VALUES(?,?,?,?,?)",
        params![id, name, platform, now(), now()],
    )?;
    tx.execute(
        "INSERT INTO device_credentials(client_id,digest) VALUES(?,?)",
        params![id, digest],
    )?;
    tx.commit()?;
    Ok(json!({"client_id":id,"token":token}))
}
