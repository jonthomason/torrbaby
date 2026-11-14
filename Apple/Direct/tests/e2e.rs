//! Through bronze, end to end: with Diamond's identity pinned, a tunnel connection carried by
//! the relay reaches the far side as TLS (bronze sees only ciphertext), and Diamond's side
//! decrypts it with its identity; a different certificate is refused.
use sha2::Digest;
use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, Ipv4Address};
use std::{ffi::CString, os::fd::IntoRawFd, sync::{Arc, Mutex, mpsc}, time::{Duration, Instant}};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
static FROM_STACK: Mutex<Option<mpsc::Sender<Vec<u8>>>> = Mutex::new(None);
extern "C" fn written(_: *mut std::ffi::c_void, data: *const u8, lengths: *const usize, count: usize) {
    let lengths = unsafe { std::slice::from_raw_parts(lengths, count) };
    let total: usize = lengths.iter().sum();
    let data = unsafe { std::slice::from_raw_parts(data, total) };
    let mut at = 0;
    for &length in lengths {
        if let Some(sender) = FROM_STACK.lock().unwrap().as_ref() { let _ = sender.send(data[at..at + length].to_vec()); }
        at += length;
    }
}

struct Wire { incoming: mpsc::Receiver<Vec<u8>>, outgoing: Vec<Vec<u8>> }
struct Rx(Vec<u8>);
struct Tx<'a>(&'a mut Vec<Vec<u8>>);
impl RxToken for Rx { fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R { f(&self.0) } }
impl TxToken for Tx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, length: usize, f: F) -> R {
        let mut packet = vec![0; length];
        let result = f(&mut packet);
        self.0.push(packet);
        result
    }
}
impl Device for Wire {
    type RxToken<'a> = Rx where Self: 'a;
    type TxToken<'a> = Tx<'a> where Self: 'a;
    fn receive(&mut self, _: smoltcp::time::Instant) -> Option<(Rx, Tx<'_>)> {
        let packet = self.incoming.try_recv().ok()?;
        Some((Rx(packet), Tx(&mut self.outgoing)))
    }
    fn transmit(&mut self, _: smoltcp::time::Instant) -> Option<Tx<'_>> { Some(Tx(&mut self.outgoing)) }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut capabilities = DeviceCapabilities::default();
        capabilities.medium = Medium::Ip;
        capabilities.max_transmission_unit = 1280;
        capabilities
    }
}


/// One request through the tunnel's packets; returns the reply.
fn request_through_tunnel(port: u16) -> Vec<u8> {
    let (sender, incoming) = mpsc::channel();
    *FROM_STACK.lock().unwrap() = Some(sender);
    assert!(tbdirect::tbtunnel_start(1280, written, std::ptr::null_mut()));
    let mut wire = Wire { incoming, outgoing: Vec::new() };
    let mut interface = Interface::new(Config::new(HardwareAddress::Ip), &mut wire, smoltcp::time::Instant::now());
    interface.update_ip_addrs(|addresses| { addresses.push(IpCidr::new(IpAddress::v4(10, 233, 0, 2), 32)).unwrap(); });
    interface.routes_mut().add_default_ipv4_route(Ipv4Address::new(10, 233, 0, 1)).unwrap();
    let mut sockets = SocketSet::new(vec![]);
    let handle = sockets.add(tcp::Socket::new(tcp::SocketBuffer::new(vec![0; 64 * 1024]), tcp::SocketBuffer::new(vec![0; 4 * 1024])));
    sockets.get_mut::<tcp::Socket>(handle).connect(interface.context(), (IpAddress::v4(10, 233, 0, 1), 8765), port).unwrap();
    let (mut sent, mut received) = (false, Vec::new());
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        interface.poll(smoltcp::time::Instant::now(), &mut wire, &mut sockets);
        for packet in wire.outgoing.drain(..) { tbdirect::tbtunnel_input(packet.as_ptr(), packet.len()); }
        let socket = sockets.get_mut::<tcp::Socket>(handle);
        if socket.may_send() && !sent { socket.send_slice(b"GET /v1/ping HTTP/1.1\r\n\r\n").unwrap(); sent = true; }
        while socket.can_recv() { socket.recv(|data| { received.extend_from_slice(data); (data.len(), ()) }).unwrap(); }
        if sent && !socket.may_recv() { break }
        std::thread::sleep(Duration::from_millis(1));
    }
    tbdirect::tbtunnel_stop();
    received
}

#[test]
fn a_relayed_connection_is_encrypted_to_diamonds_identity() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let provider = || Arc::new(rustls::crypto::ring::default_provider());
    // bronze's stand-in: QUIC with its own certificate; each stream goes to "Diamond", which
    // records the first byte bronze saw and answers through TLS with Diamond's identity.
    let bronze = rcgen::generate_simple_self_signed(vec!["torrbaby-rendezvous".into()]).unwrap();
    let bronze_fingerprint: String = sha2::Sha256::digest(bronze.cert.der().as_ref()).iter().map(|b| format!("{b:02x}")).collect();
    let diamond = rcgen::generate_simple_self_signed(vec!["torrbaby-diamond".into()]).unwrap();
    let diamond_fingerprint: String = sha2::Sha256::digest(diamond.cert.der().as_ref()).iter().map(|b| format!("{b:02x}")).collect();
    let mut diamond_tls = rustls::ServerConfig::builder_with_provider(provider()).with_protocol_versions(&[&rustls::version::TLS13]).unwrap().with_no_client_auth()
        .with_single_cert(vec![diamond.cert.der().clone()], rustls::pki_types::PrivateKeyDer::Pkcs8(diamond.key_pair.serialize_der().into())).unwrap();
    diamond_tls.alpn_protocols = vec![b"tbe2e".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(diamond_tls));
    let seen = Arc::new(Mutex::new(Vec::<u8>::new()));
    let seen_by_bronze = seen.clone();
    let relay = runtime.block_on(async move {
        let mut tls = rustls::ServerConfig::builder_with_provider(provider()).with_protocol_versions(&[&rustls::version::TLS13]).unwrap().with_no_client_auth()
            .with_single_cert(vec![bronze.cert.der().clone()], rustls::pki_types::PrivateKeyDer::Pkcs8(bronze.key_pair.serialize_der().into())).unwrap();
        tls.alpn_protocols = vec![b"tbrelay".to_vec()];
        let endpoint = quinn::Endpoint::server(quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap())), "127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        tokio::spawn(async move {
            let connection = endpoint.accept().await.unwrap().await.unwrap();
            while let Ok((send, mut recv)) = connection.accept_bi().await {
                let (acceptor, seen) = (acceptor.clone(), seen_by_bronze.clone());
                tokio::spawn(async move {
                    let mut first = vec![0u8; 4096];
                    let n = recv.read(&mut first).await.unwrap().unwrap_or(0);
                    first.truncate(n);
                    seen.lock().unwrap().extend_from_slice(&first);
                    let stream = tokio::io::join(std::io::Cursor::new(first).chain(recv), send);
                    let Ok(mut tls) = acceptor.accept(stream).await else { return };
                    let mut request = [0u8; 256];
                    let n = tls.read(&mut request).await.unwrap_or(0);
                    let reply = format!("HTTP/1.1 200 OK\r\n\r\nheard {}", String::from_utf8_lossy(&request[..n]).lines().next().unwrap_or(""));
                    let _ = tls.write_all(reply.as_bytes()).await;
                    let _ = tls.shutdown().await;
                });
            }
        });
        address
    });
    let address = CString::new(relay.to_string()).unwrap();
    let pinned = CString::new(bronze_fingerprint).unwrap();
    assert_ne!(tbdirect::tbdirect_relay_start(std::net::UdpSocket::bind("127.0.0.1:0").unwrap().into_raw_fd(), address.as_ptr(), pinned.as_ptr()), 0);

    let identity = CString::new(diamond_fingerprint).unwrap();
    tbdirect::tbdirect_identity(identity.as_ptr());
    let reply = request_through_tunnel(50200);
    assert_eq!(String::from_utf8_lossy(&reply), "HTTP/1.1 200 OK\r\n\r\nheard GET /v1/ping HTTP/1.1");
    let first = seen.lock().unwrap().clone();
    assert_eq!(first.first(), Some(&0x16), "bronze saw a TLS handshake");
    assert!(!first.windows(4).any(|w| w == b"GET "), "bronze never saw the request");

    // Pinned to another identity: the stand-in is not Diamond, and nothing is carried.
    let other = CString::new("00".repeat(32)).unwrap();
    tbdirect::tbdirect_identity(other.as_ptr());
    let reply = request_through_tunnel(50201);
    tbdirect::tbdirect_identity(c"".as_ptr());
    tbdirect::tbdirect_relay_stop();
    assert!(reply.is_empty(), "nothing came back from a server that is not Diamond");
}
