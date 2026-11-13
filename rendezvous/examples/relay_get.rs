//! Diagnosis: a device's request through the relay, as a phone would make it.
//!   cargo run --example relay_get -- 47.112.18.69:7443 <fingerprint> /v1/ping [bearer-token]
//! With path "info" it asks bronze itself whether Diamond is connected.
use std::sync::Arc;

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
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous().with_custom_certificate_verifier(Arc::new(Pinned(fingerprint, provider))).with_no_client_auth();
    tls.alpn_protocols = vec![tbrendezvous::ALPN_RELAY.to_vec()];
    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
    endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls)?)));
    let started = std::time::Instant::now();
    let connection = endpoint.connect(server, "torrbaby-rendezvous")?.await?;
    let connected = started.elapsed();
    let (mut send, mut recv) = connection.open_bi().await?;
    let request = if path == "info" { String::from_utf8(tbrendezvous::INFO.to_vec())? } else {
        let auth = args.get(4).map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
        format!("GET {path} HTTP/1.1\r\nHost: diamond\r\n{auth}Connection: close\r\n\r\n")
    };
    send.write_all(request.as_bytes()).await?;
    let reply = recv.read_to_end(1 << 20).await?;
    eprintln!("connected in {} ms, answered in {} ms", connected.as_millis(), started.elapsed().as_millis());
    println!("{}", String::from_utf8_lossy(&reply));
    connection.close(0u32.into(), b"done");
    Ok(())
}
