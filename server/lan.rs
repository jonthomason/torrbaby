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

/// Whether the peer is on the same local network as one of this Mac's own Wi-Fi or
/// Ethernet interfaces, as they are now. Nothing is configured: a new DHCP address or a
/// renumbered home network needs no change, and a tunnel (Tailscale) never counts.
pub fn trusted(peer: IpAddr) -> bool {
    let address = match peer {
        IpAddr::V4(a) => a,
        IpAddr::V6(a) => match a.to_ipv4_mapped() {
            Some(a) => a,
            None => return false,
        },
    };
    if !address.is_private() { return false; }
    own_networks().iter().any(|(net, mask)| u32::from(address) & mask == *net)
}

/// (network, mask) of each IPv4 address on the Mac's `en*` interfaces that are up.
fn own_networks() -> Vec<(u32, u32)> {
    let mut result = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut list) } != 0 { return result; }
    let mut cursor = list;
    while !cursor.is_null() {
        let entry = unsafe { &*cursor };
        cursor = entry.ifa_next;
        let name = unsafe { std::ffi::CStr::from_ptr(entry.ifa_name) }.to_string_lossy();
        if !name.starts_with("en") || entry.ifa_flags & libc::IFF_UP as u32 == 0 || entry.ifa_addr.is_null() || entry.ifa_netmask.is_null() { continue; }
        if unsafe { (*entry.ifa_addr).sa_family } as i32 != libc::AF_INET { continue; }
        let address = u32::from_be(unsafe { (*(entry.ifa_addr as *const libc::sockaddr_in)).sin_addr.s_addr });
        let mask = u32::from_be(unsafe { (*(entry.ifa_netmask as *const libc::sockaddr_in)).sin_addr.s_addr });
        if mask != 0 && Ipv4Addr::from(address).is_private() { result.push((address & mask, mask)); }
    }
    unsafe { libc::freeifaddrs(list) };
    result
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
