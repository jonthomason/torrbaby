//! Torrbaby's own direct connection, for a device away from home when Tailscale can only
//! relay (both ends behind carrier NAT). It grew out of the feasibility test in punch.rs.
//!
//! The device asks over its normal connection (POST /v1/direct) with its public address and
//! the ports a STUN server saw for its sockets. The server opens a UDP socket, learns its own
//! public address, and sprays the device's likely ports so China Telecom's filter lets the
//! device in, while the device's sockets fire at it. The first device packet that arrives is
//! echoed; the device then speaks QUIC from the winning socket's port, and this same socket
//! becomes a QUIC endpoint that accepts that device's address only. Each QUIC stream is piped
//! to the server's own HTTP port, so requests through it are ordinary Torrbaby requests with
//! the device's own credentials. The certificate is the server's own; its fingerprint reaches
//! the device over the normal (trusted) connection. Keepalives hold both NATs open; when the
//! device goes quiet the endpoint closes and the device falls back to Tailscale.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, net::{IpAddr, SocketAddr}, sync::{Arc, Mutex}, time::{Duration, Instant}};
use tokio::{io::AsyncWriteExt, net::{TcpStream, UdpSocket}};

const SPRAY_SECONDS: u64 = 25;
const MAX_WINDOW: u32 = 8192;
const ALPN: &[u8] = b"tbdirect";

pub static DIRECT: std::sync::LazyLock<Direct> = std::sync::LazyLock::new(Direct::default);

struct Identity { cert: rustls::pki_types::CertificateDer<'static>, key: Vec<u8>, fingerprint: String }

#[derive(Default)]
pub struct Direct {
    identity: Mutex<Option<Arc<Identity>>>,
    sessions: Mutex<HashMap<String, Arc<Mutex<Value>>>>,
}

impl Direct {
    fn identity(&self) -> Result<Arc<Identity>> {
        let mut held = self.identity.lock().map_err(|_| anyhow::anyhow!("Direct connection unavailable"))?;
        if let Some(identity) = held.as_ref() { return Ok(identity.clone()) }
        let made = rcgen::generate_simple_self_signed(vec!["torrbaby-direct".to_owned()])?;
        let cert = made.cert.der().clone();
        let fingerprint = Sha256::digest(cert.as_ref()).iter().map(|b| format!("{b:02x}")).collect();
        let identity = Arc::new(Identity { cert, key: made.key_pair.serialize_der(), fingerprint });
        *held = Some(identity.clone());
        Ok(identity)
    }

    /// Start a direct connection for the device at `ip`, spraying ports `low..=high`.
    pub async fn start(&self, body: &Value, http_port: u16) -> Result<Value> {
        let ip: IpAddr = body["ip"].as_str().context("ip is required")?.parse().context("ip must be an address")?;
        let low = body["low"].as_u64().context("low is required")? as u32;
        let high = body["high"].as_u64().context("high is required")? as u32;
        ensure!((1..=65535).contains(&low) && (low..=65535).contains(&high), "ports must be 1–65535, low ≤ high");
        let high = high.min(low + MAX_WINDOW - 1);
        let identity = self.identity()?;
        let std_socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
        std_socket.set_nonblocking(true)?;
        let socket = UdpSocket::from_std(std_socket)?;
        let public = crate::punch::stun(&socket).await.context("Diamond could not learn its public address")?;
        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_owned();
        let state = Arc::new(Mutex::new(json!({
            "id": id, "state": "punching", "server": public.to_string(), "device_ip": ip.to_string(),
            "low": low, "high": high, "device": null, "punched_after_ms": null, "streams": 0,
        })));
        {
            let mut sessions = self.sessions.lock().map_err(|_| anyhow::anyhow!("Direct connection unavailable"))?;
            if sessions.len() > 20 { sessions.retain(|_, s| s.lock().map(|s| s["state"] == "open").unwrap_or(false)); }
            sessions.insert(id.clone(), state.clone());
        }
        let fingerprint = identity.fingerprint.clone();
        let session = id.clone();
        tokio::spawn(async move {
            if let Err(error) = run(socket, ip, low, high, session, identity, http_port, state.clone()).await {
                if let Ok(mut s) = state.lock() { s["state"] = json!("failed"); s["error"] = json!(error.to_string()); }
            }
        });
        Ok(json!({"id": id, "server": public.to_string(), "fingerprint": fingerprint, "low": low, "high": high}))
    }

    pub fn status(&self, id: &str) -> Result<Value> {
        let sessions = self.sessions.lock().map_err(|_| anyhow::anyhow!("Direct connection unavailable"))?;
        Ok(sessions.get(id).context("No such direct connection")?.lock().map_err(|_| anyhow::anyhow!("Direct connection unavailable"))?.clone())
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(socket: UdpSocket, ip: IpAddr, low: u32, high: u32, id: String, identity: Arc<Identity>, http_port: u16, state: Arc<Mutex<Value>>) -> Result<()> {
    // Punch: open China Telecom's filter for the device's likely ports until one gets through.
    let started = Instant::now();
    let spray = format!("TBDIRECT1 {id} spray").into_bytes();
    let prefix = format!("TBDIRECT1 {id} ");
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut buffer = [0u8; 1500];
    let device = loop {
        ensure!(started.elapsed() < Duration::from_secs(SPRAY_SECONDS), "No packet from the device got through");
        tokio::select! {
            _ = tick.tick() => {
                for port in low..=high { let _ = socket.send_to(&spray, SocketAddr::new(ip, port as u16)).await; }
            }
            received = socket.recv_from(&mut buffer) => {
                let Ok((len, from)) = received else { continue };
                if from.ip() != ip { continue }
                let Ok(text) = std::str::from_utf8(&buffer[..len]) else { continue };
                if !text.starts_with(&prefix) { continue }
                // Echo a few times: the device switches this socket's port to QUIC on the first.
                for _ in 0..3 { let _ = socket.send_to(format!("TBACK {text}").as_bytes(), from).await; }
                break from;
            }
        }
    };
    // Watch briefly for the device's QUIC (anything that is not a test packet), so the log
    // shows where it comes from; QUIC resends its first packet, so handing over loses nothing.
    let watch_until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < watch_until {
        tokio::select! {
            _ = tick.tick() => {
                for port in low..=high { let _ = socket.send_to(&spray, SocketAddr::new(ip, port as u16)).await; }
            }
            received = tokio::time::timeout(watch_until.saturating_duration_since(Instant::now()), socket.recv_from(&mut buffer)) => {
                let Ok(Ok((len, from))) = received else { break };
                if buffer[..len].starts_with(b"TBDIRECT1") { continue }
                eprintln!("Direct {id}: first non-test packet from {from} ({len} bytes{}) after {} ms",
                    if from.ip() == ip { "" } else { ", another address" }, started.elapsed().as_millis());
                if let Ok(mut s) = state.lock() { s["first_quic_from"] = json!(from.to_string()) }
                break;
            }
        }
    }
    if let Ok(mut s) = state.lock() {
        s["state"] = json!("punched");
        s["device"] = json!(device.to_string());
        s["punched_after_ms"] = json!(started.elapsed().as_millis() as u64);
    }
    // The same socket, and so the same hole, becomes a QUIC endpoint for that device only.
    let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(vec![identity.cert.clone()], rustls::pki_types::PrivateKeyDer::Pkcs8(identity.key.clone().into()))?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let mut server = quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls)?));
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(10)));
    transport.max_idle_timeout(Some(Duration::from_secs(45).try_into()?));
    transport.max_concurrent_bidi_streams(256u32.into());
    server.transport_config(Arc::new(transport));
    let std_socket = socket.into_std()?;
    // Keep spraying until QUIC connects: iOS may start QUIC from a fresh port rather than the
    // punched one, and the device's NAT gives each new destination port the next port along,
    // so the window must stay open for it. A duplicate handle sends while QUIC owns the socket.
    let sprayer = UdpSocket::from_std(std_socket.try_clone()?)?;
    let connected = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let spraying = connected.clone();
    let spray_until = Instant::now() + Duration::from_secs(20);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        while !spraying.load(std::sync::atomic::Ordering::Relaxed) && Instant::now() < spray_until {
            tick.tick().await;
            for port in low..=high { let _ = sprayer.send_to(&spray, SocketAddr::new(ip, port as u16)).await; }
        }
    });
    let endpoint = quinn::Endpoint::new(quinn::EndpointConfig::default(), Some(server), std_socket, Arc::new(quinn::TokioRuntime))?;
    if let Ok(mut s) = state.lock() { s["state"] = json!("waiting") }
    eprintln!("Direct {id}: punched from {device} after {} ms; waiting for QUIC", started.elapsed().as_millis());
    // One device: the first connection from its address; then serve until it goes quiet.
    let connection = loop {
        let Ok(accepted) = tokio::time::timeout(Duration::from_secs(20), endpoint.accept()).await else {
            connected.store(true, std::sync::atomic::Ordering::Relaxed);
            eprintln!("Direct {id}: no QUIC from the device in 20 s");
            anyhow::bail!("The device did not start QUIC");
        };
        let incoming = accepted.context("Endpoint closed")?;
        let from = incoming.remote_address();
        if from.ip() != ip { incoming.refuse(); continue }
        match incoming.await {
            Ok(connection) => { eprintln!("Direct {id}: QUIC open from {from}"); break connection }
            Err(error) => { eprintln!("Direct {id}: QUIC from {from} failed: {error}"); continue }
        }
    };
    connected.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Ok(mut s) = state.lock() { s["state"] = json!("open"); s["opened_after_ms"] = json!(started.elapsed().as_millis() as u64); }
    while let Ok((send, recv)) = connection.accept_bi().await {
        if let Ok(mut s) = state.lock() { s["streams"] = json!(s["streams"].as_u64().unwrap_or(0) + 1) }
        tokio::spawn(pipe(send, recv, http_port));
    }
    endpoint.close(0u32.into(), b"done");
    eprintln!("Direct {id}: closed after {} s", started.elapsed().as_secs());
    if let Ok(mut s) = state.lock() { s["state"] = json!("closed"); s["closed_after_ms"] = json!(started.elapsed().as_millis() as u64); }
    Ok(())
}

/// One stream is one connection to the server's own HTTP port, both ways.
async fn pipe(mut send: quinn::SendStream, mut recv: quinn::RecvStream, http_port: u16) {
    let Ok(tcp) = TcpStream::connect(("127.0.0.1", http_port)).await else { let _ = send.reset(1u32.into()); return };
    let _ = tcp.set_nodelay(true);
    let (mut from_server, mut to_server) = tcp.into_split();
    let up = async { let _ = tokio::io::copy(&mut recv, &mut to_server).await; let _ = to_server.shutdown().await; };
    let down = async { let _ = tokio::io::copy(&mut from_server, &mut send).await; let _ = send.finish(); };
    tokio::join!(up, down);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A device punches, then speaks QUIC from the same port, and a stream reaches the
    /// server's HTTP port and back.
    #[tokio::test]
    async fn a_punched_socket_carries_streams_to_the_http_port() -> Result<()> {
        // A stand-in for the server's HTTP port.
        let http = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let http_port = http.local_addr()?.port();
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = http.accept().await {
                tokio::spawn(async move {
                    let mut request = [0u8; 256];
                    let n = tcp.read(&mut request).await.unwrap_or(0);
                    let _ = tcp.write_all(format!("HTTP/1.1 200 OK\r\n\r\nheard {}", String::from_utf8_lossy(&request[..n]).lines().next().unwrap_or("")).as_bytes()).await;
                });
            }
        });
        let direct = Direct::default();
        let identity = direct.identity()?;
        let server = UdpSocket::bind("127.0.0.1:0").await?;
        let server_address = server.local_addr()?;
        // The device's socket; the server sprays a window that holds its port.
        let device = UdpSocket::bind("127.0.0.1:0").await?;
        let device_port = device.local_addr()?.port() as u32;
        let state = Arc::new(Mutex::new(json!({})));
        let run_state = state.clone();
        let run_identity = identity.clone();
        tokio::spawn(async move {
            run(server, "127.0.0.1".parse().unwrap(), device_port.saturating_sub(5).max(1), device_port + 5, "t".into(), run_identity, http_port, run_state).await
        });
        device.send_to(b"TBDIRECT1 t 0 hello", server_address).await?;
        let mut buffer = [0u8; 256];
        loop {
            let (n, _) = tokio::time::timeout(Duration::from_secs(5), device.recv_from(&mut buffer)).await??;
            if buffer[..n].starts_with(b"TBACK") { break }
        }
        // QUIC from the same port, trusting the fingerprinted certificate.
        let device = device.into_std()?;
        let mut roots = rustls::RootCertStore::empty();
        roots.add(identity.cert.clone())?;
        let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])?.with_root_certificates(roots).with_no_client_auth();
        tls.alpn_protocols = vec![ALPN.to_vec()];
        let mut client = quinn::Endpoint::new(quinn::EndpointConfig::default(), None, device, Arc::new(quinn::TokioRuntime))?;
        client.set_default_client_config(quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls)?)));
        let connection = tokio::time::timeout(Duration::from_secs(5), client.connect(server_address, "torrbaby-direct")?.into_future()).await??;
        let (mut send, mut recv) = connection.open_bi().await?;
        send.write_all(b"GET /v1/ping HTTP/1.1\r\n\r\n").await?;
        send.finish()?;
        let reply = tokio::time::timeout(Duration::from_secs(5), recv.read_to_end(4096)).await??;
        assert_eq!(String::from_utf8_lossy(&reply), "HTTP/1.1 200 OK\r\n\r\nheard GET /v1/ping HTTP/1.1");
        assert_eq!(state.lock().unwrap()["state"], "open");
        Ok(())
    }
}
