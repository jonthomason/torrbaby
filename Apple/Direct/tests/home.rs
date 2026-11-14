//! At home: a tunnel connection goes straight to Diamond on this network as plain TCP, with
//! no QUIC path open at all, so the tunnel's address works at home too.
use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, Ipv4Address};
use sha2::Digest;
use std::{ffi::CString, os::fd::IntoRawFd, sync::{Arc, Mutex, mpsc}, time::{Duration, Instant}};
/// The tunnel and its routes are process-wide: the tests take turns.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

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
fn at_home_a_tunnel_connection_reaches_diamond_on_this_network() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    // Diamond's stand-in on "this network": a plain TCP server answering with 2 MB.
    let body: Vec<u8> = (0..2_000_000u32).map(|i| (i % 251) as u8).collect();
    let expected = body.clone();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        let (mut connection, _) = listener.accept().unwrap();
        let mut request = [0u8; 64];
        let _ = connection.read(&mut request);
        connection.write_all(&body).unwrap();
    });
    let home = CString::new(address.to_string()).unwrap();
    tbdirect::tbtunnel_home(home.as_ptr());

    let (sender, incoming) = mpsc::channel();
    *FROM_STACK.lock().unwrap() = Some(sender);
    assert!(tbdirect::tbtunnel_start(1280, written, std::ptr::null_mut()));

    let mut wire = Wire { incoming, outgoing: Vec::new() };
    let mut interface = Interface::new(Config::new(HardwareAddress::Ip), &mut wire, smoltcp::time::Instant::now());
    interface.update_ip_addrs(|addresses| { addresses.push(IpCidr::new(IpAddress::v4(10, 233, 0, 2), 32)).unwrap(); });
    interface.routes_mut().add_default_ipv4_route(Ipv4Address::new(10, 233, 0, 1)).unwrap();
    let socket = tcp::Socket::new(tcp::SocketBuffer::new(vec![0; 256 * 1024]), tcp::SocketBuffer::new(vec![0; 16 * 1024]));
    let mut sockets = SocketSet::new(vec![]);
    let handle = sockets.add(socket);
    sockets.get_mut::<tcp::Socket>(handle).connect(interface.context(), (IpAddress::v4(10, 233, 0, 1), 8765), 49152).unwrap();
    let (mut sent, mut received) = (false, Vec::new());
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        interface.poll(smoltcp::time::Instant::now(), &mut wire, &mut sockets);
        for packet in wire.outgoing.drain(..) { assert!(tbdirect::tbtunnel_input(packet.as_ptr(), packet.len())); }
        let socket = sockets.get_mut::<tcp::Socket>(handle);
        if socket.may_send() && !sent { socket.send_slice(b"GET /download HTTP/1.1\r\n\r\n").unwrap(); sent = true; }
        while socket.can_recv() { socket.recv(|data| { received.extend_from_slice(data); (data.len(), ()) }).unwrap(); }
        if sent && !socket.may_recv() { break }
        std::thread::sleep(Duration::from_millis(1));
    }
    let closed = Instant::now() < deadline;
    let mut stats = vec![0 as std::ffi::c_char; 768];
    tbdirect::tbtunnel_stats(stats.as_mut_ptr(), stats.len());
    eprintln!("{}", unsafe { std::ffi::CStr::from_ptr(stats.as_ptr()) }.to_string_lossy());
    tbdirect::tbtunnel_stop();
    tbdirect::tbtunnel_home(c"".as_ptr());
    assert_eq!(received.len(), expected.len(), "the whole reply came back through the packets");
    assert!(received == expected);
    assert!(closed, "the connection should close once the reply is complete");
}

/// Arriving home mid-download: the connection on the relay is cut once Diamond on this network
/// is known, and the client's next request (a download resuming) goes there instead.
#[test]
fn arriving_home_moves_a_relay_download_to_diamond_on_this_network() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    // bronze's stand-in: a relay that answers each stream with an endless trickle (a long download).
    let made = rcgen::generate_simple_self_signed(vec!["torrbaby-rendezvous".into()]).unwrap();
    let cert = made.cert.der().clone();
    let fingerprint: String = sha2::Sha256::digest(cert.as_ref()).iter().map(|b| format!("{b:02x}")).collect();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(made.key_pair.serialize_der().into());
    let relay = runtime.block_on(async move {
        let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13]).unwrap().with_no_client_auth().with_single_cert(vec![cert], key).unwrap();
        tls.alpn_protocols = vec![b"tbrelay".to_vec()];
        let config = quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap()));
        let endpoint = quinn::Endpoint::server(config, "127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        tokio::spawn(async move {
            let connection = endpoint.accept().await.unwrap().await.unwrap();
            while let Ok((mut send, mut recv)) = connection.accept_bi().await {
                tokio::spawn(async move {
                    let mut request = [0u8; 64];
                    let _ = recv.read(&mut request).await;
                    let chunk = vec![1u8; 16 * 1024];
                    loop {
                        if send.write_all(&chunk).await.is_err() { return }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                });
            }
        });
        address
    });
    let address = CString::new(relay.to_string()).unwrap();
    let pinned = CString::new(fingerprint).unwrap();
    assert_ne!(tbdirect::tbdirect_relay_start(std::net::UdpSocket::bind("127.0.0.1:0").unwrap().into_raw_fd(), address.as_ptr(), pinned.as_ptr()), 0);

    // Diamond on "this network": answers with 2's.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let home = CString::new(listener.local_addr().unwrap().to_string()).unwrap();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        while let Ok((mut connection, _)) = listener.accept() {
            let mut request = [0u8; 64];
            let _ = connection.read(&mut request);
            let _ = connection.write_all(&vec![2u8; 256 * 1024]);
        }
    });

    let (sender, incoming) = mpsc::channel();
    *FROM_STACK.lock().unwrap() = Some(sender);
    assert!(tbdirect::tbtunnel_start(1280, written, std::ptr::null_mut()));
    let mut wire = Wire { incoming, outgoing: Vec::new() };
    let mut interface = Interface::new(Config::new(HardwareAddress::Ip), &mut wire, smoltcp::time::Instant::now());
    interface.update_ip_addrs(|addresses| { addresses.push(IpCidr::new(IpAddress::v4(10, 233, 0, 2), 32)).unwrap(); });
    interface.routes_mut().add_default_ipv4_route(Ipv4Address::new(10, 233, 0, 1)).unwrap();
    let mut sockets = SocketSet::new(vec![]);
    let new_socket = || tcp::Socket::new(tcp::SocketBuffer::new(vec![0; 256 * 1024]), tcp::SocketBuffer::new(vec![0; 4 * 1024]));
    let first = sockets.add(new_socket());
    sockets.get_mut::<tcp::Socket>(first).connect(interface.context(), (IpAddress::v4(10, 233, 0, 1), 8765), 50100).unwrap();
    let (mut first_sent, mut from_relay, mut home_set_at) = (false, 0usize, None);
    let mut second: Option<smoltcp::iface::SocketHandle> = None;
    let (mut second_sent, mut from_home) = (false, Vec::new());
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        interface.poll(smoltcp::time::Instant::now(), &mut wire, &mut sockets);
        for packet in wire.outgoing.drain(..) { tbdirect::tbtunnel_input(packet.as_ptr(), packet.len()); }
        {
            let socket = sockets.get_mut::<tcp::Socket>(first);
            if socket.may_send() && !first_sent { socket.send_slice(b"GET /download HTTP/1.1\r\n\r\n").unwrap(); first_sent = true; }
            while socket.can_recv() { socket.recv(|data| { assert!(data.iter().all(|b| *b == 1)); from_relay += data.len(); (data.len(), ()) }).unwrap(); }
            // The download is under way on the relay: the phone arrives home.
            if from_relay > 64 * 1024 && home_set_at.is_none() { tbdirect::tbtunnel_home(home.as_ptr()); home_set_at = Some(Instant::now()); }
            // The relay connection was cut: the client asks again, as a download resuming does.
            if home_set_at.is_some() && !socket.may_recv() && second.is_none() {
                let handle = sockets.add(new_socket());
                sockets.get_mut::<tcp::Socket>(handle).connect(interface.context(), (IpAddress::v4(10, 233, 0, 1), 8765), 50101).unwrap();
                second = Some(handle);
            }
        }
        if let Some(handle) = second {
            let socket = sockets.get_mut::<tcp::Socket>(handle);
            if socket.may_send() && !second_sent { socket.send_slice(b"GET /download HTTP/1.1\r\n\r\n").unwrap(); second_sent = true; }
            while socket.can_recv() { socket.recv(|data| { from_home.extend_from_slice(data); (data.len(), ()) }).unwrap(); }
            if from_home.len() >= 256 * 1024 { break }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let cut_after = home_set_at.map(|at| at.elapsed());
    tbdirect::tbtunnel_stop();
    tbdirect::tbdirect_relay_stop();
    tbdirect::tbtunnel_home(c"".as_ptr());
    eprintln!("relay {from_relay} bytes, then home {} bytes ({cut_after:?} after arriving home)", from_home.len());
    assert!(from_relay > 64 * 1024, "the download started on the relay");
    assert!(second.is_some(), "the relay connection was cut after arriving home");
    assert_eq!(from_home.len(), 256 * 1024, "the next request was answered by Diamond on this network");
    assert!(from_home.iter().all(|b| *b == 2));
}
