//! The whole punch on loopback: bronze's real STUN code on two ports, a stand-in Diamond that
//! answers POST /v1/direct over HTTP, echoes the punch on its own UDP socket and then serves
//! QUIC on that same socket (as server/direct.rs does), and a request through the path.
use sha2::Digest;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn punches_through_stun_and_signalling_and_carries_a_request() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (stun1, stun2) = runtime.block_on(async {
        let a = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let b = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (x, y) = (a.local_addr().unwrap(), b.local_addr().unwrap());
        tokio::spawn(tbrendezvous::stun::serve(a));
        tokio::spawn(tbrendezvous::stun::serve(b));
        (x, y)
    });
    // Diamond's certificate and its punch socket.
    let made = rcgen::generate_simple_self_signed(vec!["torrbaby-direct".into()]).unwrap();
    let cert = made.cert.der().clone();
    let fingerprint: String = sha2::Sha256::digest(cert.as_ref()).iter().map(|b| format!("{b:02x}")).collect();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(made.key_pair.serialize_der().into());
    let diamond = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let diamond_address = diamond.local_addr().unwrap();
    // POST /v1/direct, once.
    let http = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let signal = format!("http://{}", http.local_addr().unwrap());
    let answer_fingerprint = fingerprint.clone();
    std::thread::spawn(move || {
        let (stream, _) = http.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let mut length = 0;
        let mut authorized = false;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" { break }
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") { length = v.trim().parse().unwrap() }
            if line.starts_with("Authorization: Bearer device-token") { authorized = true }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(authorized, "the token goes with the request");
        assert!(request["ports"].as_array().is_some_and(|p| !p.is_empty()), "the ports STUN saw go with the request");
        let answer = serde_json::json!({"id": "s1", "server": diamond_address.to_string(), "fingerprint": answer_fingerprint,
            "low": request["low"], "high": request["high"], "spread": request["spread"].as_str().unwrap_or("window")}).to_string();
        let mut stream = reader.into_inner();
        write!(stream, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{answer}", answer.len()).unwrap();
    });
    // Echo the first punch packet, then serve QUIC on the same socket.
    let server = runtime.spawn(async move {
        let mut buffer = [0u8; 1500];
        let (n, from) = diamond.recv_from(&mut buffer).unwrap();
        let text = String::from_utf8_lossy(&buffer[..n]).to_string();
        assert!(text.starts_with("TBDIRECT1 s1 "), "{text}");
        for _ in 0..3 { diamond.send_to(format!("TBACK {text}").as_bytes(), from).unwrap(); }
        diamond.set_nonblocking(true).unwrap();
        let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13]).unwrap().with_no_client_auth().with_single_cert(vec![cert], key).unwrap();
        tls.alpn_protocols = vec![b"tbdirect".to_vec()];
        let config = quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap()));
        let endpoint = quinn::Endpoint::new(quinn::EndpointConfig::default(), Some(config), diamond, Arc::new(quinn::TokioRuntime)).unwrap();
        let connection = endpoint.accept().await.unwrap().await.unwrap();
        let (mut send, mut recv) = connection.accept_bi().await.unwrap();
        let mut request = [0u8; 128];
        let n = recv.read(&mut request).await.unwrap().unwrap_or(0);
        let line = String::from_utf8_lossy(&request[..n]).lines().next().unwrap_or("").to_owned();
        send.write_all(format!("HTTP/1.1 200 OK\r\n\r\nheard {line}").as_bytes()).await.unwrap();
        send.finish().unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
    });
    let mut steps = Vec::new();
    let request = serde_json::json!({"stun": stun1.to_string(), "stun2": stun2.to_string(), "signal": signal, "token": "device-token", "sockets": 16}).to_string();
    let result = tbdirect::punch_json(request, |r| steps.push(r.step.clone()));
    assert_eq!(result["ok"], true, "{result}");
    assert!(steps.iter().any(|s| s.starts_with("punching")), "{steps:?}");
    assert_eq!(result["report"]["answered"], 12);
    let port = result["port"].as_u64().unwrap() as u16;
    let reply = runtime.block_on(async {
        let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        tcp.write_all(b"GET /v1/ping HTTP/1.1\r\n\r\n").await.unwrap();
        let mut reply = String::new();
        tokio::time::timeout(Duration::from_secs(5), tcp.read_to_string(&mut reply)).await.unwrap().unwrap();
        reply
    });
    assert_eq!(reply, "HTTP/1.1 200 OK\r\n\r\nheard GET /v1/ping HTTP/1.1");
    eprintln!("{}", result);
    tbdirect::tbdirect_stop();
    runtime.block_on(server).unwrap();
}
