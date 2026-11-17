//! The packet tunnel: a TCP connection made of raw IP packets (as iOS hands them to the
//! Network Extension) reaches a Diamond stand-in through Torrbaby's link (the relay, end to
//! end), and a large reply comes back as packets. The client side is smoltcp, playing iOS's TCP.
mod common;
use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, Ipv4Address};
use std::{ffi::CString, sync::{Arc, Mutex, mpsc}, time::{Duration, Instant}};

static FROM_STACK: Mutex<Option<mpsc::Sender<Vec<u8>>>> = Mutex::new(None);
/// The tunnel, its stack and its paths are process-wide: the tests take turns.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
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
fn a_tcp_connection_in_packets_reaches_diamond_through_the_link() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    // Diamond answers /download with 2 MB, as a download would.
    let body: Vec<u8> = (0..2_000_000u32).map(|i| (i % 251) as u8).collect();
    let expected = body.clone();
    let setup = common::bronze_and_diamond(&runtime, "packet", Arc::new(body));
    let pinned = CString::new(setup.diamond.clone()).unwrap();
    tbdirect::tbdirect_identity(pinned.as_ptr());
    let mut config = tbdirect::proxy::Config::new(&setup.rendezvous.to_string(), &setup.bronze);
    config.lean = true;
    assert_ne!(tbdirect::proxy::start(config), 0, "the link opens");

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
    tbdirect::proxy::stop();
    assert_eq!(received.len(), expected.len(), "the whole reply came back through the packets");
    assert!(received == expected);
    assert!(closed, "the connection should close once the reply is complete");
}
