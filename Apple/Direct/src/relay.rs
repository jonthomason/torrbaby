//! The relay through bronze, as Torrbaby reaches it: the transport only, for abt-link's route
//! (which wraps every relayed stream in TLS end to end to Diamond's identity, so bronze sees
//! ciphertext). QUIC (ALPN `tbrelay`) to Torrbaby's rendezvous on bronze, pinned to bronze's
//! certificate; where a network blocks UDP (an airport's Wi-Fi blocked 7443), TCP 443 through
//! bronze's web server, one upgraded TLS connection per stream (`/tbrelay`).
//!
//! Each relayed stream is a QUIC stream (or a TCP connection) that bronze splices to Diamond's
//! standing connection; a first line `TBTO1 <home>\n` names another home. bronze answers
//! `TBINFO1` (Diamond connected, this device's public address, Diamond's) and `TBHINTS1` (the
//! shared punch hints for this network) itself. Sockets are bound to the named interface, so a
//! VPN never carries them.
use abt_link::secure::Stream;
use quinn::crypto::rustls::QuicClientConfig;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The path bronze's web server hands to the rendezvous (the relay over TCP 443).
const TCP_PATH: &str = "/tbrelay";

#[derive(Debug)]
struct Pinned(String, Arc<rustls::crypto::CryptoProvider>);
impl ServerCertVerifier for Pinned {
    fn verify_server_cert(&self, end_entity: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        let digest: String = Sha256::digest(end_entity.as_ref()).iter().map(|b| format!("{b:02x}")).collect();
        if digest == self.0 { Ok(ServerCertVerified::assertion()) } else { Err(rustls::Error::General("Not bronze's certificate".into())) }
    }
    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.1.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.1.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> { self.1.signature_verification_algorithms.supported_schemes() }
}

/// The interface's index for binding sockets to it (0: the system's choice).
pub(crate) fn interface_index(name: &str) -> u32 {
    if name.is_empty() { return 0 }
    std::ffi::CString::new(name).map(|name| unsafe { libc::if_nametoindex(name.as_ptr()) }).unwrap_or(0)
}

#[cfg(target_vendor = "apple")]
fn bind_fd(fd: i32, v4: bool, index: u32) {
    if index == 0 { return }
    let (level, option) = if v4 { (libc::IPPROTO_IP, libc::IP_BOUND_IF) } else { (libc::IPPROTO_IPV6, libc::IPV6_BOUND_IF) };
    unsafe { libc::setsockopt(fd, level, option, &index as *const u32 as *const libc::c_void, std::mem::size_of::<u32>() as libc::socklen_t) };
}
#[cfg(not(target_vendor = "apple"))]
fn bind_fd(_: i32, _: bool, _: u32) {}

/// A UDP socket on `interface` (an index; 0: the system's choice).
fn udp_socket(v4: bool, interface: u32) -> io::Result<std::net::UdpSocket> {
    use std::os::fd::AsRawFd;
    let socket = std::net::UdpSocket::bind(if v4 { "0.0.0.0:0" } else { "[::]:0" })?;
    bind_fd(socket.as_raw_fd(), v4, interface);
    socket.set_nonblocking(true)?;
    Ok(socket)
}

struct Quic { endpoint: quinn::Endpoint, connection: quinn::Connection }

pub(crate) struct Bronze {
    /// bronze's rendezvous ("ip:7443") and its certificate's SHA-256; its web server on 443.
    server: SocketAddr,
    fingerprint: String,
    tcp_server: SocketAddr,
    /// The interface to bind sockets to (its name; empty: the system's choice).
    interface: Mutex<String>,
    quic: tokio::sync::Mutex<Option<Quic>>,
    /// The QUIC relay could not open on this network (UDP blocked): streams go over TCP 443.
    over_tcp: AtomicBool,
    /// The keeper let the relay go (nothing open for a while): the next stream opens it again.
    dormant: AtomicBool,
    lean: bool,
}

impl Bronze {
    pub(crate) fn new(server: SocketAddr, fingerprint: &str, lean: bool) -> Bronze {
        Bronze { server, fingerprint: fingerprint.trim().to_lowercase(), tcp_server: SocketAddr::new(server.ip(), 443),
            interface: Mutex::new(String::new()), quic: tokio::sync::Mutex::new(None), over_tcp: AtomicBool::new(false),
            dormant: AtomicBool::new(false), lean }
    }

    pub(crate) fn set_interface(&self, name: &str) { if let Ok(mut held) = self.interface.lock() { *held = name.to_owned() } }
    fn interface(&self) -> u32 { interface_index(&self.interface.lock().map(|i| i.clone()).unwrap_or_default()) }

    /// Whether the relay is open (the QUIC connection, or TCP mode, which opens per stream).
    pub(crate) async fn alive(&self) -> bool {
        if self.over_tcp.load(Ordering::Relaxed) { return true }
        self.quic.lock().await.as_ref().is_some_and(|q| q.connection.close_reason().is_none())
    }
    pub(crate) fn over_tcp(&self) -> bool { self.over_tcp.load(Ordering::Relaxed) }
    pub(crate) fn dormant(&self) -> bool { self.dormant.load(Ordering::Relaxed) }

    /// The QUIC connection, opened if there is none (up to 5 s). Its first stream names this
    /// device (its Torrbaby client id), for Diamond's Connection page.
    async fn connection(&self) -> io::Result<quinn::Connection> {
        let mut held = self.quic.lock().await;
        if let Some(open) = held.as_ref() && open.connection.close_reason().is_none() { return Ok(open.connection.clone()) }
        *held = None;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13]).map_err(io::Error::other)?
            .dangerous().with_custom_certificate_verifier(Arc::new(Pinned(self.fingerprint.clone(), provider)))
            .with_no_client_auth();
        tls.alpn_protocols = vec![b"tbrelay".to_vec()];
        let mut client = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls).map_err(io::Error::other)?));
        let mut transport = quinn::TransportConfig::default();
        // Keepalives every 5 s; a relay silent for 30 s is dead (a punch's spray can swamp a
        // phone hotspot for its 20 s, and the relay must outlast it).
        transport.keep_alive_interval(Some(Duration::from_secs(5)));
        transport.max_idle_timeout(Some(Duration::from_secs(30).try_into().map_err(io::Error::other)?));
        let (stream_window, window): (u32, u32) = if self.lean { (2 << 20, 4 << 20) } else { (8 << 20, 32 << 20) };
        transport.stream_receive_window(stream_window.into());
        transport.receive_window(window.into());
        client.transport_config(Arc::new(transport));
        let socket = udp_socket(self.server.is_ipv4(), self.interface())?;
        let mut endpoint = quinn::Endpoint::new(quinn::EndpointConfig::default(), None, socket, Arc::new(quinn::TokioRuntime))?;
        endpoint.set_default_client_config(client);
        let connecting = endpoint.connect(self.server, "torrbaby-rendezvous").map_err(io::Error::other)?;
        let connection = tokio::time::timeout(Duration::from_secs(5), connecting).await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "bronze's relay did not answer in 5 s"))?.map_err(io::Error::other)?;
        let client = crate::CLIENT.lock().map(|c| c.clone()).unwrap_or_default();
        if !client.is_empty() {
            let naming = connection.clone();
            tokio::spawn(async move {
                let Ok((mut send, _recv)) = naming.open_bi().await else { return };
                let _ = send.write_all(format!("TBNAME1 {client}\n").as_bytes()).await;
                let _ = send.finish();
            });
        }
        *held = Some(Quic { endpoint, connection: connection.clone() });
        self.dormant.store(false, Ordering::Relaxed);
        Ok(connection)
    }

    /// Open the relay now (the QUIC connection; where UDP is blocked, TCP 443 is checked
    /// instead). True when it is open.
    pub(crate) async fn open(&self) -> bool {
        if !self.over_tcp() {
            if self.connection().await.is_ok() { return true }
        }
        // UDP to bronze did not get through: the relay over TCP, if bronze answers there.
        let mut probe = match self.tcp_stream().await { Ok(stream) => stream, Err(_) => return false };
        let answered = probe.write_all(b"TBINFO1\n").await.is_ok() && {
            let mut reply = Vec::new();
            let _ = tokio::time::timeout(Duration::from_secs(5), (&mut probe).take(256).read_to_end(&mut reply)).await;
            !reply.is_empty()
        };
        if answered { self.over_tcp.store(true, Ordering::Relaxed); self.dormant.store(false, Ordering::Relaxed) }
        answered
    }

    /// One raw relayed stream to the home called `host` ("diamond": bronze's default).
    pub(crate) async fn stream(&self, host: &str) -> io::Result<Box<dyn Stream>> {
        let mut stream: Box<dyn Stream> = if self.over_tcp() {
            Box::new(self.tcp_stream().await?)
        } else {
            match self.connection().await {
                Ok(connection) => {
                    let (send, recv) = connection.open_bi().await.map_err(io::Error::other)?;
                    // Small requests and replies ahead of a film's bulk on the same connection.
                    Box::new(tokio::io::join(recv, abt_link::priority::Prioritised::new(send)))
                }
                Err(error) => {
                    // UDP blocked here: TCP 443 from now on, if bronze answers there.
                    if !self.open().await || !self.over_tcp() { return Err(error) }
                    Box::new(self.tcp_stream().await?)
                }
            }
        };
        if host != "diamond" { stream.write_all(format!("TBTO1 {host}\n").as_bytes()).await? }
        Ok(stream)
    }

    /// One TCP connection to bronze's web server, through TLS and upgraded: one relayed stream.
    async fn tcp_stream(&self) -> io::Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>> {
        use std::os::fd::AsRawFd;
        let server = self.tcp_server;
        let socket = if server.is_ipv4() { tokio::net::TcpSocket::new_v4()? } else { tokio::net::TcpSocket::new_v6()? };
        bind_fd(socket.as_raw_fd(), server.is_ipv4(), self.interface());
        let tcp = tokio::time::timeout(Duration::from_secs(6), socket.connect(server)).await.map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
        let _ = tcp.set_nodelay(true);
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        let mut tls = rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions().map_err(io::Error::other)?
            .with_root_certificates(roots).with_no_client_auth();
        // HTTP/1.1, which can upgrade (HTTP/2 cannot).
        tls.alpn_protocols = vec![b"http/1.1".to_vec()];
        let name = ServerName::IpAddress(server.ip().into());
        let connector = tokio_rustls::TlsConnector::from(Arc::new(tls));
        let mut stream = tokio::time::timeout(Duration::from_secs(6), connector.connect(name, tcp)).await.map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
        let client = crate::CLIENT.lock().map(|c| c.clone()).unwrap_or_default();
        let host = match server.ip() { std::net::IpAddr::V6(ip) => format!("[{ip}]"), ip => ip.to_string() };
        let request = format!("GET {TCP_PATH} HTTP/1.1\r\nHost: {host}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nX-Torrbaby-Client: {client}\r\n\r\n");
        stream.write_all(request.as_bytes()).await?;
        stream.flush().await?;
        // The response's head, a byte at a time so nothing after it is taken.
        let mut head = Vec::new();
        tokio::time::timeout(Duration::from_secs(6), async {
            while !head.ends_with(b"\r\n\r\n") {
                if head.len() > 4096 { return Err(io::Error::new(io::ErrorKind::InvalidData, "response head too long")) }
                head.push(stream.read_u8().await?);
            }
            Ok(())
        }).await.map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
        let status = String::from_utf8_lossy(&head).lines().next().unwrap_or("").to_owned();
        if status.split(' ').nth(1) != Some("101") { return Err(io::Error::other(format!("bronze answered {status}"))) }
        Ok(stream)
    }

    /// bronze's own answer to `request` (TBINFO1, TBHINTS1), within 4 s.
    pub(crate) async fn ask(&self, request: &[u8]) -> Option<Vec<u8>> {
        tokio::time::timeout(Duration::from_secs(4), async {
            let mut stream = self.stream("diamond").await.ok()?;
            stream.write_all(request).await.ok()?;
            let _ = stream.flush().await;
            // QUIC: the request ends the stream's sending side; bronze answers and closes.
            let mut reply = Vec::new();
            let _ = (&mut stream).take(1 << 16).read_to_end(&mut reply).await;
            (!reply.is_empty()).then_some(reply)
        }).await.ok().flatten()
    }

    /// The network changed: the QUIC relay moves onto a socket on the new one (QUIC migration;
    /// bronze has a public address and answers the new one). False when it cannot move.
    pub(crate) async fn rebind(&self) -> bool {
        self.over_tcp.store(false, Ordering::Relaxed);
        let held = self.quic.lock().await;
        let Some(open) = held.as_ref().filter(|q| q.connection.close_reason().is_none()) else { return false };
        let Ok(socket) = udp_socket(self.server.is_ipv4(), self.interface()) else { return false };
        open.endpoint.rebind(socket).is_ok()
    }

    /// Let the relay go (nothing open for a while): no keepalives until the next stream.
    pub(crate) async fn close(&self, dormant: bool) {
        if let Some(open) = self.quic.lock().await.take() { open.connection.close(0u32.into(), b"idle") }
        self.dormant.store(dormant, Ordering::Relaxed);
    }

    /// The QUIC relay's round trip, congestion window, loss and bytes, for the status line.
    pub(crate) async fn line(&self) -> String {
        if self.over_tcp() { return "relay over TCP 443".into() }
        let held = self.quic.lock().await;
        let Some(open) = held.as_ref().filter(|q| q.connection.close_reason().is_none()) else { return "relay closed".into() };
        let stats = open.connection.stats();
        format!("relay rtt {} ms, cwnd {} KB, lost {} pkts, sent {} KB, received {} KB",
            stats.path.rtt.as_millis(), stats.path.cwnd / 1024, stats.path.lost_packets, stats.udp_tx.bytes / 1024, stats.udp_rx.bytes / 1024)
    }
}

/// abt-link's route reaches Diamond through bronze with this.
pub(crate) struct BronzeRelay(pub(crate) Arc<Bronze>);
impl abt_link::route::Relay for BronzeRelay {
    fn open<'a>(&'a self, host: &'a str) -> abt_link::route::BoxFuture<'a, io::Result<Box<dyn Stream>>> {
        Box::pin(async move { self.0.stream(host).await })
    }
}
