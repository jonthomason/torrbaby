//! The library opens QUIC on a socket it is given, checks the pinned certificate, and its
//! loopback proxy carries a request to the server and back (as server/direct.rs pipes it).
use std::{ffi::CString, os::fd::IntoRawFd, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use sha2::Digest;

#[test]
fn proxy_reaches_the_server_through_quic_on_a_given_socket() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    // A QUIC server like Diamond's: its own certificate, ALPN tbdirect, echo-ish HTTP.
    let made = rcgen::generate_simple_self_signed(vec!["torrbaby-direct".into()]).unwrap();
    let cert = made.cert.der().clone();
    let fingerprint: String = sha2::Sha256::digest(cert.as_ref()).iter().map(|b| format!("{b:02x}")).collect();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(made.key_pair.serialize_der().into());
    let server = runtime.block_on(async {
        let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13]).unwrap().with_no_client_auth().with_single_cert(vec![cert], key).unwrap();
        tls.alpn_protocols = vec![b"tbdirect".to_vec()];
        let config = quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap()));
        let endpoint = quinn::Endpoint::server(config, "127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        tokio::spawn(async move {
            let connection = endpoint.accept().await.unwrap().await.unwrap();
            while let Ok((mut send, mut recv)) = connection.accept_bi().await {
                tokio::spawn(async move {
                    let mut request = [0u8; 128];
                    let n = recv.read(&mut request).await.unwrap().unwrap_or(0);
                    let line = String::from_utf8_lossy(&request[..n]).lines().next().unwrap_or("").to_owned();
                    send.write_all(format!("HTTP/1.1 200 OK\r\n\r\nheard {line}").as_bytes()).await.unwrap();
                    send.finish().unwrap();
                });
            }
        });
        address
    });
    // The punched socket stand-in.
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = CString::new(server.to_string()).unwrap();
    let pinned = CString::new(fingerprint).unwrap();
    let port = tbdirect::tbdirect_start(socket.into_raw_fd(), address.as_ptr(), pinned.as_ptr());
    assert_ne!(port, 0, "QUIC should open");
    assert!(tbdirect::tbdirect_alive());
    let reply = runtime.block_on(async {
        let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        tcp.write_all(b"GET /v1/ping HTTP/1.1\r\n\r\n").await.unwrap();
        let mut reply = String::new();
        tokio::time::timeout(Duration::from_secs(5), tcp.read_to_string(&mut reply)).await.unwrap().unwrap();
        reply
    });
    assert_eq!(reply, "HTTP/1.1 200 OK\r\n\r\nheard GET /v1/ping HTTP/1.1");
    tbdirect::tbdirect_stop();
    assert!(!tbdirect::tbdirect_alive());
    // A wrong fingerprint is refused.
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let wrong = CString::new("00".repeat(32)).unwrap();
    assert_eq!(tbdirect::tbdirect_start(socket.into_raw_fd(), address.as_ptr(), wrong.as_ptr()), 0);
}
