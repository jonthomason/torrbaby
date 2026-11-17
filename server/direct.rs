//! Diamond's side of a direct path to a device: answering the device's start request, spraying
//! its likely ports, then QUIC on the punched socket, each stream piped to this server's HTTP
//! port. The code is ABT's shared punch crate (`abt_punch::host`, owned by ABT; the user,
//! 2026-11-17: the best punching code lives in ABT and Torrbaby uses the same). Here: the port
//! for devices that come over IPv6, and each punch's outcome passed to bronze's shared hints.
pub use abt_punch::host::{DIRECT, fingerprint, ipv6, load_identity, pipe, results};

/// Diamond's UDP port for devices that reach it over IPv6 (nothing to punch).
pub const IPV6_PORT: u16 = 47800;

/// Every punch's outcome goes to bronze's shared punch hints (rendezvous::report_punch).
pub fn report_outcomes() {
    DIRECT.on_outcome(|o| crate::rendezvous::report_punch(&o.address, &o.kind, &o.spread, o.punch_ms));
}
