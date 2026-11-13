//! Feasibility test for direct connections that do not go through Tailscale.
//!
//! Both ends sit behind carrier NAT: China Telecom's shared address at home (stable
//! mapping, strict filtering) and a phone network (a new port per destination, handed out
//! in sequence). Neither can be reached from outside, so Tailscale relays. This test asks
//! whether the house can open its filter for the phone's next ports in advance: the phone
//! learns its public ports from Torrbaby's STUN server and reports them over the normal (relayed)
//! connection; Diamond opens its own UDP socket, learns its public address, and sprays a
//! window of the phone's likely ports while the phone's sockets fire at Diamond. Any
//! packet that arrives proves a direct path. No Torrbaby traffic uses it.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{collections::HashMap, net::{IpAddr, SocketAddr}, sync::{Arc, Mutex}, time::{Duration, Instant}};
use tokio::net::UdpSocket;

/// Torrbaby's own STUN server, on the rendezvous (bronze), set from the server's config
/// (`set_own_stun`). No third-party STUN servers.
const MAGIC: u32 = 0x2112A442;
const SPRAY_SECONDS: u64 = 25;
const MAX_WINDOW: u32 = 4096;

pub static TESTS: std::sync::LazyLock<Punch> = std::sync::LazyLock::new(Punch::default);

#[derive(Default)]
pub struct Punch { sessions: Mutex<HashMap<String, Arc<Mutex<Value>>>> }

static OWN_STUN: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Ask the rendezvous at `host` (its address, any port).
pub fn set_own_stun(rendezvous: &str) {
    let host = rendezvous.rsplit_once(':').map(|(host, _)| host).unwrap_or(rendezvous);
    let _ = OWN_STUN.set(format!("{host}:3478"));
}

/// The socket's public address as Torrbaby's STUN server sees it.
pub async fn stun(socket: &UdpSocket) -> Option<SocketAddr> {
    for server in OWN_STUN.get().map(String::as_str) {
        let Ok(mut addrs) = tokio::net::lookup_host(server).await else { continue };
        let Some(target) = addrs.find(|a| a.is_ipv4()) else { continue };
        let mut request = vec![0u8; 20];
        request[0..2].copy_from_slice(&1u16.to_be_bytes());
        request[4..8].copy_from_slice(&MAGIC.to_be_bytes());
        let txn: [u8; 12] = uuid::Uuid::new_v4().as_bytes()[..12].try_into().ok()?;
        request[8..20].copy_from_slice(&txn);
        if socket.send_to(&request, target).await.is_err() { continue }
        let mut buffer = [0u8; 1024];
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let Ok(Ok((len, from))) = tokio::time::timeout(deadline - Instant::now(), socket.recv_from(&mut buffer)).await else { break };
            if from != target || len < 20 || buffer[8..20] != txn { continue }
            if let Some(found) = mapped(&buffer[..len]) { return Some(found) }
        }
    }
    None
}

fn mapped(data: &[u8]) -> Option<SocketAddr> {
    let mut i = 20;
    while i + 4 <= data.len() {
        let kind = u16::from_be_bytes([data[i], data[i + 1]]);
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        let value = data.get(i + 4..i + 4 + len)?;
        if (kind == 0x0020 || kind == 0x8020) && value.len() >= 8 && value[1] == 1 {
            let port = u16::from_be_bytes([value[2], value[3]]) ^ (MAGIC >> 16) as u16;
            let ip = u32::from_be_bytes([value[4], value[5], value[6], value[7]]) ^ MAGIC;
            return Some(SocketAddr::new(IpAddr::from(ip.to_be_bytes()), port));
        }
        if kind == 0x0001 && value.len() >= 8 && value[1] == 1 {
            let port = u16::from_be_bytes([value[2], value[3]]);
            return Some(SocketAddr::new(IpAddr::from([value[4], value[5], value[6], value[7]]), port));
        }
        i += 4 + len + (4 - len % 4) % 4;
    }
    None
}

impl Punch {
    /// Start a test for the phone at `ip`, spraying ports `low..=high`.
    pub async fn start(&self, body: &Value) -> Result<Value> {
        let ip: IpAddr = body["ip"].as_str().context("ip is required")?.parse().context("ip must be an address")?;
        let low = body["low"].as_u64().context("low is required")? as u32;
        let high = body["high"].as_u64().context("high is required")? as u32;
        ensure!((1..=65535).contains(&low) && (low..=65535).contains(&high), "ports must be 1–65535, low ≤ high");
        let high = high.min(low + MAX_WINDOW - 1);
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        let public = stun(&socket).await.context("Diamond could not learn its public address")?;
        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_owned();
        let state = Arc::new(Mutex::new(json!({
            "id": id, "state": "spraying", "server": public.to_string(), "phone_ip": ip.to_string(),
            "low": low, "high": high, "connected_from": null, "connected_after_ms": null,
            "sprays": 0, "received": 0,
        })));
        {
            let mut sessions = self.sessions.lock().map_err(|_| anyhow::anyhow!("Test unavailable"))?;
            if sessions.len() > 20 { sessions.clear(); }
            sessions.insert(id.clone(), state.clone());
        }
        tokio::spawn(run(socket, ip, low, high, id.clone(), state));
        Ok(json!({"id": id, "server": public.to_string(), "low": low, "high": high, "seconds": SPRAY_SECONDS}))
    }
    pub fn status(&self, id: &str) -> Result<Value> {
        let sessions = self.sessions.lock().map_err(|_| anyhow::anyhow!("Test unavailable"))?;
        let state = sessions.get(id).context("No such test")?;
        Ok(state.lock().map_err(|_| anyhow::anyhow!("Test unavailable"))?.clone())
    }
}

async fn run(socket: UdpSocket, ip: IpAddr, low: u32, high: u32, id: String, state: Arc<Mutex<Value>>) {
    let started = Instant::now();
    let spray = format!("TBPUNCH1 {id} spray").into_bytes();
    let prefix = format!("TBPUNCH1 {id} ");
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut buffer = [0u8; 512];
    let mut connected = false;
    let end = started + Duration::from_secs(SPRAY_SECONDS + 15);
    while Instant::now() < end {
        tokio::select! {
            _ = tick.tick() => {
                // Keep opening the filter for the whole window until something gets through.
                if !connected && started.elapsed() < Duration::from_secs(SPRAY_SECONDS) {
                    for port in low..=high { let _ = socket.send_to(&spray, SocketAddr::new(ip, port as u16)).await; }
                    if let Ok(mut s) = state.lock() { s["sprays"] = json!(s["sprays"].as_u64().unwrap_or(0) + 1); }
                }
            }
            received = socket.recv_from(&mut buffer) => {
                let Ok((len, from)) = received else { continue };
                let Ok(text) = std::str::from_utf8(&buffer[..len]) else { continue };
                if !text.starts_with(&prefix) { continue }
                // Echo it: the phone times the round trip from its own timestamp.
                let _ = socket.send_to(format!("TBACK {text}").as_bytes(), from).await;
                if let Ok(mut s) = state.lock() {
                    s["received"] = json!(s["received"].as_u64().unwrap_or(0) + 1);
                    if !connected {
                        connected = true;
                        s["state"] = json!("connected");
                        s["connected_from"] = json!(from.to_string());
                        s["connected_after_ms"] = json!(started.elapsed().as_millis() as u64);
                    }
                }
            }
        }
    }
    if let Ok(mut s) = state.lock() { if s["state"] == "spraying" { s["state"] = json!("failed") } else { s["state"] = json!("finished") } }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_xor_mapped_addresses() {
        // Binding success carrying XOR-MAPPED-ADDRESS 14.155.71.43:15619.
        let mut data = vec![0x01, 0x01, 0, 12, 0x21, 0x12, 0xA4, 0x42];
        data.extend_from_slice(&[0; 12]);
        let port = 15619u16 ^ 0x2112;
        let ip = u32::from_be_bytes([14, 155, 71, 43]) ^ MAGIC;
        data.extend_from_slice(&[0x00, 0x20, 0, 8, 0, 1]);
        data.extend_from_slice(&port.to_be_bytes());
        data.extend_from_slice(&ip.to_be_bytes());
        assert_eq!(mapped(&data), Some("14.155.71.43:15619".parse().unwrap()));
    }
}
