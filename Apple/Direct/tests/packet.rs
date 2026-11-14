//! The packet tunnel: a TCP connection made of raw IP packets (as iOS hands them to the
//! Network Extension) reaches a Diamond stand-in as a QUIC stream, and a large reply comes
//! back as packets. The client side is smoltcp, playing iOS's TCP.
use sha2::Digest;
use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, Ipv4Address};
use std::{ffi::CString, os::fd::IntoRawFd, sync::{Arc, Mutex, mpsc}, time::{Duration, Instant}};

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

#[test]
fn a_tcp_connection_in_packets_reaches_diamond_as_a_quic_stream() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    // Diamond's stand-in: answers a request with 2 MB, as a download would.
    let made = rcgen::generate_simple_self_signed(vec!["torrbaby-direct".into()]).unwrap();
    let cert = made.cert.der().clone();
    let fingerprint: String = sha2::Sha256::digest(cert.as_ref()).iter().map(|b| format!("{b:02x}")).collect();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(made.key_pair.serialize_der().into());
    let body: Vec<u8> = (0..2_000_000u32).map(|i| (i % 251) as u8).collect();
    let expected = body.clone();
    let server = runtime.block_on(async move {
        let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13]).unwrap().with_no_client_auth().with_single_cert(vec![cert], key).unwrap();
        tls.alpn_protocols = vec![b"tbdirect".to_vec()];
        let config = quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap()));
        let endpoint = quinn::Endpoint::server(config, "127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let body = Arc::new(body);
        tokio::spawn(async move {
            let connection = endpoint.accept().await.unwrap().await.unwrap();
            while let Ok((mut send, mut recv)) = connection.accept_bi().await {
                let body = body.clone();
                tokio::spawn(async move {
                    let mut request = [0u8; 64];
                    let _ = recv.read(&mut request).await;
                    send.write_all(&body).await.unwrap();
                    send.finish().unwrap();
                });
            }
        });
        address
    });
    let address = CString::new(server.to_string()).unwrap();
    let pinned = CString::new(fingerprint).unwrap();
    assert_ne!(tbdirect::tbdirect_start(std::net::UdpSocket::bind("127.0.0.1:0").unwrap().into_raw_fd(), address.as_ptr(), pinned.as_ptr()), 0);

    let (sender, incoming) = mpsc::channel();
    *FROM_STACK.lock().unwrap() = Some(sender);
    assert!(tbdirect::tbtunnel_start(1280, written, std::ptr::null_mut()));

    // iOS's side of the tunnel: 10.233.0.2, talking to Diamond's tunnel address 10.233.0.1.
    let mut wire = Wire { incoming, outgoing: Vec::new() };
    let mut interface = Interface::new(Config::new(HardwareAddress::Ip), &mut wire, smoltcp::time::Instant::now());
    interface.update_ip_addrs(|addresses| { addresses.push(IpCidr::new(IpAddress::v4(10, 233, 0, 2), 32)).unwrap(); });
    interface.routes_mut().add_default_ipv4_route(Ipv4Address::new(10, 233, 0, 1)).unwrap();
    let socket = tcp::Socket::new(tcp::SocketBuffer::new(vec![0; 256 * 1024]), tcp::SocketBuffer::new(vec![0; 16 * 1024]));
    let mut sockets = SocketSet::new(vec![]);
    let handle = sockets.add(socket);
    sockets.get_mut::<tcp::Socket>(handle).connect(interface.context(), (IpAddress::v4(10, 233, 0, 1), 8765), 49152).unwrap();
    let mut sent = false;
    let mut received = Vec::new();
    let begun = Instant::now();
    let deadline = begun + Duration::from_secs(20);
    let mut complete_at = None;
    while Instant::now() < deadline {
        interface.poll(smoltcp::time::Instant::now(), &mut wire, &mut sockets);
        for packet in wire.outgoing.drain(..) { assert!(tbdirect::tbtunnel_input(packet.as_ptr(), packet.len())); }
        let socket = sockets.get_mut::<tcp::Socket>(handle);
        if socket.may_send() && !sent { socket.send_slice(b"GET /download HTTP/1.1\r\n\r\n").unwrap(); sent = true; }
        while socket.can_recv() { socket.recv(|data| { received.extend_from_slice(data); (data.len(), ()) }).unwrap(); }
        if received.len() >= expected.len() && complete_at.is_none() { complete_at = Some(begun.elapsed()); }
        // The stream's end reaches iOS's side as a FIN.
        if sent && !socket.may_recv() { break }
        std::thread::sleep(Duration::from_millis(1));
    }
    let closed = Instant::now() < deadline;
    eprintln!("2 MB in {:?}, closed: {closed}", complete_at);
    tbdirect::tbtunnel_stop();
    tbdirect::tbdirect_stop();
    assert_eq!(received.len(), expected.len(), "the whole reply came back through the packets");
    assert!(received == expected);
    assert!(closed, "the connection should close once the reply is complete");
}

/// As a video player does: many connections at once, half of them cut off mid-transfer (RST).
/// The stack must keep carrying the rest, and nothing may panic.
#[test]
fn many_connections_with_half_aborted_mid_transfer() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let made = rcgen::generate_simple_self_signed(vec!["torrbaby-direct".into()]).unwrap();
    let cert = made.cert.der().clone();
    let fingerprint: String = sha2::Sha256::digest(cert.as_ref()).iter().map(|b| format!("{b:02x}")).collect();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(made.key_pair.serialize_der().into());
    let server = runtime.block_on(async move {
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
                    let mut request = [0u8; 64];
                    let _ = recv.read(&mut request).await;
                    let chunk = vec![7u8; 64 * 1024];
                    for _ in 0..64 { if send.write_all(&chunk).await.is_err() { return } }
                    let _ = send.finish();
                });
            }
        });
        address
    });
    let address = CString::new(server.to_string()).unwrap();
    let pinned = CString::new(fingerprint).unwrap();
    assert_ne!(tbdirect::tbdirect_start_lean(std::net::UdpSocket::bind("127.0.0.1:0").unwrap().into_raw_fd(), address.as_ptr(), pinned.as_ptr()), 0);
    let (sender, incoming) = mpsc::channel();
    *FROM_STACK.lock().unwrap() = Some(sender);
    assert!(tbdirect::tbtunnel_start(1280, written, std::ptr::null_mut()));
    let mut wire = Wire { incoming, outgoing: Vec::new() };
    let mut interface = Interface::new(Config::new(HardwareAddress::Ip), &mut wire, smoltcp::time::Instant::now());
    interface.update_ip_addrs(|addresses| { addresses.push(IpCidr::new(IpAddress::v4(10, 233, 0, 2), 32)).unwrap(); });
    interface.routes_mut().add_default_ipv4_route(Ipv4Address::new(10, 233, 0, 1)).unwrap();
    let mut sockets = SocketSet::new(vec![]);
    let handles: Vec<_> = (0..16u16).map(|i| {
        let mut socket = tcp::Socket::new(tcp::SocketBuffer::new(vec![0; 64 * 1024]), tcp::SocketBuffer::new(vec![0; 4 * 1024]));
        socket.connect(interface.context(), (IpAddress::v4(10, 233, 0, 1), 8765), 50000 + i).unwrap();
        sockets.add(socket)
    }).collect();
    let mut received = vec![0usize; handles.len()];
    let mut sent = vec![false; handles.len()];
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        interface.poll(smoltcp::time::Instant::now(), &mut wire, &mut sockets);
        for packet in wire.outgoing.drain(..) { tbdirect::tbtunnel_input(packet.as_ptr(), packet.len()); }
        for (i, handle) in handles.iter().enumerate() {
            let socket = sockets.get_mut::<tcp::Socket>(*handle);
            if socket.may_send() && !sent[i] { socket.send_slice(b"GET /video HTTP/1.1\r\n\r\n").unwrap(); sent[i] = true; }
            while socket.can_recv() { socket.recv(|data| { received[i] += data.len(); (data.len(), ()) }).unwrap(); }
            // Odd connections are abandoned after 256 KB, as a player seeking away does.
            if i % 2 == 1 && received[i] > 256 * 1024 && socket.is_open() { socket.abort(); }
        }
        let done = (0..handles.len()).all(|i| if i % 2 == 1 { received[i] > 256 * 1024 } else { received[i] >= 4 * 1024 * 1024 });
        if done { break }
        std::thread::sleep(Duration::from_micros(200));
    }
    let alive = tbdirect::tbtunnel_alive();
    eprintln!("received {:?}, alive {alive}", received.iter().map(|n| n / 1024).collect::<Vec<_>>());
    tbdirect::tbtunnel_stop();
    tbdirect::tbdirect_stop();
    for i in (0..handles.len()).step_by(2) { assert_eq!(received[i], 4 * 1024 * 1024, "connection {i} should get its whole 4 MB"); }
    assert!(alive, "the stack is still running");
}
