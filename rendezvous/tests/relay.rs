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
async fn home(server: SocketAddr, secret: &str) -> Result<quinn::Connection, String> { tagged_home(server, secret, "").await }

/// As `home`, answering "heard<tag> <line>" (another home).
async fn tagged_home(server: SocketAddr, secret: &str, tag: &'static str) -> Result<quinn::Connection, String> {
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
                send.write_all(format!("heard{tag} {line} ({} bytes)", request.len()).as_bytes()).await.unwrap();
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

fn start() -> (SocketAddr, String) { let (address, secret, _) = start_in(); (address, secret) }

fn start_in() -> (SocketAddr, String, std::path::PathBuf) {
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
    let hub = Hub::new(&secret);
    hub.load_homes(&dir);
    tokio::spawn(serve(endpoint, hub.clone()));
    let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    tcp.set_nonblocking(true).unwrap();
    TCP.lock().unwrap().insert(address, tcp.local_addr().unwrap());
    tokio::spawn(serve_tcp(tokio::net::TcpListener::from_std(tcp).unwrap(), hub));
    (address, secret, dir)
}

/// Each test's relay over TCP, by its QUIC address.
static TCP: std::sync::Mutex<std::collections::BTreeMap<SocketAddr, SocketAddr>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

/// One stream over the relay over TCP, as bronze's web server hands it over: the upgrade
/// request (with the headers Caddy adds), then `request`, then the reply to the end.
async fn ask_tcp(server: SocketAddr, path: &str, request: &[u8]) -> (String, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let tcp = *TCP.lock().unwrap().get(&server).unwrap();
    let mut stream = tokio::net::TcpStream::connect(tcp).await.unwrap();
    stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: 47.112.18.69\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nX-Torrbaby-Client: 9c1d-phone\r\nX-Forwarded-For: 203.0.113.7\r\n\r\n").as_bytes()).await.unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") { head.push(stream.read_u8().await.unwrap()) }
    stream.write_all(request).await.unwrap();
    stream.shutdown().await.unwrap();
    let mut reply = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut reply)).await.unwrap().unwrap();
    (String::from_utf8_lossy(&head).lines().next().unwrap_or("").to_owned(), String::from_utf8_lossy(&reply).into_owned())
}

#[tokio::test]
async fn the_relay_over_tcp_carries_streams_like_quic() {
    let (server, secret) = start();
    let (status, reply) = ask_tcp(server, "/tbrelay", INFO).await;
    assert_eq!(status, "HTTP/1.1 101 Switching Protocols");
    assert!(reply.starts_with("home offline\nyou 203.0.113.7:0\n"), "{reply}");
    let (_, refused) = ask_tcp(server, "/tbrelay", b"GET /v1/ping HTTP/1.1\r\n\r\n").await;
    assert_eq!(refused, "", "closed while Diamond is not connected");
    let diamond = home(server, &secret).await.unwrap();
    assert!(ask_tcp(server, "/tbrelay", INFO).await.1.starts_with("home online\n"));
    let big = [b"POST /v1/x HTTP/1.1\r\n\r\n".as_slice(), &vec![b'a'; 300_000]].concat();
    let ((_, a), (_, b)) = tokio::join!(ask_tcp(server, "/tbrelay", &big), ask_tcp(server, "/tbrelay", b"GET /v1/status?sig=x HTTP/1.1\r\n\r\n"));
    assert_eq!(a, format!("heard POST /v1/x HTTP/1.1 ({} bytes)", big.len()));
    assert!(b.starts_with("heard GET /v1/status?sig=x"));
    // Another path is not the relay.
    assert_eq!(ask_tcp(server, "/other", b"").await.0, "HTTP/1.1 404 Not Found");
    // The streams are counted for the device while open, and in the day's total after.
    let (mut send, mut recv) = diamond.open_bi().await.unwrap();
    send.write_all(tbrendezvous::USAGE).await.unwrap();
    send.finish().unwrap();
    let usage: serde_json::Value = serde_json::from_slice(&recv.read_to_end(1 << 20).await.unwrap()).unwrap();
    assert!(usage["devices"].as_array().unwrap().is_empty(), "no stream open now: {usage}");
    let today = usage["today"].as_str().unwrap();
    assert!(usage["days"][today]["up"].as_u64().unwrap() >= big.len() as u64, "{usage}");
}

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[tokio::test]
async fn device_streams_reach_diamond_and_back() {
    let (server, secret) = start();
    let device = connect(server, ALPN_RELAY).await;
    assert_eq!(ask(&device, INFO).await.unwrap().lines().next().unwrap(), "home offline");
    let _home = home(server, &secret).await.unwrap();
    assert_eq!(ask(&device, INFO).await.unwrap().lines().next().unwrap(), "home online");
    // Diamond's public IP, for a device to tell whether it is behind the same NAT (at home).
    assert!(ask(&device, INFO).await.unwrap().lines().any(|l| l == "homeip 127.0.0.1"));
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

/// Diamond's Connection page: the devices on the relay (by Torrbaby client id) and the bytes
/// relayed, per device and per day, asked over Diamond's own connection.
#[tokio::test]
async fn diamond_sees_who_uses_the_relay_and_how_much() {
    let (server, secret) = start();
    let diamond = home(server, &secret).await.unwrap();
    let device = connect(server, ALPN_RELAY).await;
    ask(&device, b"TBNAME1 3f2a-client\n").await.unwrap();
    let big = [b"POST /v1/x HTTP/1.1\r\n\r\n".as_slice(), &vec![b'a'; 300_000]].concat();
    ask(&device, &big).await.unwrap();
    let (mut send, mut recv) = diamond.open_bi().await.unwrap();
    send.write_all(tbrendezvous::USAGE).await.unwrap();
    send.finish().unwrap();
    let usage: serde_json::Value = serde_json::from_slice(&recv.read_to_end(1 << 20).await.unwrap()).unwrap();
    let devices = usage["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0]["client"], "3f2a-client");
    assert!(devices[0]["up"].as_u64().unwrap() >= big.len() as u64, "{usage}");
    assert!(devices[0]["down"].as_u64().unwrap() > 0);
    let today = usage["today"].as_str().unwrap();
    assert!(usage["days"][today]["up"].as_u64().unwrap() >= big.len() as u64, "{usage}");
}

#[test]
fn china_days() {
    assert_eq!(tbrendezvous::china_day(0), "1970-01-01");
    // 2026-11-14 15:59:59 UTC is 23:59:59 in China; a second later it is the 15th there.
    assert_eq!(tbrendezvous::china_day(1794671999), "2026-11-14");
    assert_eq!(tbrendezvous::china_day(1794672000), "2026-11-15");
    assert_eq!(tbrendezvous::china_day(951782400 - 8 * 3600), "2000-02-29");
}

/// Another home (the Mac's), added while running: devices reach it by naming it; Diamond's
/// streams and its Connection page are as before.
#[tokio::test]
async fn another_home_over_quic_is_reached_by_name() {
    let (server, secret, dir) = start_in();
    let diamond = home(server, &secret).await.unwrap();
    let mac_secret = std::fs::read_to_string(create_home_secret(&dir, "mac").unwrap()).unwrap();
    assert!(create_home_secret(&dir, "diamond").is_err());
    let _mac = tagged_home(server, mac_secret.trim(), " by mac").await.unwrap();
    let device = connect(server, ALPN_RELAY).await;
    assert_eq!(ask(&device, b"TBTO1 mac\nGET /pipe HTTP/1.1\r\n\r\n").await.unwrap(), "heard by mac GET /pipe HTTP/1.1 (22 bytes)");
    assert!(ask(&device, b"TBTO1 mac\nTBINFO1\n").await.unwrap().starts_with("home online\n"));
    assert_eq!(ask(&device, b"GET /v1/ping HTTP/1.1\r\n\r\n").await.unwrap(), "heard GET /v1/ping HTTP/1.1 (25 bytes)");
    assert_eq!(ask(&device, b"TBTO1 diamond\nGET /v1/ping HTTP/1.1\r\n\r\n").await.unwrap(), "heard GET /v1/ping HTTP/1.1 (25 bytes)");
    assert!(ask(&device, b"TBTO1 nobody\nGET / HTTP/1.1\r\n\r\n").await.is_err(), "an unknown home is refused");
    // A device that only used the Mac is not on Diamond's page; its bytes are not Diamond's.
    let other = connect(server, ALPN_RELAY).await;
    let big = [b"TBTO1 mac\nPOST /x HTTP/1.1\r\n\r\n".as_slice(), &vec![b'a'; 300_000]].concat();
    assert!(ask(&other, &big).await.unwrap().starts_with("heard by mac POST /x"));
    let (mut send, mut recv) = diamond.open_bi().await.unwrap();
    send.write_all(USAGE).await.unwrap();
    send.finish().unwrap();
    let usage: serde_json::Value = serde_json::from_slice(&recv.read_to_end(1 << 20).await.unwrap()).unwrap();
    assert_eq!(usage["devices"].as_array().unwrap().len(), 1, "{usage}");
    assert!(usage["days"][usage["today"].as_str().unwrap()]["up"].as_u64().unwrap() < 1000, "{usage}");
}

/// A home over TCP 443: it keeps idle upgraded connections, each carrying one device stream.
async fn pooled(server: SocketAddr, secret: String, idle: usize) {
    for _ in 0..idle { tokio::spawn(pooled_one(server, secret.clone())); }
}

fn pooled_one(server: SocketAddr, secret: String) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let tcp = *TCP.lock().unwrap().get(&server).unwrap();
        let mut stream = tokio::net::TcpStream::connect(tcp).await.unwrap();
        stream.write_all(b"GET /tbrelay HTTP/1.1\r\nHost: 47.112.18.69\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nX-Forwarded-For: 198.51.100.9\r\n\r\n").await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") { head.push(stream.read_u8().await.unwrap()) }
        stream.write_all(format!("{HOME_HELLO}{secret}\n").as_bytes()).await.unwrap();
        let mut ok = [0u8; 3];
        stream.read_exact(&mut ok).await.unwrap();
        assert_eq!(&ok, b"OK\n");
        loop {
            match stream.read_u8().await { Ok(KEEPALIVE) => continue, Ok(CLAIMED) => break, _ => return }
        }
        tokio::spawn(pooled_one(server, secret));
        let mut request = Vec::new();
        stream.read_to_end(&mut request).await.unwrap();
        let line = String::from_utf8_lossy(&request).lines().next().unwrap_or("").to_owned();
        stream.write_all(format!("heard by mac over tcp {line} ({} bytes)", request.len()).as_bytes()).await.unwrap();
        stream.shutdown().await.unwrap();
    })
}

#[tokio::test]
async fn another_home_over_tcp_is_reached_by_name() {
    let (server, _secret, dir) = start_in();
    let mac_secret = std::fs::read_to_string(create_home_secret(&dir, "mac").unwrap()).unwrap().trim().to_owned();
    pooled(server, mac_secret, 2).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let device = connect(server, ALPN_RELAY).await;
    let info = ask(&device, b"TBTO1 mac\nTBINFO1\n").await.unwrap();
    assert!(info.starts_with("home online\n") && info.contains("homeip 198.51.100.9"), "{info}");
    // More streams at once than idle connections: each claimed one is replaced.
    let asks: Vec<_> = (0..5).map(|i| { let device = device.clone(); tokio::spawn(async move { ask(&device, format!("TBTO1 mac\nGET /pipe/{i} HTTP/1.1\r\n\r\n").as_bytes()).await }) }).collect();
    for (i, asked) in asks.into_iter().enumerate() {
        assert_eq!(asked.await.unwrap().unwrap(), format!("heard by mac over tcp GET /pipe/{i} HTTP/1.1 (24 bytes)"));
    }
    // A device over TCP too, naming the home on its first line.
    let (_, reply) = ask_tcp(server, "/tbrelay", b"TBTO1 mac\nGET /pipe/tcp HTTP/1.1\r\n\r\n").await;
    assert_eq!(reply, "heard by mac over tcp GET /pipe/tcp HTTP/1.1 (26 bytes)");
    // Diamond is not connected: its streams are refused as before.
    assert!(ask(&device, b"GET /v1/ping HTTP/1.1\r\n\r\n").await.is_err());
    // A wrong secret gets no place in the pool.
    pooled_wrong(server).await;
}

async fn pooled_wrong(server: SocketAddr) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let tcp = *TCP.lock().unwrap().get(&server).unwrap();
    let mut stream = tokio::net::TcpStream::connect(tcp).await.unwrap();
    stream.write_all(b"GET /tbrelay HTTP/1.1\r\nHost: x\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n").await.unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") { head.push(stream.read_u8().await.unwrap()) }
    stream.write_all(format!("{HOME_HELLO}not-the-secret-not-the-secret-not-the-secret\n").as_bytes()).await.unwrap();
    let mut rest = Vec::new();
    stream.read_to_end(&mut rest).await.unwrap();
    assert!(rest.is_empty(), "closed without OK");
}
