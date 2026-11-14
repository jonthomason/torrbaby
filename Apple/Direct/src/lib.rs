//! Torrbaby's direct connection, the iPhone/iPad half (the server half is server/direct.rs).
//!
//! The app punches a path through both carriers' NAT with plain UDP sockets, then hands the
//! winning socket here. QUIC runs on that very socket, so the hole stays the same (iOS's own
//! QUIC would not transmit when told to reuse the port). The server's certificate is checked
//! against the fingerprint the app learned over its normal, trusted connection. A loopback
//! listener turns each connection the app or the player makes into a QUIC stream, which the
//! server pipes to its own HTTP port.
//!
//! The relay is the same machinery pointed at Torrbaby's rendezvous on bronze (ALPN
//! `tbrelay`), for when there is no home network, no Tailscale and no punched path yet.
use quinn::crypto::rustls::QuicClientConfig;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use std::ffi::{CStr, c_char};
use std::net::SocketAddr;
use std::os::fd::FromRawFd;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

pub mod e2e;
pub mod packet;
pub mod proxy;
pub mod punch;

pub(crate) struct Tunnel {
    runtime: tokio::runtime::Runtime,
    connection: quinn::Connection,
    /// Kept to move the connection to another socket (a new network) without reconnecting.
    endpoint: quinn::Endpoint,
}

/// The direct tunnel (punched, to Diamond) and the relay (to bronze), held independently:
/// the relay stays up underneath while the direct path comes and goes.
pub(crate) static TUNNEL: Mutex<Option<Tunnel>> = Mutex::new(None);
pub(crate) static RELAY: Mutex<Option<Tunnel>> = Mutex::new(None);

#[derive(Debug)]
struct Pinned(String, Arc<rustls::crypto::CryptoProvider>);
impl ServerCertVerifier for Pinned {
    fn verify_server_cert(&self, end_entity: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        let digest: String = Sha256::digest(end_entity.as_ref()).iter().map(|b| format!("{b:02x}")).collect();
        if digest == self.0 { Ok(ServerCertVerified::assertion()) } else { Err(rustls::Error::General("Not the server's certificate".into())) }
    }
    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.1.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.1.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> { self.1.signature_verification_algorithms.supported_schemes() }
}

fn text(pointer: *const c_char) -> Option<String> {
    if pointer.is_null() { return None }
    unsafe { CStr::from_ptr(pointer) }.to_str().ok().map(str::to_owned)
}

/// Open the tunnel on the punched socket `fd` (ownership passes here) to the server at
/// `server` ("ip:port") whose certificate has SHA-256 `fingerprint`. Returns the loopback
/// port the app connects to, or 0 when QUIC could not be opened. Blocks for up to 8 s.
#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_start(fd: i32, server: *const c_char, fingerprint: *const c_char) -> u16 {
    tbdirect_stop();
    // Keepalives every 5 s and a 45 s idle limit, so a short trip out of the app (iOS stops
    // the keepalives) usually finds the path still open. A path that died meanwhile (a
    // carrier switch behind a hotspot) is caught by the app's public-address check over the
    // relay and by reads retried over the relay, not by this limit.
    open(&TUNNEL, fd, server, fingerprint, b"tbdirect", "torrbaby-direct", 8, (5, 45), APP_WINDOWS)
}

/// As `tbdirect_start`, for the packet tunnel extension: iOS ends an extension that uses more
/// than 50 MB, so the connection buffers less (4 MB still covers 200 Mbit/s at 150 ms).
#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_start_lean(fd: i32, server: *const c_char, fingerprint: *const c_char) -> u16 {
    tbdirect_stop();
    open(&TUNNEL, fd, server, fingerprint, b"tbdirect", "torrbaby-direct", 8, LEAN_TIMING, LEAN_WINDOWS)
}

/// As `tbdirect_relay_start`, with the extension's smaller buffers.
#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_relay_start_lean(fd: i32, server: *const c_char, fingerprint: *const c_char) -> u16 {
    tbdirect_relay_stop();
    open(&RELAY, fd, server, fingerprint, b"tbrelay", "torrbaby-rendezvous", 5, RELAY_TIMING, LEAN_WINDOWS)
}

/// Keepalive and idle limit (s) in the packet tunnel extension, which iOS never suspends: a
/// path silent for 15 s (three keepalives lost) is dead, and its requests fail over at once,
/// not after 45 s. (The app keeps 45 s: its keepalives stop while iOS suspends it.)
pub(crate) const LEAN_TIMING: (u64, u64) = (5, 15);
/// The extension's relay gets longer: a punch's spray can swamp a phone hotspot for its 20 s,
/// and the relay must not be dropped meanwhile (it carries everything while there is no path).
pub(crate) const RELAY_TIMING: (u64, u64) = (5, 30);

/// QUIC receive windows (per stream, per connection).
pub(crate) const APP_WINDOWS: (u32, u32) = (8 << 20, 32 << 20);
const LEAN_WINDOWS: (u32, u32) = (2 << 20, 4 << 20);

/// Open the relay: QUIC from socket `fd` (ownership passes here) to Torrbaby's rendezvous on
/// bronze at `server` ("ip:port"), pinned to `fingerprint`. Each loopback connection becomes
/// a stream that bronze splices to Diamond's standing connection. Returns the loopback port,
/// or 0. Blocks for up to 5 s.
#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_relay_start(fd: i32, server: *const c_char, fingerprint: *const c_char) -> u16 {
    tbdirect_relay_stop();
    open(&RELAY, fd, server, fingerprint, b"tbrelay", "torrbaby-rendezvous", 5, (10, 45), APP_WINDOWS)
}

fn open(slot: &Mutex<Option<Tunnel>>, fd: i32, server: *const c_char, fingerprint: *const c_char, alpn: &[u8], name: &str, seconds: u64, timing: (u64, u64), windows: (u32, u32)) -> u16 {
    let socket = if fd >= 0 { Some(unsafe { std::net::UdpSocket::from_raw_fd(fd) }) } else { None };
    let (Some(socket), Some(server), Some(fingerprint)) = (socket, text(server), text(fingerprint)) else { return 0 };
    let Ok(server): Result<SocketAddr, _> = server.parse() else { return 0 };
    open_on(slot, socket, server, fingerprint, alpn, name, seconds, timing, windows)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn open_on(slot: &Mutex<Option<Tunnel>>, socket: std::net::UdpSocket, server: SocketAddr, fingerprint: String, alpn: &[u8], name: &str, seconds: u64, (keepalive, idle): (u64, u64), (stream_window, window): (u32, u32)) -> u16 {
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() else { return 0 };
    let opened = runtime.block_on(async move {
        socket.set_nonblocking(true).ok()?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        // The direct path is QUIC to Diamond's own identity: once the app has pinned it, that is
        // the certificate required, whatever the signalling (through bronze) said.
        let relayed = alpn == b"tbrelay";
        let fingerprint = if relayed { fingerprint } else { e2e::pinned().unwrap_or(fingerprint) };
        let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13]).ok()?
            .dangerous().with_custom_certificate_verifier(Arc::new(Pinned(fingerprint.to_lowercase(), provider)))
            .with_no_client_auth();
        tls.alpn_protocols = vec![alpn.to_vec()];
        let mut client = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls).ok()?));
        let mut transport = quinn::TransportConfig::default();
        transport.keep_alive_interval(Some(Duration::from_secs(keepalive)));
        transport.max_idle_timeout(Some(Duration::from_secs(idle).try_into().ok()?));
        transport.stream_receive_window(stream_window.into());
        transport.receive_window(window.into());
        client.transport_config(Arc::new(transport));
        let mut endpoint = quinn::Endpoint::new(quinn::EndpointConfig::default(), None, socket, Arc::new(quinn::TokioRuntime)).ok()?;
        endpoint.set_default_client_config(client);
        let connecting = endpoint.connect(server, name).ok()?;
        let connection = tokio::time::timeout(Duration::from_secs(seconds), connecting).await.ok()?.ok()?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.ok()?;
        let port = listener.local_addr().ok()?.port();
        let carrier = connection.clone();
        let kept = endpoint.clone();
        let client = CLIENT.lock().map(|c| c.clone()).unwrap_or_default();
        if relayed && !client.is_empty() {
            let naming = connection.clone();
            tokio::spawn(async move {
                let Ok((mut send, _recv)) = naming.open_bi().await else { return };
                let _ = send.write_all(format!("TBNAME1 {client}\n").as_bytes()).await;
                let _ = send.finish();
            });
        }
        tokio::spawn(async move {
            let _endpoint = endpoint;
            while let Ok((tcp, _)) = listener.accept().await {
                let stream_carrier = carrier.clone();
                tokio::spawn(async move {
                    let Ok((send, recv)) = stream_carrier.open_bi().await else { return };
                    if !relayed { return pipe(tcp, send, recv).await }
                    match e2e::wrap(send, recv).await {
                        e2e::Wrapped::Encrypted(stream) => { let (from, to) = tokio::io::split(stream); pipe(tcp, to, from).await }
                        // Plain only until Diamond's identity is pinned: then closed (crate::e2e).
                        e2e::Wrapped::Plain(send, recv) => { tokio::select! { _ = pipe(tcp, send, recv) => {}, _ = e2e::pinned_now() => {} } }
                        e2e::Wrapped::Failed => {}
                    }
                });
                if carrier.close_reason().is_some() { break }
            }
        });
        Some((connection, port, kept))
    });
    let Some((connection, port, endpoint)) = opened else { runtime.shutdown_background(); return 0 };
    if let Ok(mut held) = slot.lock() { *held = Some(Tunnel { runtime, connection, endpoint }) }
    port
}

async fn pipe<W: tokio::io::AsyncWrite + Unpin, R: tokio::io::AsyncRead + Unpin>(tcp: tokio::net::TcpStream, mut send: W, mut recv: R) {
    let _ = tcp.set_nodelay(true);
    let (mut from_app, mut to_app) = tcp.into_split();
    let up = async { let _ = tokio::io::copy(&mut from_app, &mut send).await; let _ = send.shutdown().await; };
    let down = async { let _ = tokio::io::copy(&mut recv, &mut to_app).await; let _ = to_app.shutdown().await; };
    tokio::join!(up, down);
}

pub(crate) fn alive(slot: &Mutex<Option<Tunnel>>) -> bool {
    slot.lock().ok().and_then(|held| held.as_ref().map(|t| t.connection.close_reason().is_none())).unwrap_or(false)
}

pub(crate) fn stop(slot: &Mutex<Option<Tunnel>>) {
    let tunnel = slot.lock().ok().and_then(|mut held| held.take());
    if let Some(tunnel) = tunnel {
        tunnel.connection.close(0u32.into(), b"done");
        tunnel.runtime.shutdown_background();
    }
}

/// Whether the tunnel is open.
#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_alive() -> bool { alive(&TUNNEL) }

/// Close the tunnel, if any.
#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_stop() { stop(&TUNNEL) }

/// Move the open relay onto socket `fd` (ownership passes here), bound to the new network:
/// QUIC migrates the same connection to the new address, so the relay keeps carrying traffic
/// across a change of network instead of being dropped and opened again (bronze has a public
/// address, so it simply answers the new one). True when it was moved.
#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_relay_rebind(fd: i32) -> bool {
    if fd < 0 { return false }
    relay_rebind(unsafe { std::net::UdpSocket::from_raw_fd(fd) })
}

/// Move the open relay onto `socket` (see `tbdirect_relay_rebind`).
pub(crate) fn relay_rebind(socket: std::net::UdpSocket) -> bool {
    if socket.set_nonblocking(true).is_err() { return false }
    let held = RELAY.lock().ok().and_then(|held| held.as_ref().map(|t| (t.runtime.handle().clone(), t.endpoint.clone(), t.connection.clone())));
    let Some((handle, endpoint, connection)) = held else { return false };
    if connection.close_reason().is_some() { return false }
    let _guard = handle.enter();
    endpoint.rebind(socket).is_ok()
}

/// Whether the relay is open.
#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_relay_alive() -> bool { alive(&RELAY) }

/// Close the relay, if any.
#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_relay_stop() { stop(&RELAY) }

/// Ask bronze, over the open relay, whether Diamond is connected and what public address it
/// sees this device at ("home online\nyou 1.2.3.4:5678\n"). Writes the reply, NUL-terminated,
/// into `buffer`; returns its length, or -1. Blocks for up to 3 s.
#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_relay_info(buffer: *mut c_char, capacity: usize) -> i32 {
    if buffer.is_null() || capacity == 0 { return -1 }
    let Some(reply) = relay_info() else { return -1 };
    let length = reply.len().min(capacity - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(reply.as_ptr() as *const c_char, buffer, length);
        *buffer.add(length) = 0;
    }
    length as i32
}

/// bronze's TBINFO reply over the open relay (see `tbdirect_relay_info`). Blocks up to 3 s;
/// not to be called from inside an async task (use spawn_blocking).
pub(crate) fn relay_info() -> Option<Vec<u8>> {
    let held = RELAY.lock().ok().and_then(|held| held.as_ref().map(|t| (t.runtime.handle().clone(), t.connection.clone())));
    let (handle, connection) = held?;
    handle.block_on(async move {
        tokio::time::timeout(Duration::from_secs(3), async move {
            let (mut send, mut recv) = connection.open_bi().await.ok()?;
            send.write_all(b"TBINFO1\n").await.ok()?;
            send.finish().ok()?;
            recv.read_to_end(256).await.ok()
        }).await.ok().flatten()
    })
}

/// Diamond's address on this network while the device is at home (set by the extension):
/// tunnel connections go straight to it, with no QUIC and nothing through bronze. So the
/// tunnel's address works everywhere, and a background download keeps one address whether
/// the phone is at home or away.
static HOME: Mutex<Option<SocketAddr>> = Mutex::new(None);

/// This device's Torrbaby client id: the relay names its connection with it, so Diamond's
/// Connection page can show which devices use bronze (bronze is told nothing more).
static CLIENT: Mutex<String> = Mutex::new(String::new());

#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_client(id: *const c_char) {
    let id = text(id).unwrap_or_default();
    let changed = CLIENT.lock().map(|mut held| { let changed = *held != id; *held = id.clone(); changed }).unwrap_or(false);
    // A relay already open is told now (a newly opened one names itself as it opens).
    if !changed || id.is_empty() { return }
    let open = RELAY.lock().ok().and_then(|held| held.as_ref().map(|t| (t.runtime.handle().clone(), t.connection.clone())));
    if let Some((handle, connection)) = open {
        handle.spawn(async move {
            let Ok((mut send, _recv)) = connection.open_bi().await else { return };
            let _ = send.write_all(format!("TBNAME1 {id}\n").as_bytes()).await;
            let _ = send.finish();
        });
    }
}

/// Diamond's identity fingerprint, learned from pairing or from Diamond at home: relayed
/// streams are encrypted end to end to it, and the direct path must present it. Empty forgets it.
#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_identity(fingerprint: *const c_char) {
    e2e::pin(&text(fingerprint).unwrap_or_default());
}

/// Connections open now, and connections waiting for a route: the extension lets the paths go
/// when nothing has been open for a while (no keepalives, the radio can sleep), and opens
/// them again when a connection waits.
#[unsafe(no_mangle)]
pub extern "C" fn tbtunnel_open_flows() -> u64 { packet::stats::get(&packet::stats::FLOWS_OPEN) }
#[unsafe(no_mangle)]
pub extern "C" fn tbtunnel_waiting_flows() -> u64 { packet::stats::get(&packet::stats::FLOWS_WAITING) }

/// Diamond on this network ("ip:port"), or empty when away from home.
#[unsafe(no_mangle)]
pub extern "C" fn tbtunnel_home(address: *const c_char) {
    let address = text(address).and_then(|a| a.parse::<SocketAddr>().ok());
    if let Ok(mut held) = HOME.lock() { *held = address }
}

pub(crate) fn open_connection(slot: &Mutex<Option<Tunnel>>) -> Option<quinn::Connection> {
    slot.lock().ok().and_then(|held| held.as_ref().map(|t| t.connection.clone())).filter(|c| c.close_reason().is_none())
}

/// The route a tunnel connection should take: Diamond on this network at home, else the
/// direct path while it is open, else the relay.
pub(crate) fn carrier() -> Option<packet::Route> {
    if let Some(address) = HOME.lock().ok().and_then(|held| *held) { return Some(packet::Route::Lan(address)) }
    open_connection(&TUNNEL).map(packet::Route::Quic).or_else(|| open_connection(&RELAY).map(packet::Route::Relay))
}

/// A connection on the relay makes way once Diamond on this network or the direct path is open.
pub(crate) fn superseded(route: &packet::Route) -> bool {
    matches!(route, packet::Route::Relay(_)) && (HOME.lock().ok().is_some_and(|held| held.is_some()) || open_connection(&TUNNEL).is_some())
}

/// Packets the stack sends back to the extension, in batches: `count` packets laid end to end
/// in `data`, with their lengths in `lengths`. (One call per packet was too slow for video.)
pub type PacketWriter = extern "C" fn(context: *mut std::ffi::c_void, data: *const u8, lengths: *const usize, count: usize);
struct Context(*mut std::ffi::c_void);
unsafe impl Send for Context {}

/// Start the packet tunnel's TCP stack (Network Extension). Each packet it sends goes to
/// `write(context, packet, length)`, from a background thread. A TCP connection that arrives
/// while neither the direct path nor the relay is open waits up to 20 s for one.
#[unsafe(no_mangle)]
pub extern "C" fn tbtunnel_start(mtu: u16, write: PacketWriter, context: *mut std::ffi::c_void) -> bool {
    tbtunnel_stop();
    let Some((stack, mut output)) = packet::Stack::start(mtu, Box::new(carrier), Box::new(superseded), Duration::from_secs(20)) else { return false };
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
/// stack, drops, connections, and each QUIC path's round trip, congestion window and loss.
/// Writes it NUL-terminated into `buffer`; returns its length, or -1.
#[unsafe(no_mangle)]
pub extern "C" fn tbtunnel_stats(buffer: *mut c_char, capacity: usize) -> i32 {
    use packet::stats::*;
    if buffer.is_null() || capacity == 0 { return -1 }
    let mut line = format!(
        "in {} pkts/{} KB, out {} pkts/{} KB, dropped in {}, streams down {} KB up {} KB, flows open {} (opened {}, abandoned {}, uncarried {}, moved to a better route {}, at home {})",
        get(&PACKETS_IN), get(&BYTES_IN) / 1024, get(&PACKETS_OUT), get(&BYTES_OUT) / 1024, get(&DROPPED_IN),
        get(&STREAM_DOWN) / 1024, get(&STREAM_UP) / 1024, get(&FLOWS_OPEN), get(&FLOWS_OPENED), get(&FLOWS_ABANDONED), get(&FLOWS_UNCARRIED), get(&FLOWS_MOVED), get(&FLOWS_LAN));
    line.push_str(&paths_line());
    let bytes = line.as_bytes();
    let length = bytes.len().min(capacity - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, buffer, length);
        *buffer.add(length) = 0;
    }
    length as i32
}

/// Each open QUIC path's round trip, congestion window, loss and bytes ("; direct rtt …").
pub(crate) fn paths_line() -> String {
    let mut line = String::new();
    for (name, slot) in [("direct", &TUNNEL), ("relay", &RELAY)] {
        let connection = slot.lock().ok().and_then(|held| held.as_ref().map(|t| t.connection.clone()));
        if let Some(connection) = connection.filter(|c| c.close_reason().is_none()) {
            let stats = connection.stats();
            line.push_str(&format!("; {name} rtt {} ms, cwnd {} KB, lost {} pkts, sent {} KB, received {} KB",
                stats.path.rtt.as_millis(), stats.path.cwnd / 1024, stats.path.lost_packets,
                stats.udp_tx.bytes / 1024, stats.udp_rx.bytes / 1024));
        }
    }
    line
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

/// Each step of a punch, as JSON (see punch::Report), from the punching thread.
pub type PunchReporter = extern "C" fn(context: *mut std::ffi::c_void, report: *const c_char);

/// Punch a direct path to Diamond and open QUIC on it (see punch.rs). `request` is JSON:
/// {"stun": "ip:3478", "signal": "http://host:port", "token": "…", "interface": "en0",
/// "random_from": ["ip", …], "sockets": 192, "lean": false}. Each step goes to `report`.
/// Blocks up to about 40 s. Returns JSON, to be freed with `tbdirect_free`:
/// {"ok": true, "port": <loopback port>, "summary", "public_address", "quic_ms", "report"} or
/// {"ok": false, "failure", "session", "window_failed_at", "report"}.
#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_punch(request: *const c_char, report: PunchReporter, context: *mut std::ffi::c_void) -> *mut c_char {
    let reply = punch_json(text(request).unwrap_or_default(), |r| {
        if let Ok(line) = std::ffi::CString::new(r.json().to_string()) { report(context, line.as_ptr()) }
    });
    std::ffi::CString::new(reply.to_string()).map(|c| c.into_raw()).unwrap_or(std::ptr::null_mut())
}

/// Free a string `tbdirect_punch` returned.
#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_free(text: *mut c_char) {
    if !text.is_null() { drop(unsafe { std::ffi::CString::from_raw(text) }) }
}

/// The punch behind `tbdirect_punch`, with JSON in and out (Rust callers use it directly).
pub fn punch_json(request: String, mut report: impl FnMut(&punch::Report)) -> serde_json::Value {
    use serde_json::json;
    let request: serde_json::Value = serde_json::from_str(&request).unwrap_or_default();
    let string = |key: &str| request[key].as_str().unwrap_or("").to_owned();
    let plan = punch::Plan {
        stun: string("stun"), stun2: request["stun2"].as_str().map(str::to_owned), signal: string("signal"), token: string("token"),
        interface: request["interface"].as_str().filter(|s| !s.is_empty()).map(str::to_owned),
        random_from: request["random_from"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
        sockets: request["sockets"].as_u64().unwrap_or(192).clamp(1, 512) as usize,
        interval: std::time::Duration::from_millis(request["interval_ms"].as_u64().unwrap_or(200).clamp(50, 2000)),
    };
    let lean = request["lean"].as_bool().unwrap_or(false);
    let mut last = punch::Report::default();
    let outcome = punch::punch(&plan, |r| { last = r.clone(); report(r) });
    match outcome {
        Err(failure) => json!({"ok": false, "failure": failure.message, "session": failure.session, "window_failed_at": failure.window_failed_at, "report": last.json()}),
        Ok(punched) => {
            tbdirect_stop();
            let started = std::time::Instant::now();
            let port = open_on(&TUNNEL, punched.socket, punched.server, punched.fingerprint, b"tbdirect", "torrbaby-direct", 8,
                if lean { LEAN_TIMING } else { (5, 45) }, if lean { LEAN_WINDOWS } else { APP_WINDOWS });
            if port == 0 {
                json!({"ok": false, "failure": "Punched, but QUIC did not open on the punched socket within 8 s", "session": last.session, "window_failed_at": null, "report": last.json()})
            } else {
                json!({"ok": true, "port": port, "summary": punched.summary, "public_address": punched.public_address,
                    "quic_ms": started.elapsed().as_millis() as u64, "report": last.json()})
            }
        }
    }
}
