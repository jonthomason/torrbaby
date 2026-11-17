//! Diamond's side of the secure path to a device: answering a punch (spray, QUIC on the punched
//! socket), its identity, and end-to-end encryption on relayed streams. The code is ABT's shared
//! abt-link (`abt_link::host`; the user, 2026-11-17: the best code lives in ABT, Torrbaby uses
//! the same). Here: the port for devices that come over IPv6, and each punch's outcome passed to
//! bronze's shared hints.
pub use abt_link::host::{DIRECT, fingerprint, ipv6, load_identity, pipe_relayed, results, set_first_pairing};

/// Diamond's UDP port for devices that reach it over IPv6 (nothing to punch).
pub const IPV6_PORT: u16 = 47800;

/// Every punch's outcome goes to bronze's shared punch hints (rendezvous::report_punch).
pub fn report_outcomes() {
    DIRECT.on_outcome(|o| crate::rendezvous::report_punch(&o.address, &o.kind, &o.spread, o.punch_ms));
}
