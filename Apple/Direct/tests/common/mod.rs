//! A real rendezvous (rendezvous/) and a stand-in Diamond on loopback: it holds its standing
//! connection to bronze and serves relayed streams the way Diamond does (abt-link's
//! pipe_relayed: TLS to its identity end to end, then its HTTP port). Diamond's HTTP port
//! answers "/download" with `download` and anything else with the request line it heard.
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub struct Setup { pub rendezvous: std::net::SocketAddr, pub bronze: String, pub diamond: String }

pub fn bronze_and_diamond(runtime: &tokio::runtime::Runtime, name: &str, download: Arc<Vec<u8>>) -> Setup {
    let dir = std::env::temp_dir().join(format!("tbdirect-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bronze")).unwrap();
    std::fs::create_dir_all(dir.join("diamond")).unwrap();
    let bronze = tbrendezvous::Identity::load_or_create(&dir.join("bronze")).unwrap();
    let secret = tbrendezvous::load_or_create_secret(&dir.join("bronze")).unwrap();
    abt_link::host::load_identity(&dir.join("diamond")).unwrap();
    let diamond = abt_link::host::fingerprint().unwrap();
    let fingerprint = bronze.fingerprint.clone();
    let rendezvous = runtime.block_on(async move {
        let http = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http_port = http.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = http.accept().await {
                let download = download.clone();
                tokio::spawn(async move {
                    let mut request = [0u8; 512];
                    let n = tcp.read(&mut request).await.unwrap_or(0);
                    let line = String::from_utf8_lossy(&request[..n]).lines().next().unwrap_or("").to_owned();
                    if line.starts_with("GET /download") {
                        let _ = tcp.write_all(&download).await;
                    } else {
                        let body = format!("Diamond heard {line}");
                        let _ = tcp.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await;
                    }
                    let _ = tcp.shutdown().await;
                });
            }
        });
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = socket.local_addr().unwrap();
        tokio::spawn(tbrendezvous::serve(tbrendezvous::endpoint(socket, &bronze).unwrap(), tbrendezvous::Hub::new(&secret)));
        let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(bronze.cert.clone()).unwrap();
        let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13]).unwrap().with_root_certificates(roots).with_no_client_auth();
        tls.alpn_protocols = vec![b"tbhome".to_vec()];
        endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap())));
        let home = endpoint.connect(address, "torrbaby-rendezvous").unwrap().await.unwrap();
        let (mut send, mut recv) = home.open_bi().await.unwrap();
        send.write_all(format!("TBHOME1 {secret}\n").as_bytes()).await.unwrap();
        send.finish().unwrap();
        assert_eq!(recv.read_to_end(8).await.unwrap(), b"OK\n");
        tokio::spawn(async move {
            let _endpoint = endpoint;
            while let Ok((send, recv)) = home.accept_bi().await {
                tokio::spawn(abt_link::host::pipe_relayed(tokio::io::join(recv, send), http_port, "Relay".into()));
            }
        });
        address
    });
    Setup { rendezvous, bronze: fingerprint, diamond }
}
