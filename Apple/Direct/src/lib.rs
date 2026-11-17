//! Torrbaby away from home, the device's half (Diamond's is server/direct.rs): a C interface
//! for the iPhone/iPad app and its packet tunnel (Swift), and the same in Rust for the Android
//! TV app (Android/native).
//!
//! The path to Diamond is ABT's shared abt-link (punch, route, end-to-end encryption; the user,
//! 2026-11-17: Torrbaby uses ABT's code when ABT is not installed). Torrbaby's own parts: the
//! relay through bronze as transport (relay.rs), the link's keeper (proxy.rs), and the packet
//! tunnel's TCP stack (packet.rs).
use std::ffi::{CStr, c_char};
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Duration;

pub mod packet;
pub mod proxy;
mod relay;

pub(crate) fn text(pointer: *const c_char) -> Option<String> {
    if pointer.is_null() { return None }
    unsafe { CStr::from_ptr(pointer) }.to_str().ok().map(str::to_owned)
}

/// `text` into `buffer`, NUL-terminated and cut to fit; its length, or -1.
fn write_out(text: &str, buffer: *mut c_char, capacity: usize) -> i32 {
    if buffer.is_null() || capacity == 0 { return -1 }
    let bytes = text.as_bytes();
    let length = bytes.len().min(capacity - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, buffer, length);
        *buffer.add(length) = 0;
    }
    length as i32
}

/// This device's Torrbaby client id: the relay names its connection with it, so Diamond's
/// Connection page can show which devices use bronze (bronze is told nothing more).
pub(crate) static CLIENT: Mutex<String> = Mutex::new(String::new());

#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_client(id: *const c_char) {
    if let Ok(mut held) = CLIENT.lock() { *held = text(id).unwrap_or_default() }
}

/// Diamond's identity (SHA-256 of its certificate, hex), learned from pairing or from Diamond at
/// home: relayed streams are TLS to it end to end, and the direct path must present it.
static PINNED: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn pinned() -> Option<String> { PINNED.lock().ok().and_then(|held| held.clone()) }

/// Pin Diamond's identity; empty forgets it. The link takes it at its next start.
#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_identity(fingerprint: *const c_char) {
    let fingerprint = text(fingerprint).map(|f| f.trim().to_lowercase()).filter(|f| f.len() == 64 && f.chars().all(|c| c.is_ascii_hexdigit()));
    if let Ok(mut held) = PINNED.lock() { *held = fingerprint }
}

/// Start Torrbaby's link to Diamond (proxy.rs) and return its loopback port (0 only when it could
/// not start). `config` is JSON: {"relay": "ip:7443", "relay_fingerprint", "token", "state_dir", "lean", "idle_s"}; Diamond's identity is the pinned one.
/// Already running for the same relay and identity: takes the new token and returns the same
/// port. Blocks up to about 7 s: call it off the main thread.
#[unsafe(no_mangle)]
pub extern "C" fn tbproxy_start(config: *const c_char) -> u16 {
    let config: serde_json::Value = text(config).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let string = |key: &str| config[key].as_str().unwrap_or("").to_owned();
    let mut link = proxy::Config::new(&string("relay"), &string("relay_fingerprint"));
    link.token = string("token");
    link.state_dir = config["state_dir"].as_str().filter(|s| !s.is_empty()).map(std::path::PathBuf::from);
    link.lean = config["lean"].as_bool().unwrap_or(false);
    if let Some(idle) = config["idle_s"].as_u64() { link.idle = Duration::from_secs(idle) }
    proxy::start(link)
}

/// Whether the link is running.
#[unsafe(no_mangle)]
pub extern "C" fn tbproxy_running() -> bool { proxy::running() }

/// Stop the link.
#[unsafe(no_mangle)]
pub extern "C" fn tbproxy_stop() { proxy::stop() }

/// The device's network now: `signature` changes with the network, `interface` is its name
/// ("en0", "pdp_ip0"). The link's sockets are bound to it.
#[unsafe(no_mangle)]
pub extern "C" fn tbproxy_network(signature: *const c_char, interface: *const c_char) {
    proxy::network(&text(signature).unwrap_or_default(), &text(interface).unwrap_or_default())
}

/// Back from the background: the direct path is dropped and punched again.
#[unsafe(no_mangle)]
pub extern "C" fn tbproxy_resumed() { proxy::resumed() }

/// Diamond's address on this network while at home ("ip:port"); empty: away.
#[unsafe(no_mangle)]
pub extern "C" fn tbproxy_home(address: *const c_char) {
    proxy::home(text(address).and_then(|a| a.parse::<SocketAddr>().ok()))
}

/// The link now, as JSON (proxy::state), NUL-terminated into `buffer`; its length, or -1.
/// Blocks briefly: not from inside an async task.
#[unsafe(no_mangle)]
pub extern "C" fn tbproxy_state(buffer: *mut c_char, capacity: usize) -> i32 {
    write_out(&proxy::state().to_string(), buffer, capacity)
}

#[unsafe(no_mangle)]
pub extern "C" fn tbtunnel_open_flows() -> u64 { packet::stats::get(&packet::stats::FLOWS_OPEN) }
#[unsafe(no_mangle)]
pub extern "C" fn tbtunnel_waiting_flows() -> u64 { packet::stats::get(&packet::stats::FLOWS_WAITING) }

/// Packets the stack sends back to the extension, in batches: `count` packets laid end to end
/// in `data`, with their lengths in `lengths`. (One call per packet was too slow for video.)
pub type PacketWriter = extern "C" fn(context: *mut std::ffi::c_void, data: *const u8, lengths: *const usize, count: usize);
struct Context(*mut std::ffi::c_void);
unsafe impl Send for Context {}

/// Start the packet tunnel's TCP stack (Network Extension). Each packet it sends goes to
/// `write(context, packet, length)`, from a background thread. A TCP connection that arrives
/// while the link has no path waits up to 20 s for one.
#[unsafe(no_mangle)]
pub extern "C" fn tbtunnel_start(mtu: u16, write: PacketWriter, context: *mut std::ffi::c_void) -> bool {
    tbtunnel_stop();
    let Some((stack, mut output)) = packet::Stack::start(mtu, Duration::from_secs(20)) else { return false };
    let context = Context(context);
    stack.runtime.spawn(async move {
        let context = context;
        let (mut data, mut lengths) = (Vec::with_capacity(64 * 1500), Vec::with_capacity(64));
        while let Some(packet) = output.recv().await {
            data.clear(); lengths.clear();
            data.extend_from_slice(&packet); lengths.push(packet.len());
            // Whatever else is already waiting goes in the same call (up to 64 packets).
            while lengths.len() < 64 { let Ok(more) = output.try_recv() else { break }; data.extend_from_slice(&more); lengths.push(more.len()); }
            packet::stats::add(&packet::stats::PACKETS_OUT, lengths.len() as u64);
            packet::stats::add(&packet::stats::BYTES_OUT, data.len() as u64);
            write(context.0, data.as_ptr(), lengths.as_ptr(), lengths.len());
        }
    });
    if let Ok(mut held) = packet::STACK.lock() { *held = Some(stack) }
    true
}

/// One packet the extension read from the tunnel.
#[unsafe(no_mangle)]
pub extern "C" fn tbtunnel_input(packet: *const u8, length: usize) -> bool {
    if packet.is_null() { return false }
    let bytes = unsafe { std::slice::from_raw_parts(packet, length) }.to_vec();
    packet::STACK.lock().ok().and_then(|held| held.as_ref().map(|stack| stack.input(bytes))).unwrap_or(false)
}

/// One line of counters for the extension's log: packets and bytes each way through the TCP
/// stack, drops, connections, and the link's state. NUL-terminated into `buffer`; its length, or -1.
#[unsafe(no_mangle)]
pub extern "C" fn tbtunnel_stats(buffer: *mut c_char, capacity: usize) -> i32 {
    use packet::stats::*;
    let line = format!(
        "in {} pkts/{} KB, out {} pkts/{} KB, dropped in {}, streams down {} KB up {} KB, flows open {} (opened {}, abandoned {}, uncarried {}); link {}",
        get(&PACKETS_IN), get(&BYTES_IN) / 1024, get(&PACKETS_OUT), get(&BYTES_OUT) / 1024, get(&DROPPED_IN),
        get(&STREAM_DOWN) / 1024, get(&STREAM_UP) / 1024, get(&FLOWS_OPEN), get(&FLOWS_OPENED), get(&FLOWS_ABANDONED), get(&FLOWS_UNCARRIED), proxy::status());
    write_out(&line, buffer, capacity)
}

/// Whether the packet tunnel's TCP stack is running (false after a panic inside it).
#[unsafe(no_mangle)]
pub extern "C" fn tbtunnel_alive() -> bool {
    packet::STACK.lock().ok().is_some_and(|held| held.is_some()) && packet::RUNNING.load(std::sync::atomic::Ordering::SeqCst)
}

/// Stop the packet tunnel's TCP stack.
#[unsafe(no_mangle)]
pub extern "C" fn tbtunnel_stop() {
    let stack = packet::STACK.lock().ok().and_then(|mut held| held.take());
    if let Some(stack) = stack { stack.runtime.shutdown_background() }
}
