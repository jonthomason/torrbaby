//! The punch through both NATs: ABT's shared crate (`abt_punch::device`; the user, 2026-11-17:
//! the best punching code lives in ABT and Torrbaby uses the same when ABT is not installed).
//! Torrbaby keeps what is its own around it: QUIC on the punched socket with Diamond's pinned
//! identity, the loopback proxy, the relay and the packet tunnel (lib.rs, proxy.rs).
pub use abt_punch::device::HttpSignal;
pub use abt_punch::device::punch::{Failure, Plan, Report, ipv6, punch};
use serde_json::Value;

/// What bronze knows of the network this device is on (TBHINTS1 over the open relay), applied
/// to `plan`: the network's other public addresses (a hotel or carrier pool) for Diamond to
/// spray too, and random ports at once where a predicted spray failed there.
pub fn apply_hints(plan: &mut Plan, hints: Option<Value>) {
    let Some(hints) = hints else { return };
    for address in hints["outcomes"].as_array().into_iter().flatten().filter_map(|o| o["address"].as_str()).take(6) {
        if !plan.pool.iter().any(|a| a == address) { plan.pool.push(address.to_owned()) }
    }
    if hints["spread"] == "random" {
        if let Some(prefix) = hints["prefix"].as_str() { plan.random_from.push(prefix.split('/').next().unwrap_or(prefix).to_owned()) }
    }
}
