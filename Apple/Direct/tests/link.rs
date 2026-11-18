//! Torrbaby's link on loopback: a real rendezvous (rendezvous/), a stand-in Diamond that holds
//! its connection and serves relayed streams the way Diamond does (abt-link's pipe_relayed: TLS
//! to its identity, end to end, then its HTTP port), and a request on the link's loopback port.
mod common;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn a_request_on_the_link_reaches_diamond_end_to_end_through_bronze() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let setup = common::bronze_and_diamond(&runtime, "link", std::sync::Arc::new(Vec::new()));
    let (rendezvous, diamond) = (setup.rendezvous, setup.diamond.clone());
    struct Bronze { fingerprint: String }
    let bronze = Bronze { fingerprint: setup.bronze.clone() };
    // The device: Diamond's identity pinned, no token (relay only, no punch).
    let pinned = std::ffi::CString::new(diamond.clone()).unwrap();
    tbdirect::tbdirect_identity(pinned.as_ptr());
    let mut config = tbdirect::proxy::Config::local(rendezvous, &bronze.fingerprint);
    let port = tbdirect::proxy::start(config.clone());
    assert_ne!(port, 0, "the relay opens");
    assert_eq!(tbdirect::proxy::start(config), port, "starting again keeps the same port");
    let ask = |path: &'static str| runtime.block_on(async move {
        let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        tcp.write_all(format!("GET {path} HTTP/1.1\r\nHost: diamond\r\n\r\n").as_bytes()).await.unwrap();
        let mut reply = String::new();
        tokio::time::timeout(Duration::from_secs(10), tcp.read_to_string(&mut reply)).await.unwrap().unwrap();
        reply
    });
    let reply = ask("/v1/ping");
    assert!(reply.ends_with("Diamond heard GET /v1/ping HTTP/1.1"), "{reply}");
    let state = tbdirect::proxy::state();
    assert_eq!(state["state"], "relay", "{state}");
    assert_eq!(state["relay"], "quic", "{state}");
    assert!(state["bytes"]["relay"]["received"].as_u64().unwrap() > 0, "{state}");

    // A wrong identity pinned: the stand-in Diamond's TLS is refused, so nothing gets through.
    tbdirect::proxy::stop();
    let wrong = std::ffi::CString::new("00".repeat(32)).unwrap();
    tbdirect::tbdirect_identity(wrong.as_ptr());
    let port = tbdirect::proxy::start(tbdirect::proxy::Config::local(rendezvous, &bronze.fingerprint));
    assert_ne!(port, 0);
    let refused = runtime.block_on(async move {
        let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let _ = tcp.write_all(b"GET /v1/ping HTTP/1.1\r\n\r\n").await;
        let mut reply = String::new();
        let _ = tokio::time::timeout(Duration::from_secs(10), tcp.read_to_string(&mut reply)).await;
        reply
    });
    assert!(!refused.contains("Diamond heard"), "a wrong identity must not get through: {refused}");
    tbdirect::proxy::stop();
    assert!(!tbdirect::proxy::running());
}
