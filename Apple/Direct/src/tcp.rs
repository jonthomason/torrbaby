//! The relay over TCP 443, for networks that block UDP (an airport's Wi-Fi blocked bronze's
//! UDP 7443, so the QUIC relay could not open). bronze's web server (Caddy) owns 443 with a
//! Let's Encrypt certificate for bronze's IP; a request for `/tbrelay` asking to upgrade is
//! handed to tbrendezvous, and after the `101` the connection is one relayed stream, spliced
//! to Diamond exactly as a QUIC relay stream is (rendezvous/src/lib.rs). Inside it the
//! stream is TLS end to end to Diamond's pinned identity (crate::e2e), as on the QUIC relay.
//!
//! One TCP connection per relayed stream: each costs a TCP and TLS handshake (resumed) and the
//! upgrade, a few round trips more than a QUIC stream. Used only when the QUIC relay cannot
//! open. Sockets are bound to the physical interface, so another VPN never carries them.
use std::ffi::c_char;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::client::TlsStream;

pub const PATH: &str = "/tbrelay";
/// Upgrades that fail in a row before the relay counts as closed (the app then finds a route again).
const FAILURES_TO_CLOSE: u32 = 3;

pub(crate) struct TcpRelay {
    runtime: tokio::runtime::Runtime,
    link: Arc<Link>,
    dead: Arc<AtomicBool>,
}

/// Where and how to open one relayed stream.
struct Link {
    server: SocketAddr,
    interface: u32,
    tls: Arc<rustls::ClientConfig>,
}

pub(crate) static TCP_RELAY: Mutex<Option<TcpRelay>> = Mutex::new(None);

fn tls() -> Option<Arc<rustls::ClientConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let mut tls = rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions().ok()?
        .with_root_certificates(roots).with_no_client_auth();
    // HTTP/1.1, which can upgrade (HTTP/2 cannot).
    tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    Some(Arc::new(tls))
}

impl Link {
    /// A TCP connection to bronze, through TLS and upgraded: one relayed stream.
    async fn open(&self) -> std::io::Result<TlsStream<tokio::net::TcpStream>> {
        use std::io::{Error, ErrorKind};
        let socket = if self.server.is_ipv4() { tokio::net::TcpSocket::new_v4()? } else { tokio::net::TcpSocket::new_v6()? };
        bind_interface(&socket, self.server.is_ipv4(), self.interface);
        let tcp = tokio::time::timeout(Duration::from_secs(6), socket.connect(self.server)).await.map_err(|_| Error::from(ErrorKind::TimedOut))??;
        let _ = tcp.set_nodelay(true);
        let name = rustls::pki_types::ServerName::IpAddress(self.server.ip().into());
        let connector = tokio_rustls::TlsConnector::from(self.tls.clone());
        let mut stream = tokio::time::timeout(Duration::from_secs(6), connector.connect(name, tcp)).await.map_err(|_| Error::from(ErrorKind::TimedOut))??;
        let client = crate::CLIENT.lock().map(|c| c.clone()).unwrap_or_default();
        let host = match self.server.ip() { std::net::IpAddr::V6(ip) => format!("[{ip}]"), ip => ip.to_string() };
        let request = format!("GET {PATH} HTTP/1.1\r\nHost: {host}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nX-Torrbaby-Client: {client}\r\n\r\n");
        stream.write_all(request.as_bytes()).await?;
        stream.flush().await?;
        // The response's head, a byte at a time so nothing after it is taken.
        let mut head = Vec::new();
        tokio::time::timeout(Duration::from_secs(6), async {
            while !head.ends_with(b"\r\n\r\n") {
                if head.len() > 4096 { return Err(Error::new(ErrorKind::InvalidData, "response head too long")) }
                head.push(stream.read_u8().await?);
            }
            Ok(())
        }).await.map_err(|_| Error::from(ErrorKind::TimedOut))??;
        let status = String::from_utf8_lossy(&head).lines().next().unwrap_or("").to_owned();
        if status.split(' ').nth(1) != Some("101") { return Err(Error::other(format!("bronze answered {status}"))) }
        Ok(stream)
    }

    /// bronze's TBINFO reply ("home online\nyou 1.2.3.4:0\n…").
    async fn info(&self) -> Option<Vec<u8>> {
        tokio::time::timeout(Duration::from_secs(8), async {
            let mut stream = self.open().await.ok()?;
            stream.write_all(tbrelay_info()).await.ok()?;
            stream.flush().await.ok()?;
            let mut reply = Vec::new();
            let _ = (&mut stream).take(256).read_to_end(&mut reply).await;
            (!reply.is_empty()).then_some(reply)
        }).await.ok().flatten()
    }
}

fn tbrelay_info() -> &'static [u8] { b"TBINFO1\n" }

#[cfg(target_vendor = "apple")]
fn bind_interface(socket: &tokio::net::TcpSocket, v4: bool, index: u32) {
    use std::os::fd::AsRawFd;
    if index == 0 { return }
    let (level, option) = if v4 { (libc::IPPROTO_IP, libc::IP_BOUND_IF) } else { (libc::IPPROTO_IPV6, libc::IPV6_BOUND_IF) };
    unsafe { libc::setsockopt(socket.as_raw_fd(), level, option, &index as *const u32 as *const libc::c_void, std::mem::size_of::<u32>() as libc::socklen_t) };
}

#[cfg(not(target_vendor = "apple"))]
fn bind_interface(_: &tokio::net::TcpSocket, _: bool, _: u32) {}

/// Open the relay over TCP to bronze at `server` ("ip:443"), its sockets bound to `interface`
/// (a name such as "en0"; empty: the system's choice). bronze is asked once whether it
/// answers; then each loopback connection becomes one relayed stream. Returns the loopback
/// port, or 0 when bronze did not answer. Blocks for up to 8 s.
#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_relay_tcp_start(server: *const c_char, interface: *const c_char) -> u16 {
    tbdirect_relay_tcp_stop();
    let Some(Ok(server)) = crate::text(server).map(|s| s.parse::<SocketAddr>()) else { return 0 };
    let interface = crate::text(interface).filter(|i| !i.is_empty()).and_then(|name| std::ffi::CString::new(name).ok())
        .map(|name| unsafe { libc::if_nametoindex(name.as_ptr()) }).unwrap_or(0);
    start(server, interface)
}

pub(crate) fn start(server: SocketAddr, interface: u32) -> u16 {
    let Some(tls) = tls() else { return 0 };
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() else { return 0 };
    let link = Arc::new(Link { server, interface, tls });
    let dead = Arc::new(AtomicBool::new(false));
    let (serving, flag) = (link.clone(), dead.clone());
    let port = runtime.block_on(async move {
        // bronze must answer before the relay counts as open.
        serving.info().await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.ok()?;
        let port = listener.local_addr().ok()?.port();
        let failures = Arc::new(AtomicU32::new(0));
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (link, failures, dead) = (serving.clone(), failures.clone(), flag.clone());
                tokio::spawn(async move {
                    let stream = match link.open().await {
                        Ok(stream) => { failures.store(0, Ordering::Relaxed); stream }
                        Err(_) => {
                            if failures.fetch_add(1, Ordering::Relaxed) + 1 >= FAILURES_TO_CLOSE { dead.store(true, Ordering::Relaxed) }
                            return;
                        }
                    };
                    match crate::e2e::wrap_io(stream).await {
                        crate::e2e::WrappedIo::Encrypted(stream) => { let (from, to) = tokio::io::split(stream); crate::pipe(tcp, to, from).await }
                        crate::e2e::WrappedIo::Plain(stream) => {
                            let (from, to) = tokio::io::split(stream);
                            tokio::select! { _ = crate::pipe(tcp, to, from) => {}, _ = crate::e2e::pinned_now() => {} }
                        }
                        crate::e2e::WrappedIo::Failed => {}
                    }
                });
            }
        });
        Some(port)
    });
    let Some(port) = port else { runtime.shutdown_background(); return 0 };
    if let Ok(mut held) = TCP_RELAY.lock() { *held = Some(TcpRelay { runtime, link, dead }) }
    port
}

/// Whether the relay over TCP is open: started, and its last upgrades did not all fail.
#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_relay_tcp_alive() -> bool {
    TCP_RELAY.lock().ok().is_some_and(|held| held.as_ref().is_some_and(|r| !r.dead.load(Ordering::Relaxed)))
}

#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_relay_tcp_stop() {
    let relay = TCP_RELAY.lock().ok().and_then(|mut held| held.take());
    if let Some(relay) = relay { relay.runtime.shutdown_background() }
}

/// bronze's TBINFO reply over the relay over TCP, if it is open. Blocks up to 8 s; not to be
/// called from inside an async task.
pub(crate) fn info() -> Option<Vec<u8>> {
    let held = TCP_RELAY.lock().ok().and_then(|held| held.as_ref().map(|r| (r.runtime.handle().clone(), r.link.clone())));
    let (handle, link) = held?;
    handle.block_on(async move { link.info().await })
}
