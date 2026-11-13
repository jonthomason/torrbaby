//! Torrbaby's direct connection, the iPhone/iPad half (the server half is server/direct.rs).
//!
//! The app punches a path through both carriers' NAT with plain UDP sockets, then hands the
//! winning socket here. QUIC runs on that very socket, so the hole stays the same (iOS's own
//! QUIC would not transmit when told to reuse the port). The server's certificate is checked
//! against the fingerprint the app learned over its normal, trusted connection. A loopback
//! listener turns each connection the app or the player makes into a QUIC stream, which the
//! server pipes to its own HTTP port.
use quinn::crypto::rustls::QuicClientConfig;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use std::ffi::{CStr, c_char};
use std::net::SocketAddr;
use std::os::fd::FromRawFd;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

struct Tunnel {
    runtime: tokio::runtime::Runtime,
    connection: quinn::Connection,
}

static TUNNEL: Mutex<Option<Tunnel>> = Mutex::new(None);

#[derive(Debug)]
struct Pinned(String, Arc<rustls::crypto::CryptoProvider>);
impl ServerCertVerifier for Pinned {
    fn verify_server_cert(&self, end_entity: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        let digest: String = Sha256::digest(end_entity.as_ref()).iter().map(|b| format!("{b:02x}")).collect();
        if digest == self.0 { Ok(ServerCertVerified::assertion()) } else { Err(rustls::Error::General("Not the server's certificate".into())) }
    }
    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.1.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.1.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> { self.1.signature_verification_algorithms.supported_schemes() }
}

fn text(pointer: *const c_char) -> Option<String> {
    if pointer.is_null() { return None }
    unsafe { CStr::from_ptr(pointer) }.to_str().ok().map(str::to_owned)
}

/// Open the tunnel on the punched socket `fd` (ownership passes here) to the server at
/// `server` ("ip:port") whose certificate has SHA-256 `fingerprint`. Returns the loopback
/// port the app connects to, or 0 when QUIC could not be opened. Blocks for up to 8 s.
#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_start(fd: i32, server: *const c_char, fingerprint: *const c_char) -> u16 {
    tbdirect_stop();
    let (Some(server), Some(fingerprint)) = (text(server), text(fingerprint)) else { return 0 };
    let Ok(server): Result<SocketAddr, _> = server.parse() else { return 0 };
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() else { return 0 };
    let opened = runtime.block_on(async move {
        let socket = unsafe { std::net::UdpSocket::from_raw_fd(fd) };
        socket.set_nonblocking(true).ok()?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13]).ok()?
            .dangerous().with_custom_certificate_verifier(Arc::new(Pinned(fingerprint, provider)))
            .with_no_client_auth();
        tls.alpn_protocols = vec![b"tbdirect".to_vec()];
        let mut client = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls).ok()?));
        let mut transport = quinn::TransportConfig::default();
        transport.keep_alive_interval(Some(Duration::from_secs(10)));
        transport.max_idle_timeout(Some(Duration::from_secs(45).try_into().ok()?));
        client.transport_config(Arc::new(transport));
        let mut endpoint = quinn::Endpoint::new(quinn::EndpointConfig::default(), None, socket, Arc::new(quinn::TokioRuntime)).ok()?;
        endpoint.set_default_client_config(client);
        let connecting = endpoint.connect(server, "torrbaby-direct").ok()?;
        let connection = tokio::time::timeout(Duration::from_secs(8), connecting).await.ok()?.ok()?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.ok()?;
        let port = listener.local_addr().ok()?.port();
        let carrier = connection.clone();
        tokio::spawn(async move {
            let _endpoint = endpoint;
            while let Ok((tcp, _)) = listener.accept().await {
                let stream_carrier = carrier.clone();
                tokio::spawn(async move {
                    let Ok((send, recv)) = stream_carrier.open_bi().await else { return };
                    pipe(tcp, send, recv).await;
                });
                if carrier.close_reason().is_some() { break }
            }
        });
        Some((connection, port))
    });
    let Some((connection, port)) = opened else { runtime.shutdown_background(); return 0 };
    if let Ok(mut held) = TUNNEL.lock() { *held = Some(Tunnel { runtime, connection }) }
    port
}

async fn pipe(tcp: tokio::net::TcpStream, mut send: quinn::SendStream, mut recv: quinn::RecvStream) {
    let _ = tcp.set_nodelay(true);
    let (mut from_app, mut to_app) = tcp.into_split();
    let up = async { let _ = tokio::io::copy(&mut from_app, &mut send).await; let _ = send.finish(); };
    let down = async { let _ = tokio::io::copy(&mut recv, &mut to_app).await; let _ = to_app.shutdown().await; };
    tokio::join!(up, down);
}

/// Whether the tunnel is open.
#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_alive() -> bool {
    TUNNEL.lock().ok().and_then(|held| held.as_ref().map(|t| t.connection.close_reason().is_none())).unwrap_or(false)
}

/// Close the tunnel, if any.
#[unsafe(no_mangle)]
pub extern "C" fn tbdirect_stop() {
    let tunnel = TUNNEL.lock().ok().and_then(|mut held| held.take());
    if let Some(tunnel) = tunnel {
        tunnel.connection.close(0u32.into(), b"done");
        tunnel.runtime.shutdown_background();
    }
}
