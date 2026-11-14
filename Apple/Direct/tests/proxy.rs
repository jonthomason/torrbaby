//! The proxy mode (Android TVs away from home): one loopback port, routed by the best path open,
//! and the Rust maintainer that keeps the paths. On loopback: a relay stand-in (bronze: TBINFO,
//! and each other stream spliced to "Diamond", which answers POST /v1/direct and serves an
//! endless download of 1s), bronze's real STUN code on two ports, and a Diamond stand-in that
//! echoes the punch and serves QUIC on the punched socket (downloads of 2s).
use sha2::Digest;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The proxy, its paths and their counters are process-wide: the tests take turns.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn log(line: &str) { eprintln!("maintainer: {line}") }

fn identity(name: &str) -> (rustls::pki_types::CertificateDer<'static>, rustls::pki_types::PrivateKeyDer<'static>, String) {
    let made = rcgen::generate_simple_self_signed(vec![name.into()]).unwrap();
    let cert = made.cert.der().clone();
    let fingerprint = sha2::Sha256::digest(cert.as_ref()).iter().map(|b| format!("{b:02x}")).collect();
    (cert, rustls::pki_types::PrivateKeyDer::Pkcs8(made.key_pair.serialize_der().into()), fingerprint)
}

fn quic_server(alpn: &[u8], cert: rustls::pki_types::CertificateDer<'static>, key: rustls::pki_types::PrivateKeyDer<'static>) -> quinn::ServerConfig {
    let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13]).unwrap().with_no_client_auth().with_single_cert(vec![cert], key).unwrap();
    tls.alpn_protocols = vec![alpn.to_vec()];
    quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap()))
}

/// What the relay stand-in saw.
#[derive(Default)]
struct Seen { posts: AtomicUsize, relayed: AtomicUsize, infos: AtomicUsize }

/// bronze's stand-in. `info` is its TBINFO reply; `answer` what Diamond answers the first
/// POST /v1/direct with (after `gate` opens); later POSTs are refused.
fn relay(runtime: &tokio::runtime::Runtime, info: String, answer: Option<serde_json::Value>, gate: Arc<tokio::sync::Notify>) -> (String, String, Arc<Seen>) {
    let (cert, key, fingerprint) = identity("torrbaby-rendezvous");
    let seen = Arc::new(Seen::default());
    let counted = seen.clone();
    let address = runtime.block_on(async move {
        let endpoint = quinn::Endpoint::server(quic_server(b"tbrelay", cert, key), "127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let (info, answer, gate, seen) = (info.clone(), answer.clone(), gate.clone(), counted.clone());
                tokio::spawn(async move {
                    let Ok(connection) = incoming.await else { return };
                    while let Ok((mut send, mut recv)) = connection.accept_bi().await {
                        let (info, answer, gate, seen) = (info.clone(), answer.clone(), gate.clone(), seen.clone());
                        tokio::spawn(async move {
                            let mut head = Vec::new();
                            let mut buffer = [0u8; 4096];
                            // Enough to tell the kind of stream (and, for a POST, its whole body).
                            loop {
                                let Ok(Some(n)) = recv.read(&mut buffer).await else { break };
                                head.extend_from_slice(&buffer[..n]);
                                if head.starts_with(b"TBINFO1\n") || head.starts_with(b"TBNAME1 ") { break }
                                let text = String::from_utf8_lossy(&head).to_string();
                                if let Some(split) = text.find("\r\n\r\n") {
                                    let length: usize = text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
                                    if head.len() >= split + 4 + length { break }
                                }
                            }
                            if head.starts_with(b"TBINFO1\n") {
                                seen.infos.fetch_add(1, Ordering::SeqCst);
                                let _ = send.write_all(info.as_bytes()).await;
                                let _ = send.finish();
                            } else if head.starts_with(b"TBNAME1 ") {
                            } else if head.starts_with(b"POST /v1/direct ") {
                                let text = String::from_utf8_lossy(&head).to_string();
                                assert!(text.contains("Authorization: Bearer device-token"), "the token goes with the request");
                                let body: serde_json::Value = serde_json::from_str(&text[text.find("\r\n\r\n").unwrap() + 4..]).unwrap();
                                let first = seen.posts.fetch_add(1, Ordering::SeqCst) == 0;
                                let reply = match (&answer, first) {
                                    (Some(answer), true) => {
                                        gate.notified().await;
                                        let mut answer = answer.clone();
                                        answer["low"] = body["low"].clone(); answer["high"] = body["high"].clone();
                                        answer["spread"] = serde_json::json!(body["spread"].as_str().unwrap_or("window"));
                                        format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{answer}", answer.to_string().len())
                                    }
                                    _ => "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 17\r\n\r\n{\"error\":\"busy\"}\n".into(),
                                };
                                let _ = send.write_all(reply.as_bytes()).await;
                                let _ = send.finish();
                            } else {
                                // Anything else: an endless download over the relay (video).
                                seen.relayed.fetch_add(1, Ordering::SeqCst);
                                let chunk = vec![1u8; 16 * 1024];
                                loop {
                                    if send.write_all(&chunk).await.is_err() { return }
                                    tokio::time::sleep(Duration::from_millis(10)).await;
                                }
                            }
                        });
                    }
                });
            }
        });
        address
    });
    (address.to_string(), fingerprint, seen)
}

/// Diamond's punch socket: echoes the first punch packet, then serves QUIC on the same socket,
/// answering each stream with 256 KB of 2s.
fn diamond(runtime: &tokio::runtime::Runtime) -> (String, String) {
    let (cert, key, fingerprint) = identity("torrbaby-direct");
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    runtime.spawn(async move {
        let socket = tokio::task::spawn_blocking(move || {
            let mut buffer = [0u8; 1500];
            let (n, from) = socket.recv_from(&mut buffer).unwrap();
            let text = String::from_utf8_lossy(&buffer[..n]).to_string();
            assert!(text.starts_with("TBDIRECT1 s1 "), "{text}");
            for _ in 0..3 { socket.send_to(format!("TBACK {text}").as_bytes(), from).unwrap(); }
            socket.set_nonblocking(true).unwrap();
            socket
        }).await.unwrap();
        let endpoint = quinn::Endpoint::new(quinn::EndpointConfig::default(), Some(quic_server(b"tbdirect", cert, key)), socket, Arc::new(quinn::TokioRuntime)).unwrap();
        while let Some(incoming) = endpoint.accept().await {
            let Ok(connection) = incoming.await else { continue };
            tokio::spawn(async move {
                while let Ok((mut send, mut recv)) = connection.accept_bi().await {
                    tokio::spawn(async move {
                        let mut request = [0u8; 256];
                        let _ = recv.read(&mut request).await;
                        let _ = send.write_all(&vec![2u8; 256 * 1024]).await;
                        let _ = send.finish();
                    });
                }
            });
        }
    });
    (address.to_string(), fingerprint)
}

fn stun(runtime: &tokio::runtime::Runtime) -> (String, String) {
    runtime.block_on(async {
        let a = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let b = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (x, y) = (a.local_addr().unwrap().to_string(), b.local_addr().unwrap().to_string());
        tokio::spawn(tbrendezvous::stun::serve(a));
        tokio::spawn(tbrendezvous::stun::serve(b));
        (x, y)
    })
}

fn wait_for(seconds: u64, what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        if done() { return }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("{what} within {seconds} s; status: {}", tbdirect::proxy::status());
}

#[test]
fn a_connection_goes_by_the_relay_then_moves_to_the_punched_direct_path() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    tbdirect::proxy::set_logger(log);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (stun1, stun2) = stun(&runtime);
    let (diamond_address, diamond_fingerprint) = diamond(&runtime);
    let gate = Arc::new(tokio::sync::Notify::new());
    // bronze sees this device at 127.0.0.1 (as STUN does) and Diamond elsewhere.
    let answer = serde_json::json!({"id": "s1", "server": diamond_address, "fingerprint": diamond_fingerprint});
    let (relay_address, relay_fingerprint, seen) = relay(&runtime, "home online\nyou 127.0.0.1:40000\nhomeip 10.0.0.1\n".into(), Some(answer), gate.clone());
    tbdirect::proxy::network("wlan0 192.168.1.20 via 192.168.1.1", "");
    let mut config = tbdirect::proxy::Config::new(&relay_address, &relay_fingerprint);
    config.stun = stun1;
    config.stun2 = Some(stun2);
    config.token = "device-token".into();
    let port = tbdirect::proxy::start(config.clone());
    assert_ne!(port, 0, "the relay opens");
    assert_eq!(tbdirect::proxy::start(config), port, "starting again keeps the same port");
    let moved_before = moved();

    // A long download begins while the direct path is still being punched: it goes by the relay.
    let (relayed, cut) = runtime.block_on(async {
        let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        tcp.write_all(b"GET /video HTTP/1.1\r\n\r\n").await.unwrap();
        let mut buffer = vec![0u8; 64 * 1024];
        let mut relayed = 0usize;
        while relayed < 128 * 1024 {
            let n = tokio::time::timeout(Duration::from_secs(10), tcp.read(&mut buffer)).await.unwrap().unwrap();
            assert!(n > 0 && buffer[..n].iter().all(|b| *b == 1), "the download comes over the relay");
            relayed += n;
        }
        // Diamond's answer to the punch's signalling (through the relay) is let go now.
        let deadline = Instant::now() + Duration::from_secs(15);
        while seen.posts.load(Ordering::SeqCst) == 0 && Instant::now() < deadline { tokio::time::sleep(Duration::from_millis(50)).await }
        assert_eq!(seen.posts.load(Ordering::SeqCst), 1, "the maintainer asked Diamond to start, through the relay");
        assert!(tbdirect::proxy::status().contains("punching now"), "{}", tbdirect::proxy::status());
        gate.notify_one();
        // Once the direct path has been open 2 s, the relay connection makes way (closed).
        let cut = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                match tcp.read(&mut buffer).await { Ok(0) | Err(_) => return, Ok(n) => relayed += n }
            }
        }).await.is_ok();
        (relayed, cut)
    });
    assert!(cut, "the relay connection was closed once the direct path opened");
    assert!(tbdirect::tbdirect_alive(), "the direct path is open");
    assert!(moved() > moved_before);
    // The client asks again, as a player resumes: by the direct path, at the same port.
    let direct = runtime.block_on(async {
        let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        tcp.write_all(b"GET /video HTTP/1.1\r\n\r\n").await.unwrap();
        let mut reply = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), tcp.read_to_end(&mut reply)).await.unwrap().unwrap();
        reply
    });
    assert_eq!(direct.len(), 256 * 1024);
    assert!(direct.iter().all(|b| *b == 2), "the next request came by the direct path");
    let status = tbdirect::proxy::status();
    eprintln!("{status}");
    assert!(status.contains("direct open (punched from 127.0.0.1"), "{status}");
    assert!(status.contains("last punch opened"), "{status}");
    assert_eq!(seen.relayed.load(Ordering::SeqCst), 1, "{relayed} bytes came over the relay, by one connection");

    // A new network: the direct path is dropped, the relay moves with the device, and the
    // punch is tried again (Diamond's stand-in refuses this one): a failure, and a wait.
    tbdirect::proxy::network("eth0 10.1.1.20 via 10.1.1.1", "");
    wait_for(8, "the direct path dropped on the network change", || !tbdirect::tbdirect_alive());
    assert!(tbdirect::tbdirect_relay_alive(), "the relay moved to the new network");
    wait_for(15, "the second punch failed", || tbdirect::proxy::status().contains("last punch failed"));
    let status = tbdirect::proxy::status();
    assert!(status.contains("next punch in"), "{status}");
    assert!(tbdirect::proxy::notes().iter().any(|n| n.contains("network changed") && n.contains("moved to the new network")));
    tbdirect::proxy::stop();
    assert!(!tbdirect::tbdirect_relay_alive() && !tbdirect::tbdirect_alive() && !tbdirect::proxy::running());
}

fn moved() -> u64 { tbdirect::packet::stats::get(&tbdirect::packet::stats::FLOWS_MOVED) }

#[test]
fn behind_diamonds_own_address_the_proxy_stays_on_the_relay_and_never_punches() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    tbdirect::proxy::set_logger(log);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    // STUN that only counts: a punch would ask it first.
    let stun = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    stun.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    let (relay_address, relay_fingerprint, seen) = relay(&runtime, "home online\nyou 203.0.113.7:40000\nhomeip 203.0.113.7\n".into(), None, Arc::new(tokio::sync::Notify::new()));
    let mut config = tbdirect::proxy::Config::new(&relay_address, &relay_fingerprint);
    config.stun = stun.local_addr().unwrap().to_string();
    config.token = "device-token".into();
    let port = tbdirect::proxy::start(config);
    assert_ne!(port, 0);
    wait_for(8, "bronze was asked twice", || seen.infos.load(Ordering::SeqCst) >= 2);
    let status = tbdirect::proxy::status();
    assert!(status.contains("relay only"), "{status}");
    assert!(status.contains("public address 203.0.113.7"), "{status}");
    assert_eq!(seen.posts.load(Ordering::SeqCst), 0, "Diamond was never asked to punch");
    assert!(stun.recv_from(&mut [0u8; 64]).is_err(), "STUN was never asked");
    // Playback-sized traffic flows through the proxy by the relay.
    let got = runtime.block_on(async {
        let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        tcp.write_all(b"GET /video HTTP/1.1\r\n\r\n").await.unwrap();
        let mut buffer = vec![0u8; 64 * 1024];
        let mut got = 0usize;
        while got < 2 << 20 { got += tokio::time::timeout(Duration::from_secs(10), tcp.read(&mut buffer)).await.unwrap().unwrap(); }
        got
    });
    assert!(got >= 2 << 20);
    eprintln!("{}", tbdirect::proxy::status());
    tbdirect::proxy::stop();
}

#[test]
fn an_idle_proxy_lets_its_paths_go_and_wakes_for_the_next_connection() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    tbdirect::proxy::set_logger(log);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (relay_address, relay_fingerprint, _seen) = relay(&runtime, "home online\nyou 198.51.100.4:40000\nhomeip 203.0.113.7\n".into(), None, Arc::new(tokio::sync::Notify::new()));
    let mut config = tbdirect::proxy::Config::new(&relay_address, &relay_fingerprint);
    config.idle = Duration::from_secs(2);
    let port = tbdirect::proxy::start(config);
    assert_ne!(port, 0);
    wait_for(5, "relay only (no token, no punch)", || tbdirect::proxy::status().contains("relay only: no device token"));
    wait_for(10, "dormant", || tbdirect::proxy::status().contains("dormant") && !tbdirect::tbdirect_relay_alive());
    // A connection arrives: it waits while the relay opens again, then is carried.
    let started = Instant::now();
    let first = runtime.block_on(async {
        let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        tcp.write_all(b"GET /v1/ping HTTP/1.1\r\n\r\n").await.unwrap();
        let mut buffer = [0u8; 1024];
        tokio::time::timeout(Duration::from_secs(10), tcp.read(&mut buffer)).await.unwrap().unwrap()
    });
    assert!(first > 0, "carried after waking");
    eprintln!("woke and carried in {:?}", started.elapsed());
    assert!(tbdirect::tbdirect_relay_alive());
    assert!(!tbdirect::proxy::status().contains(", dormant;"), "{}", tbdirect::proxy::status());
    tbdirect::proxy::stop();
}
