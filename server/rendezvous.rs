//! Diamond's standing connection to Torrbaby's rendezvous on bronze (rendezvous/ in this
//! repository). Nothing can reach Diamond from outside China Telecom's carrier NAT, so it
//! connects out and stays connected: QUIC keepalives every 10 s hold the NAT's mapping open.
//! The first stream proves the shared secret; after that bronze opens one stream for each
//! stream a device opens through its relay, and each is piped to this server's own HTTP port
//! (direct.rs's `pipe`). Requests through it are ordinary Torrbaby requests carrying the
//! device's own credentials, checked here as always; bronze holds none.
//!
//! Devices learn bronze's address and certificate fingerprint from /v1/ping (and pairing), so
//! they can reach this server when they have no home network and no Tailscale.
use crate::config::RendezvousConfig;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ALPN: &[u8] = b"tbhome";

static STATE: Mutex<Option<Value>> = Mutex::new(None);
/// The standing connection while it is up (for asking bronze about its usage).
static CONNECTION: Mutex<Option<quinn::Connection>> = Mutex::new(None);

/// bronze's account of the relay: the devices on it now (by Torrbaby client id, with the
/// bytes relayed for each) and bytes relayed per day, China time.
pub async fn usage() -> Result<Value> {
    let connection = CONNECTION.lock().ok().and_then(|held| held.clone()).filter(|c| c.close_reason().is_none())
        .context("Not connected to the rendezvous")?;
    let asked = async {
        let (mut send, mut recv) = connection.open_bi().await?;
        send.write_all(b"TBUSAGE1\n").await?;
        send.finish()?;
        let reply = recv.read_to_end(4 << 20).await?;
        anyhow::Ok(serde_json::from_slice::<Value>(&reply)?)
    };
    tokio::time::timeout(Duration::from_secs(10), asked).await.context("The rendezvous did not answer")?
}

fn set(update: impl FnOnce(&mut Value)) {
    if let Ok(mut held) = STATE.lock() {
        let value = held.get_or_insert_with(|| json!({}));
        update(value);
    }
}

fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

/// What devices are told: bronze's address and fingerprint, and whether this server is
/// connected to it now. Null when no rendezvous is configured.
pub fn advertised() -> Value {
    STATE.lock().ok().and_then(|held| held.clone()).map(|state| json!({
        "address": state["address"], "fingerprint": state["fingerprint"], "connected": state["state"] == "connected",
    })).unwrap_or(Value::Null)
}

/// `usage`, with each device's name from `names` (client id → name).
pub async fn usage_named(names: std::collections::HashMap<String, String>) -> Result<Value> {
    let mut usage = usage().await?;
    for device in usage["devices"].as_array_mut().into_iter().flatten() {
        let name = device["client"].as_str().and_then(|c| names.get(c)).cloned();
        device["name"] = json!(name);
    }
    Ok(usage)
}

/// The connection's state for /v1/status: address, state, since, streams, last error.
pub fn status() -> Value { STATE.lock().ok().and_then(|held| held.clone()).unwrap_or(Value::Null) }

/// Keep connected to the rendezvous for as long as the server runs.
pub fn spawn(config: RendezvousConfig, http_port: u16) -> tokio::task::JoinHandle<()> {
    crate::punch::set_own_stun(&config.address);
    set(|s| *s = json!({"address": config.address, "fingerprint": config.fingerprint, "state": "connecting", "since": now(), "streams": 0}));
    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(1);
        loop {
            let started = Instant::now();
            match connect_and_serve(&config, http_port).await {
                Ok(reason) => eprintln!("Rendezvous: closed after {} s ({reason})", started.elapsed().as_secs()),
                Err(error) => {
                    eprintln!("Rendezvous: {error:#}");
                    set(|s| s["last_error"] = json!(format!("{error:#}")));
                }
            }
            set(|s| { s["state"] = json!("connecting"); s["since"] = json!(now()); });
            // A connection that held for a while reconnects at once; repeated failures back off.
            if started.elapsed() > Duration::from_secs(60) { backoff = Duration::from_secs(1) }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    })
}

async fn connect_and_serve(config: &RendezvousConfig, http_port: u16) -> Result<String> {
    let server: SocketAddr = config.address.parse().context("The rendezvous address must be ip:port")?;
    let secret_path = crate::config::expand(&config.secret_file)?;
    let secret = std::fs::read_to_string(&secret_path).with_context(|| format!("Reading {}", secret_path.display()))?.trim().to_owned();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous().with_custom_certificate_verifier(Arc::new(Pinned(config.fingerprint.to_lowercase(), provider)))
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let mut client = quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls)?));
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(10)));
    transport.max_idle_timeout(Some(Duration::from_secs(45).try_into()?));
    // Bronze opens the streams: one per device stream, many at once while video plays.
    transport.max_concurrent_bidi_streams(1024u32.into());
    transport.max_concurrent_uni_streams(0u32.into());
    transport.stream_receive_window((8u32 << 20).into());
    transport.receive_window((32u32 << 20).into());
    transport.send_window(32 << 20);
    // BBR, not Cubic: Cubic fills a bottleneck's queue until packets are lost (on CT 5G the
    // round trip climbed 100 ms → 3 s with almost no loss while video streamed, and the player
    // stalled); BBR paces to the measured bottleneck and keeps the queue short.
    transport.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
    client.transport_config(Arc::new(transport));
    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
    endpoint.set_default_client_config(client);
    let connection = tokio::time::timeout(Duration::from_secs(10), endpoint.connect(server, "torrbaby-rendezvous")?)
        .await.context("No answer from the rendezvous")?.context("Could not connect to the rendezvous")?;
    let proved = async {
        let (mut send, mut recv) = connection.open_bi().await?;
        send.write_all(format!("TBHOME1 {secret}\n").as_bytes()).await?;
        send.finish()?;
        let reply = recv.read_to_end(64).await?;
        if reply != b"OK\n" { bail!("The rendezvous refused the secret") }
        anyhow::Ok(())
    };
    tokio::time::timeout(Duration::from_secs(10), proved).await.context("The rendezvous did not answer the hello")??;
    eprintln!("Rendezvous: connected to {server}");
    set(|s| { s["state"] = json!("connected"); s["since"] = json!(now()); s["last_error"] = Value::Null; });
    if let Ok(mut held) = CONNECTION.lock() { *held = Some(connection.clone()) }
    while let Ok((send, recv)) = connection.accept_bi().await {
        set(|s| s["streams"] = json!(s["streams"].as_u64().unwrap_or(0) + 1));
        tokio::spawn(crate::direct::pipe(send, recv, http_port, "Relay".to_owned()));
    }
    let reason = connection.close_reason().map(|r| r.to_string()).unwrap_or_default();
    endpoint.close(0u32.into(), b"done");
    Ok(reason)
}

#[derive(Debug)]
struct Pinned(String, Arc<rustls::crypto::CryptoProvider>);
impl rustls::client::danger::ServerCertVerifier for Pinned {
    fn verify_server_cert(&self, end_entity: &rustls::pki_types::CertificateDer<'_>, _: &[rustls::pki_types::CertificateDer<'_>], _: &rustls::pki_types::ServerName<'_>, _: &[u8], _: rustls::pki_types::UnixTime) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let digest: String = Sha256::digest(end_entity.as_ref()).iter().map(|b| format!("{b:02x}")).collect();
        if digest == self.0 { Ok(rustls::client::danger::ServerCertVerified::assertion()) } else { Err(rustls::Error::General("Not the rendezvous's certificate".into())) }
    }
    fn verify_tls12_signature(&self, message: &[u8], cert: &rustls::pki_types::CertificateDer<'_>, dss: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.1.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &rustls::pki_types::CertificateDer<'_>, dss: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.1.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> { self.1.signature_verification_algorithms.supported_schemes() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Against a real tbrendezvous: `cargo run` in rendezvous/ with `--state DIR --stun P
    /// --quic Q`, then TB_RENDEZVOUS=127.0.0.1:Q TB_RENDEZVOUS_STATE=DIR cargo test
    /// rendezvous -- --ignored. A device stream through the relay reaches the HTTP port.
    #[tokio::test]
    #[ignore]
    async fn a_device_stream_through_the_relay_reaches_the_http_port() -> Result<()> {
        let address = std::env::var("TB_RENDEZVOUS")?;
        let state = std::path::PathBuf::from(std::env::var("TB_RENDEZVOUS_STATE")?);
        let cert = std::fs::read(state.join("cert.der"))?;
        let fingerprint: String = Sha256::digest(&cert).iter().map(|b| format!("{b:02x}")).collect();
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
        let config = RendezvousConfig { address: address.clone(), fingerprint: fingerprint.clone(), secret_file: state.join("home.secret").display().to_string() };
        spawn(config, http_port);
        for _ in 0..50 {
            if advertised()["connected"] == true { break }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(advertised()["connected"], true, "{}", status());
        // The device side, pinning the same fingerprint, ALPN tbrelay.
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .dangerous().with_custom_certificate_verifier(Arc::new(Pinned(fingerprint, provider))).with_no_client_auth();
        tls.alpn_protocols = vec![b"tbrelay".to_vec()];
        let mut device = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
        device.set_default_client_config(quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls)?)));
        let connection = device.connect(address.parse()?, "torrbaby-rendezvous")?.await?;
        let (mut send, mut recv) = connection.open_bi().await?;
        send.write_all(b"GET /v1/ping HTTP/1.1\r\n\r\n").await?;
        send.finish()?;
        let reply = tokio::time::timeout(Duration::from_secs(5), recv.read_to_end(4096)).await??;
        assert_eq!(String::from_utf8_lossy(&reply), "HTTP/1.1 200 OK\r\n\r\nheard GET /v1/ping HTTP/1.1");
        assert_eq!(status()["streams"], 1);
        Ok(())
    }
}
