//! The relay over TCP against the real bronze: TBINFO, then /v1/ping to Diamond through it.
//!   cargo run --example tcp_relay -- 47.112.18.69:443 [interface]
use std::ffi::CString;
use std::io::{Read, Write};
fn main() {
    let mut args = std::env::args().skip(1);
    let server = CString::new(args.next().unwrap_or("47.112.18.69:443".into())).unwrap();
    let interface = CString::new(args.next().unwrap_or_default()).unwrap();
    let started = std::time::Instant::now();
    let port = tbdirect::tcp::tbdirect_relay_tcp_start(server.as_ptr(), interface.as_ptr());
    println!("open: port {port} in {} ms", started.elapsed().as_millis());
    if port == 0 { std::process::exit(1) }
    let mut buffer = [0 as std::ffi::c_char; 256];
    let n = tbdirect::tbdirect_relay_info(buffer.as_mut_ptr(), buffer.len());
    println!("info ({n}): {:?}", unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) });
    for _ in 0..3 {
        let started = std::time::Instant::now();
        let mut tcp = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        tcp.write_all(b"GET /v1/ping HTTP/1.1\r\nHost: diamond\r\nConnection: close\r\n\r\n").unwrap();
        let mut reply = String::new();
        let _ = tcp.read_to_string(&mut reply);
        println!("ping in {} ms: {}", started.elapsed().as_millis(), reply.lines().next().unwrap_or("(nothing)"));
    }
    println!("alive {}", tbdirect::tcp::tbdirect_relay_tcp_alive());
}
