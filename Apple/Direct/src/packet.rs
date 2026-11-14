//! The packet tunnel's half: the iPhone/iPad Network Extension hands every IP packet sent to
//! Diamond's tunnel address here. A userspace TCP stack (ipstack) turns them back into TCP
//! connections, and each connection becomes a QUIC stream on the direct path, exactly as the
//! app's loopback proxy does. Diamond pipes each stream to its own HTTP port, so it needs no
//! change for the tunnel.
//!
//! The point is the direct path: the extension owns the punched socket and its QUIC
//! connection, and iOS keeps the extension running while Torrbaby is suspended, so the path
//! to Diamond stays open (and is punched again on a network change) with the app in the
//! background. Background downloads, which iOS fetches itself, go straight to Diamond through
//! it. The relay through bronze carries a stream only while there is no direct path.
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;

/// A packet "device" for ipstack: each read is one packet from the extension, each write
/// one packet back to it.
pub struct Packets {
    incoming: mpsc::Receiver<Vec<u8>>,
    outgoing: mpsc::UnboundedSender<Vec<u8>>,
}

impl AsyncRead for Packets {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        match self.incoming.poll_recv(cx) {
            Poll::Ready(Some(packet)) => {
                let length = packet.len().min(buf.remaining());
                buf.put_slice(&packet[..length]);
                Poll::Ready(Ok(()))
            }
            // The extension stopped: an empty read ends the stack.
            Poll::Ready(None) => Poll::Ready(Ok(())),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for Packets {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        // ipstack writes each packet whole; taking it all at once keeps the boundary.
        match self.outgoing.send(buf.to_vec()) {
            Ok(()) => Poll::Ready(Ok(buf.len())),
            Err(_) => Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into())),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> { Poll::Ready(Ok(())) }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> { Poll::Ready(Ok(())) }
}

/// Where a TCP connection from the tunnel goes: a QUIC connection to open a stream on.
pub type Carrier = Box<dyn Fn() -> Option<quinn::Connection> + Send + Sync>;

/// The running stack: packets in through `input`, packets out through the receiver given to
/// `start`'s caller.
pub struct Stack {
    pub runtime: tokio::runtime::Runtime,
    input: mpsc::Sender<Vec<u8>>,
}

impl Stack {
    /// Start the stack. `carrier` is asked for a connection whenever a TCP connection arrives;
    /// while it has none (the path is being punched) the connection waits up to `wait`.
    pub fn start(mtu: u16, carrier: Carrier, wait: Duration) -> Option<(Stack, mpsc::UnboundedReceiver<Vec<u8>>)> {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().ok()?;
        let (input, incoming) = mpsc::channel(1024);
        let (outgoing, output) = mpsc::unbounded_channel();
        let carrier = std::sync::Arc::new(carrier);
        runtime.spawn(async move {
            let mut config = ipstack::IpStackConfig::default();
            config.mtu_unchecked(mtu);
            let mut stack = ipstack::IpStack::new(config, Packets { incoming, outgoing });
            while let Ok(stream) = stack.accept().await {
                let ipstack::IpStackStream::Tcp(tcp) = stream else { continue };
                let carrier = carrier.clone();
                tokio::spawn(async move {
                    let started = tokio::time::Instant::now();
                    let connection = loop {
                        if let Some(connection) = carrier() { break Some(connection) }
                        if started.elapsed() >= wait { break None }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    };
                    let Some(connection) = connection else { return };
                    let Ok((mut send, mut recv)) = connection.open_bi().await else { return };
                    let (mut from_ios, mut to_ios) = tokio::io::split(tcp);
                    let up = async { let _ = tokio::io::copy(&mut from_ios, &mut send).await; let _ = send.finish(); };
                    let down = async {
                        let _ = tokio::io::copy(&mut recv, &mut to_ios).await;
                        // ipstack sends the FIN only if nothing is in flight when asked and is
                        // not woken when that clears, so ask again until it goes (up to 30 s).
                        use tokio::io::AsyncWriteExt;
                        for _ in 0..600 {
                            if tokio::time::timeout(Duration::from_millis(50), to_ios.shutdown()).await.is_ok() { break }
                        }
                    };
                    tokio::join!(up, down);
                });
            }
        });
        Some((Stack { runtime, input }, output))
    }

    /// One packet from the extension. Dropped when the stack is this far behind (TCP resends).
    pub fn input(&self, packet: Vec<u8>) -> bool { self.input.try_send(packet).is_ok() }
}

/// The stack the extension runs, if started.
pub static STACK: Mutex<Option<Stack>> = Mutex::new(None);
