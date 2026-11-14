//! End-to-end encryption through bronze. bronze splices each relayed stream to Diamond's
//! standing connection, so it could read them: each relayed stream is therefore wrapped in TLS
//! to Diamond's own identity, pinned by the fingerprint the app learned from pairing or from
//! Diamond on the home network (never through bronze). Diamond decrypts it (server/direct.rs).
//! The direct path, QUIC to the same identity, is pinned to it too.
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// ALPN of the TLS inside a relayed stream (server/direct.rs `E2E_ALPN`).
const ALPN: &[u8] = b"tbe2e";

/// Diamond's identity fingerprint, and the TLS configuration pinned to it (one, kept, so
/// sessions resume).
static PINNED: Mutex<Option<(String, Arc<rustls::ClientConfig>)>> = Mutex::new(None);

/// Pin Diamond's identity (64 hex characters), or forget it (empty: relayed streams go plain,
/// as before, for a device that has not yet learned it).
pub fn pin(fingerprint: &str) {
    let fingerprint = fingerprint.trim().to_lowercase();
    let Ok(mut held) = PINNED.lock() else { return };
    if fingerprint.len() != 64 { *held = None; return }
    if held.as_ref().is_some_and(|(pinned, _)| *pinned == fingerprint) { return }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let Ok(builder) = rustls::ClientConfig::builder_with_provider(provider.clone()).with_protocol_versions(&[&rustls::version::TLS13]) else { return };
    let mut tls = builder.dangerous().with_custom_certificate_verifier(Arc::new(crate::Pinned(fingerprint.clone(), provider))).with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    *held = Some((fingerprint, Arc::new(tls)));
}

/// Resolves once an identity is pinned: a plain relayed connection (opened before the device
/// knew Diamond's identity, as pairing's own requests are) is then closed, so the client's
/// keep-alive connection cannot go on carrying requests and credentials unencrypted; it
/// reconnects, encrypted.
pub async fn pinned_now() {
    loop {
        if pinned().is_some() { return }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// The pinned fingerprint, if any.
pub fn pinned() -> Option<String> { PINNED.lock().ok().and_then(|held| held.as_ref().map(|(f, _)| f.clone())) }

pub type Stream = tokio_rustls::client::TlsStream<tokio::io::Join<quinn::RecvStream, quinn::SendStream>>;

/// A relayed stream, ready to carry a connection.
pub enum Wrapped {
    /// TLS to Diamond's pinned identity.
    Encrypted(Stream),
    /// No identity pinned yet: plain, as before.
    Plain(quinn::SendStream, quinn::RecvStream),
    /// The handshake failed (not Diamond, or the relay dropped it): not to be used.
    Failed,
}

pub async fn wrap(send: quinn::SendStream, recv: quinn::RecvStream) -> Wrapped {
    let config = PINNED.lock().ok().and_then(|held| held.as_ref().map(|(_, config)| config.clone()));
    let Some(config) = config else { return Wrapped::Plain(send, recv) };
    let connector = tokio_rustls::TlsConnector::from(config);
    let name = rustls::pki_types::ServerName::try_from("torrbaby-diamond").expect("a valid name");
    match tokio::time::timeout(Duration::from_secs(15), connector.connect(name, tokio::io::join(recv, send))).await {
        Ok(Ok(stream)) => Wrapped::Encrypted(stream),
        _ => Wrapped::Failed,
    }
}
