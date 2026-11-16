//! Check another home against a real tbrendezvous: connect as that home over QUIC (its secret
//! from a file), answer each stream with "heard by <name>", then as a device reach it by name.
//!   cargo run --example home_probe -- 47.112.18.69:7443 c0fcde96…(fingerprint) NAME SECRET_FILE
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug)]
struct Pin(String, Arc<rustls::crypto::CryptoProvider>);
impl rustls::client::danger::ServerCertVerifier for Pin {
    fn verify_server_cert(&self, cert: &rustls::pki_types::CertificateDer<'_>, _: &[rustls::pki_types::CertificateDer<'_>], _: &rustls::pki_types::ServerName<'_>, _: &[u8], _: rustls::pki_types::UnixTime) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if tbrendezvous::fingerprint(cert) == self.0 { Ok(rustls::client::danger::ServerCertVerified::assertion()) } else { Err(rustls::Error::General("not bronze".into())) }
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &rustls::pki_types::CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> { rustls::crypto::verify_tls12_signature(m, c, d, &self.1.signature_verification_algorithms) }
    fn verify_tls13_signature(&self, m: &[u8], c: &rustls::pki_types::CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> { rustls::crypto::verify_tls13_signature(m, c, d, &self.1.signature_verification_algorithms) }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> { self.1.signature_verification_algorithms.supported_schemes() }
}

async fn connect(server: std::net::SocketAddr, fingerprint: &str, alpn: &[u8]) -> quinn::Connection {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone()).with_protocol_versions(&[&rustls::version::TLS13]).unwrap()
        .dangerous().with_custom_certificate_verifier(Arc::new(Pin(fingerprint.to_owned(), provider))).with_no_client_auth();
    tls.alpn_protocols = vec![alpn.to_vec()];
    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap()).unwrap();
    endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap())));
    endpoint.connect(server, "torrbaby-rendezvous").unwrap().await.unwrap()
}

async fn ask(device: &quinn::Connection, request: &[u8]) -> String {
    let (mut send, mut recv) = device.open_bi().await.unwrap();
    send.write_all(request).await.unwrap();
    send.finish().unwrap();
    match tokio::time::timeout(Duration::from_secs(10), recv.read_to_end(1 << 20)).await {
        Ok(Ok(reply)) => String::from_utf8_lossy(&reply).into_owned(),
        Ok(Err(error)) => format!("(refused: {error})"),
        Err(_) => "(timed out)".into(),
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let server = args[0].parse().unwrap();
    let (fingerprint, name) = (args[1].clone(), args[2].clone());
    let secret = std::fs::read_to_string(&args[3]).unwrap().trim().to_owned();
    let home = connect(server, &fingerprint, tbrendezvous::ALPN_HOME).await;
    let (mut send, mut recv) = home.open_bi().await.unwrap();
    send.write_all(format!("{}{secret}\n", tbrendezvous::HOME_HELLO).as_bytes()).await.unwrap();
    println!("hello: {:?}", String::from_utf8_lossy(&recv.read_to_end(16).await.unwrap()));
    let serving = home.clone();
    let tag = name.clone();
    tokio::spawn(async move {
        while let Ok((mut send, mut recv)) = serving.accept_bi().await {
            let tag = tag.clone();
            tokio::spawn(async move {
                let request = recv.read_to_end(1 << 20).await.unwrap_or_default();
                let line = String::from_utf8_lossy(&request).lines().next().unwrap_or("").to_owned();
                let _ = send.write_all(format!("heard by {tag}: {line}").as_bytes()).await;
                let _ = send.finish();
            });
        }
    });
    let device = connect(server, &fingerprint, tbrendezvous::ALPN_RELAY).await;
    println!("info: {:?}", ask(&device, format!("TBTO1 {name}\nTBINFO1\n").as_bytes()).await);
    for i in 0..3 {
        let started = Instant::now();
        let reply = ask(&device, format!("TBTO1 {name}\nGET /probe/{i} HTTP/1.1\r\n\r\n").as_bytes()).await;
        println!("stream {i} in {} ms: {reply}", started.elapsed().as_millis());
    }
    println!("diamond info: {:?}", ask(&device, b"TBINFO1\n").await.lines().next());
    println!("unknown home: {}", ask(&device, b"TBTO1 nobody\nGET / HTTP/1.1\r\n\r\n").await);
}
