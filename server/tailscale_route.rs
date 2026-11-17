//! Video never crosses to the US and back (the user's rule, 2026-11-17: "I don't want movie
//! traffic going through the US at all. ever."). A request that reaches this server over
//! Tailscale (from 100.64.0.0/10) when Tailscale has no direct path to that device goes through
//! Tailscale's DERP relay, in San Francisco for this network. Media is refused then; the app
//! uses the home network, the relay through bronze, its direct path or ABT instead. Everything
//! else (the library, settings) may still come that way.
use serde_json::Value;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Tailscale's status, read at most every 10 s.
static STATUS: Mutex<Option<(Instant, Option<Value>)>> = Mutex::new(None);

/// A Tailscale address (100.64.0.0/10, or Tailscale's IPv6 range fd7a:115c:a1e0::/48).
pub fn tailscale(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => { let o = v4.octets(); o[0] == 100 && (64..128).contains(&o[1]) }
        IpAddr::V6(v6) => { let s = v6.segments(); s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0 }
    }
}

async fn status() -> Option<Value> {
    if let Some((at, value)) = STATUS.lock().ok().and_then(|held| held.clone()) {
        if at.elapsed() < Duration::from_secs(10) { return value }
    }
    let mut value = None;
    for program in ["/Applications/Tailscale.app/Contents/MacOS/Tailscale", "/usr/local/bin/tailscale", "/opt/homebrew/bin/tailscale"] {
        let run = tokio::process::Command::new(program).args(["status", "--json"]).kill_on_drop(true).output();
        if let Ok(Ok(output)) = tokio::time::timeout(Duration::from_secs(3), run).await {
            if output.status.success() { value = serde_json::from_slice::<Value>(&output.stdout).ok(); if value.is_some() { break } }
        }
    }
    if let Ok(mut held) = STATUS.lock() { *held = Some((Instant::now(), value.clone())) }
    value
}

/// Whether a request from `ip` comes over Tailscale through its relay (no direct path). A
/// Tailscale address whose path cannot be read counts as relayed: the rule has no exceptions.
pub async fn relayed(ip: IpAddr) -> bool {
    if !tailscale(ip) { return false }
    let Some(status) = status().await else { return true };
    !direct_in(&status, ip)
}

/// Whether `status` (tailscale status --json) has a direct path to the peer at `ip`.
fn direct_in(status: &Value, ip: IpAddr) -> bool {
    let text = ip.to_string();
    status["Peer"].as_object().into_iter().flat_map(|peers| peers.values())
        .find(|peer| peer["TailscaleIPs"].as_array().is_some_and(|ips| ips.iter().any(|i| i.as_str() == Some(text.as_str()))))
        .is_some_and(|peer| peer["CurAddr"].as_str().is_some_and(|a| !a.is_empty()))
}

pub const REFUSED: &str = "Video does not go through Tailscale's relay in the US";

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_a_direct_tailscale_path_carries_video() {
        let status = serde_json::json!({"Peer": {
            "a": {"TailscaleIPs": ["100.125.9.97"], "CurAddr": "192.168.1.6:41641", "Relay": "sfo"},
            "b": {"TailscaleIPs": ["100.90.66.115"], "CurAddr": "", "Relay": "sfo"}}});
        assert!(direct_in(&status, "100.125.9.97".parse().unwrap()));
        assert!(!direct_in(&status, "100.90.66.115".parse().unwrap()));
        assert!(!direct_in(&status, "100.1.1.1".parse().unwrap()));
        assert!(tailscale("100.80.147.115".parse().unwrap()) && !tailscale("127.0.0.1".parse().unwrap()) && !tailscale("100.128.0.1".parse().unwrap()));
    }
}
