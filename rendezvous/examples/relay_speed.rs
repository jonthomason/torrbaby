//! Diagnosis: how fast the relay carries a download, as a phone would fetch video through it.
//!   TB_TOKEN=… cargo run --release --example relay_speed -- 47.112.18.69:7443 <fingerprint> <path> [megabytes]
//! Fetches the first `megabytes` (default 30) of `path` with the phone extension's receive
//! windows and prints the rate each second and overall.
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug)]
struct Pinned(String, Arc<rustls::crypto::CryptoProvider>);
impl rustls::client::danger::ServerCertVerifier for Pinned {
    fn verify_server_cert(&self, cert: &rustls::pki_types::CertificateDer<'_>, _: &[rustls::pki_types::CertificateDer<'_>], _: &rustls::pki_types::ServerName<'_>, _: &[u8], _: rustls::pki_types::UnixTime) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if tbrendezvous::fingerprint(cert) == self.0 { Ok(rustls::client::danger::ServerCertVerified::assertion()) } else { Err(rustls::Error::General("fingerprint mismatch".into())) }
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &rustls::pki_types::CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.1.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &rustls::pki_types::CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.1.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> { self.1.signature_verification_algorithms.supported_schemes() }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (server, fingerprint, path) = (args[1].parse()?, args[2].clone(), args[3].clone());
    let megabytes: u64 = args.get(4).and_then(|m| m.parse().ok()).unwrap_or(30);
    let token = std::env::var("TB_TOKEN").unwrap_or_default();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous().with_custom_certificate_verifier(Arc::new(Pinned(fingerprint, provider))).with_no_client_auth();
    tls.alpn_protocols = vec![tbrendezvous::ALPN_RELAY.to_vec()];
    let mut client = quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls)?));
    // The phone extension's windows (Apple/Direct LEAN_WINDOWS).
    let mut transport = quinn::TransportConfig::default();
    transport.stream_receive_window((2u32 << 20).into());
    transport.receive_window((4u32 << 20).into());
    client.transport_config(Arc::new(transport));
    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
    endpoint.set_default_client_config(client);
    let connection = endpoint.connect(server, "torrbaby-rendezvous")?.await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: diamond\r\nAuthorization: Bearer {token}\r\nRange: bytes=0-{}\r\nConnection: close\r\n\r\n", megabytes * 1_000_000 - 1);
    send.write_all(request.as_bytes()).await?;
    let started = Instant::now();
    let (mut total, mut second, mut mark) = (0u64, 0u64, Instant::now());
    let mut buffer = vec![0u8; 64 * 1024];
    while let Some(n) = recv.read(&mut buffer).await? {
        if total == 0 { eprintln!("first bytes after {} ms: {}", started.elapsed().as_millis(), String::from_utf8_lossy(&buffer[..n.min(12)])) }
        total += n as u64; second += n as u64;
        if mark.elapsed() >= Duration::from_secs(1) {
            let stats = connection.stats();
            eprintln!("{:>5.1} s {:>6.1} Mbit/s  rtt {} ms, lost {}", started.elapsed().as_secs_f64(), second as f64 * 8.0 / mark.elapsed().as_secs_f64() / 1e6, stats.path.rtt.as_millis(), stats.path.lost_packets);
            second = 0; mark = Instant::now();
        }
    }
    let seconds = started.elapsed().as_secs_f64();
    eprintln!("{:.1} MB in {:.1} s: {:.1} Mbit/s", total as f64 / 1e6, seconds, total as f64 * 8.0 / seconds / 1e6);
    connection.close(0u32.into(), b"done");
    Ok(())
}
