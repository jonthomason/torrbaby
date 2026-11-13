//! The whole path on loopback: a stand-in Diamond holds its connection, a device opens
//! streams through the relay, and STUN reports the sender's address.
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tbrendezvous::*;

#[derive(Debug)]
struct Trust(Arc<rustls::crypto::CryptoProvider>);
impl rustls::client::danger::ServerCertVerifier for Trust {
    fn verify_server_cert(&self, _: &rustls::pki_types::CertificateDer<'_>, _: &[rustls::pki_types::CertificateDer<'_>], _: &rustls::pki_types::ServerName<'_>, _: &[u8], _: rustls::pki_types::UnixTime) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &rustls::pki_types::CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &rustls::pki_types::CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> { self.0.signature_verification_algorithms.supported_schemes() }
}

async fn connect(server: SocketAddr, alpn: &[u8]) -> quinn::Connection {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13]).unwrap()
        .dangerous().with_custom_certificate_verifier(Arc::new(Trust(provider))).with_no_client_auth();
    tls.alpn_protocols = vec![alpn.to_vec()];
    let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap())));
    endpoint.connect(server, "torrbaby-rendezvous").unwrap().await.unwrap()
}

/// A stand-in Diamond: proves the secret, then answers each stream with "heard <line>".
async fn home(server: SocketAddr, secret: &str) -> Result<quinn::Connection, String> {
    let connection = connect(server, ALPN_HOME).await;
    let (mut send, mut recv) = connection.open_bi().await.unwrap();
    send.write_all(format!("{HOME_HELLO}{secret}\n").as_bytes()).await.unwrap();
    let reply = recv.read_to_end(16).await.map_err(|e| e.to_string())?;
    if reply != b"OK\n" { return Err(String::from_utf8_lossy(&reply).into()) }
    let serving = connection.clone();
    tokio::spawn(async move {
        while let Ok((mut send, mut recv)) = serving.accept_bi().await {
            tokio::spawn(async move {
                let request = recv.read_to_end(1 << 20).await.unwrap();
                let line = String::from_utf8_lossy(&request).lines().next().unwrap_or("").to_owned();
                send.write_all(format!("heard {line} ({} bytes)", request.len()).as_bytes()).await.unwrap();
                send.finish().unwrap();
            });
        }
    });
    Ok(connection)
}

async fn ask(device: &quinn::Connection, request: &[u8]) -> Result<String, String> {
    let (mut send, mut recv) = device.open_bi().await.unwrap();
    send.write_all(request).await.unwrap();
    send.finish().unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(10), recv.read_to_end(1 << 20)).await.map_err(|_| "timed out".to_owned())?;
    reply.map(|r| String::from_utf8_lossy(&r).into_owned()).map_err(|e| e.to_string())
}

fn start() -> (SocketAddr, String) {
    let dir = std::env::temp_dir().join(format!("tbrendezvous-test-{}", std::process::id() as u64 * 1000 + NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let identity = Identity::load_or_create(&dir).unwrap();
    let again = Identity::load_or_create(&dir).unwrap();
    assert_eq!(identity.fingerprint, again.fingerprint, "the certificate is kept");
    let secret = load_or_create_secret(&dir).unwrap();
    assert_eq!(secret, load_or_create_secret(&dir).unwrap(), "the secret is kept");
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    let endpoint = endpoint(socket, &identity).unwrap();
    tokio::spawn(serve(endpoint, Hub::new(&secret)));
    (address, secret)
}

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[tokio::test]
async fn device_streams_reach_diamond_and_back() {
    let (server, secret) = start();
    let device = connect(server, ALPN_RELAY).await;
    assert_eq!(ask(&device, INFO).await.unwrap().lines().next().unwrap(), "home offline");
    let _home = home(server, &secret).await.unwrap();
    assert_eq!(ask(&device, INFO).await.unwrap().lines().next().unwrap(), "home online");
    assert_eq!(ask(&device, b"GET /v1/ping?sig=x HTTP/1.1\r\n\r\n").await.unwrap(), "heard GET /v1/ping?sig=x HTTP/1.1 (31 bytes)");
    // A large body, in several reads, arrives whole; streams run side by side.
    let big = [b"POST /v1/x HTTP/1.1\r\n\r\n".as_slice(), &vec![b'a'; 300_000]].concat();
    let (a, b) = tokio::join!(ask(&device, &big), ask(&device, b"GET /v1/status HTTP/1.1\r\n\r\n"));
    assert_eq!(a.unwrap(), format!("heard POST /v1/x HTTP/1.1 ({} bytes)", big.len()));
    assert!(b.unwrap().starts_with("heard GET /v1/status"));
}

#[tokio::test]
async fn a_wrong_secret_is_refused_and_streams_then_fail() {
    let (server, _) = start();
    assert!(home(server, "not-the-secret-not-the-secret-not-the-secret").await.is_err());
    let device = connect(server, ALPN_RELAY).await;
    let refused = ask(&device, b"GET /v1/ping HTTP/1.1\r\n\r\n").await.unwrap_err();
    assert!(refused.contains("reset") || refused.contains('2'), "{refused}");
}

#[tokio::test]
async fn a_reconnecting_diamond_replaces_the_old_connection() {
    let (server, secret) = start();
    let first = home(server, &secret).await.unwrap();
    let _second = home(server, &secret).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), first.closed()).await.unwrap();
    let device = connect(server, ALPN_RELAY).await;
    assert!(ask(&device, b"GET / HTTP/1.1\r\n\r\n").await.unwrap().starts_with("heard GET /"));
}

#[tokio::test]
async fn stun_reports_the_senders_address() {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    tokio::spawn(stun::serve(socket));
    let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut request = vec![0u8; 20];
    request[0..2].copy_from_slice(&1u16.to_be_bytes());
    request[4..8].copy_from_slice(&0x2112A442u32.to_be_bytes());
    request[8..20].copy_from_slice(b"abcdefghijkl");
    client.send_to(&request, address).await.unwrap();
    let mut buffer = [0u8; 256];
    let (n, _) = tokio::time::timeout(Duration::from_secs(5), client.recv_from(&mut buffer)).await.unwrap().unwrap();
    let reply = &buffer[..n];
    assert_eq!(&reply[0..2], &[0x01, 0x01]);
    assert_eq!(&reply[8..20], b"abcdefghijkl");
    // XOR-MAPPED-ADDRESS first, decoded as server/punch.rs does.
    assert_eq!(&reply[20..22], &[0x00, 0x20]);
    let port = u16::from_be_bytes([reply[26], reply[27]]) ^ 0x2112;
    let ip = u32::from_be_bytes(reply[28..32].try_into().unwrap()) ^ 0x2112A442;
    assert_eq!(SocketAddr::new(std::net::Ipv4Addr::from(ip).into(), port), client.local_addr().unwrap());
    // Not a Binding request: no answer.
    assert!(stun::answer(b"hello there, this is not stun", address).is_none());
    let mut wrong_length = request.clone();
    wrong_length[3] = 4;
    assert!(stun::answer(&wrong_length, address).is_none());
}
