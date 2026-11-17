//! Diamond's side of the secure path to a device: answering a punch (spray, QUIC on the punched
//! socket), its identity, and end-to-end encryption on relayed streams. The code is ABT's shared
//! abt-link (`abt_link::host`; the user, 2026-11-17: the best code lives in ABT, Torrbaby uses
//! the same). Here: the port for devices that come over IPv6. (Each punch's outcome goes to every
//! rendezvous server's shared hints from `keep_homes`, rendezvous.rs.)
pub use abt_link::host::{DIRECT, fingerprint, ipv6, load_identity, results, set_first_pairing};

/// Diamond's UDP port for devices that reach it over IPv6 (nothing to punch).
pub const IPV6_PORT: u16 = 47800;

