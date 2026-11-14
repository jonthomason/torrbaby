//! The relay: the library connects to a real rendezvous (rendezvous/, on loopback), a
//! stand-in Diamond holds its connection, and a request on the loopback port reaches it.
use std::{ffi::CString, os::fd::IntoRawFd, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn a_loopback_request_reaches_diamond_through_the_relay() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let dir = std::env::temp_dir().join(format!("tbdirect-relay-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let identity = tbrendezvous::Identity::load_or_create(&dir).unwrap();
    let secret = tbrendezvous::load_or_create_secret(&dir).unwrap();
    let fingerprint = identity.fingerprint.clone();
    let rendezvous = runtime.block_on(async {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = socket.local_addr().unwrap();
        tokio::spawn(tbrendezvous::serve(tbrendezvous::endpoint(socket, &identity).unwrap(), tbrendezvous::Hub::new(&secret)));
        // A stand-in Diamond (trusting any certificate is fine for the stand-in).
        let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(identity.cert.clone()).unwrap();
        let mut tls = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13]).unwrap().with_root_certificates(roots).with_no_client_auth();
        tls.alpn_protocols = vec![b"tbhome".to_vec()];
        endpoint.set_default_client_config(quinn::ClientConfig::new(std::sync::Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap())));
        let home = endpoint.connect(address, "torrbaby-rendezvous").unwrap().await.unwrap();
        let (mut send, mut recv) = home.open_bi().await.unwrap();
        send.write_all(format!("TBHOME1 {secret}\n").as_bytes()).await.unwrap();
        send.finish().unwrap();
        assert_eq!(recv.read_to_end(8).await.unwrap(), b"OK\n");
        tokio::spawn(async move {
            let _endpoint = endpoint;
            while let Ok((mut send, mut recv)) = home.accept_bi().await {
                tokio::spawn(async move {
                    let mut request = [0u8; 256];
                    let n = recv.read(&mut request).await.unwrap().unwrap_or(0);
                    let line = String::from_utf8_lossy(&request[..n]).lines().next().unwrap_or("").to_owned();
                    send.write_all(format!("HTTP/1.1 200 OK\r\n\r\nDiamond heard {line}").as_bytes()).await.unwrap();
                    send.finish().unwrap();
                });
            }
        });
        address
    });
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let server = CString::new(rendezvous.to_string()).unwrap();
    // A wrong fingerprint is refused.
    let wrong = CString::new("00".repeat(32)).unwrap();
    assert_eq!(tbdirect::tbdirect_relay_start(std::net::UdpSocket::bind("127.0.0.1:0").unwrap().into_raw_fd(), server.as_ptr(), wrong.as_ptr()), 0);
    let pinned = CString::new(fingerprint).unwrap();
    let port = tbdirect::tbdirect_relay_start(socket.into_raw_fd(), server.as_ptr(), pinned.as_ptr());
    assert_ne!(port, 0);
    assert!(tbdirect::tbdirect_relay_alive());
    assert!(!tbdirect::tbdirect_alive(), "the direct tunnel is separate");
    let reply = runtime.block_on(async {
        let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        tcp.write_all(b"GET /v1/ping HTTP/1.1\r\n\r\n").await.unwrap();
        let mut reply = String::new();
        tokio::time::timeout(Duration::from_secs(5), tcp.read_to_string(&mut reply)).await.unwrap().unwrap();
        reply
    });
    assert_eq!(reply, "HTTP/1.1 200 OK\r\n\r\nDiamond heard GET /v1/ping HTTP/1.1");
    // Bronze says Diamond is connected and where it sees this device.
    let mut info = [0 as std::ffi::c_char; 256];
    let length = tbdirect::tbdirect_relay_info(info.as_mut_ptr(), info.len());
    assert!(length > 0);
    let text = unsafe { std::ffi::CStr::from_ptr(info.as_ptr()) }.to_str().unwrap().to_owned();
    assert!(text.starts_with("home online\nyou 127.0.0.1:"), "{text}");
    // A new network: the relay moves to a new socket and carries on, and bronze sees the move.
    let moved = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let new_port = moved.local_addr().unwrap().port();
    assert!(tbdirect::tbdirect_relay_rebind(moved.into_raw_fd()));
    let reply = runtime.block_on(async {
        let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        tcp.write_all(b"GET /v1/library HTTP/1.1\r\n\r\n").await.unwrap();
        let mut reply = String::new();
        tokio::time::timeout(Duration::from_secs(5), tcp.read_to_string(&mut reply)).await.unwrap().unwrap();
        reply
    });
    assert_eq!(reply, "HTTP/1.1 200 OK\r\n\r\nDiamond heard GET /v1/library HTTP/1.1");
    let length = tbdirect::tbdirect_relay_info(info.as_mut_ptr(), info.len());
    assert!(length > 0);
    let text = unsafe { std::ffi::CStr::from_ptr(info.as_ptr()) }.to_str().unwrap().to_owned();
    assert!(text.contains(&format!("you 127.0.0.1:{new_port}")), "bronze sees the new socket: {text}");
    tbdirect::tbdirect_relay_stop();
    assert!(!tbdirect::tbdirect_relay_alive());
}
