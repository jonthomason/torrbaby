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

/// Where a TCP connection from the tunnel goes: a stream on a QUIC connection (the direct
/// path or the relay), or, at home, a plain TCP connection to Diamond on this network.
#[derive(Clone)]
pub enum Route {
    Quic(quinn::Connection),
    Lan(std::net::SocketAddr),
}
pub type Carrier = Box<dyn Fn() -> Option<Route> + Send + Sync>;
/// Whether a connection on `route` should make way: it is on the relay, and the direct path
/// or Diamond on this network is now open.
pub type Superseded = Box<dyn Fn(&Route) -> bool + Send + Sync>;

/// The running stack: packets in through `input`, packets out through the receiver given to
/// `start`'s caller.
pub struct Stack {
    pub runtime: tokio::runtime::Runtime,
    input: mpsc::Sender<Vec<u8>>,
}

impl Stack {
    /// Start the stack. `carrier` is asked for a route whenever a TCP connection arrives;
    /// while it has none (the path is being punched) the connection waits up to `wait`.
    ///
    /// A connection carried by the relay is closed once a better route has been open for 2 s
    /// (the direct path, or Diamond on this network), so the client's next request takes it.
    /// Players hold one connection for as long as they can: video started while the path was
    /// being punched (just back from the background, a new network) otherwise stayed on the
    /// relay to the end, at the relay's speed, with the direct path open and unused.
    pub fn start(mtu: u16, carrier: Carrier, superseded: Superseded, wait: Duration) -> Option<(Stack, mpsc::UnboundedReceiver<Vec<u8>>)> {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().ok()?;
        let (input, incoming) = mpsc::channel(1024);
        let (outgoing, output) = mpsc::unbounded_channel();
        let carrier = std::sync::Arc::new(carrier);
        let superseded = std::sync::Arc::new(superseded);
        runtime.spawn(async move {
            let mut config = ipstack::IpStackConfig::default();
            config.mtu_unchecked(mtu);
            // 64 KB in flight per connection toward iOS (ipstack's default 16 KB made video too
            // slow to play; without window scaling 64 KB is TCP's ceiling), and segments sized
            // to the tunnel's MTU.
            let mut tcp = ipstack::TcpConfig::default();
            tcp.max_unacked_bytes = 65535;
            tcp.options = Some(vec![ipstack::TcpOptions::MaximumSegmentSize(mtu.saturating_sub(40))]);
            config.with_tcp_config(tcp);
            let mut stack = ipstack::IpStack::new(config, Packets { incoming, outgoing });
            RUNNING.store(true, std::sync::atomic::Ordering::SeqCst);
            while let Ok(stream) = stack.accept().await {
                let ipstack::IpStackStream::Tcp(tcp) = stream else { continue };
                let carrier = carrier.clone();
                let superseded = superseded.clone();
                tokio::spawn(async move {
                    let started = tokio::time::Instant::now();
                    let route = loop {
                        if let Some(route) = carrier() { break Some(route) }
                        if started.elapsed() >= wait { break None }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    };
                    let Some(route) = route else { stats::add(&stats::FLOWS_UNCARRIED, 1); return };
                    // A better route open at two checks a second apart: this one makes way.
                    let moved = {
                        let route = route.clone();
                        async move {
                            let mut seen = 0;
                            loop {
                                tokio::time::sleep(Duration::from_secs(1)).await;
                                seen = if superseded(&route) { seen + 1 } else { 0 };
                                if seen >= 2 { return }
                            }
                        }
                    };
                    match route {
                        Route::Quic(connection) => {
                            let Ok((mut send, mut recv)) = connection.open_bi().await else { stats::add(&stats::FLOWS_UNCARRIED, 1); return };
                            let complete = carry(tcp, &mut send, &mut recv, moved).await;
                            // A connection abandoned midway: tell QUIC to discard what it holds
                            // for this stream, or that data keeps the connection's receive
                            // allowance and, added up over many abandoned requests (video
                            // seeks), stalls every stream.
                            if !complete { let _ = recv.stop(0u32.into()); }
                        }
                        Route::Lan(address) => {
                            let connected = tokio::time::timeout(Duration::from_secs(3), tokio::net::TcpStream::connect(address)).await;
                            let Ok(Ok(diamond)) = connected else { stats::add(&stats::FLOWS_UNCARRIED, 1); return };
                            let _ = diamond.set_nodelay(true);
                            stats::add(&stats::FLOWS_LAN, 1);
                            let (mut from_diamond, mut to_diamond) = diamond.into_split();
                            carry(tcp, &mut to_diamond, &mut from_diamond, moved).await;
                        }
                    }
                });
            }
            // The stack's own task ended (a panic inside it): the extension restarts it.
            RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
        });
        Some((Stack { runtime, input }, output))
    }

    /// One packet from the extension. Dropped when the stack is this far behind (TCP resends).
    pub fn input(&self, packet: Vec<u8>) -> bool {
        let length = packet.len() as u64;
        let accepted = self.input.try_send(packet).is_ok();
        if accepted { stats::add(&stats::PACKETS_IN, 1); stats::add(&stats::BYTES_IN, length) } else { stats::add(&stats::DROPPED_IN, 1) }
        accepted
    }
}

/// Carry one TCP connection from iOS to Diamond (`up`) and back (`down`) until either side
/// ends it or `moved` fires. True when Diamond's reply was delivered whole.
async fn carry<W, R>(tcp: ipstack::IpStackTcpStream, up_to: &mut W, down_from: &mut R, moved: impl std::future::Future<Output = ()>) -> bool
where W: tokio::io::AsyncWrite + Unpin, R: tokio::io::AsyncRead + Unpin {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stats::add(&stats::FLOWS_OPENED, 1);
    stats::add(&stats::FLOWS_OPEN, 1);
    struct Open;
    impl Drop for Open { fn drop(&mut self) { stats::FLOWS_OPEN.fetch_sub(1, std::sync::atomic::Ordering::Relaxed); } }
    let _open = Open;
    let (mut from_ios, mut to_ios) = tokio::io::split(tcp);
    // ipstack marks a connection closed when iOS resets it (a player cutting a request short)
    // but wakes nothing waiting on it, so reads and writes toward iOS are attempted in 1 s
    // slices: each new attempt sees the close at once. `up` is true when it failed.
    let up = async {
        let mut buffer = vec![0u8; 16 * 1024];
        loop {
            match tokio::time::timeout(Duration::from_secs(1), from_ios.read(&mut buffer)).await {
                Err(_) => continue,
                Ok(Ok(0)) => { let _ = up_to.shutdown().await; return false }
                Ok(Ok(n)) => { if up_to.write_all(&buffer[..n]).await.is_err() { return true }; stats::add(&stats::STREAM_UP, n as u64) }
                Ok(Err(_)) => { let _ = up_to.shutdown().await; return true }
            }
        }
    };
    let down = async {
        let mut buffer = vec![0u8; 16 * 1024];
        loop {
            // Diamond quiet for 2 minutes: the stream is given up.
            let n = match tokio::time::timeout(Duration::from_secs(120), down_from.read(&mut buffer)).await {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => n,
                _ => return false,
            };
            let (mut written, mut idle) = (0, 0);
            while written < n {
                match tokio::time::timeout(Duration::from_secs(1), to_ios.write(&buffer[written..n])).await {
                    Ok(Ok(0)) | Ok(Err(_)) => return false,
                    Ok(Ok(m)) => { written += m; idle = 0; stats::add(&stats::STREAM_DOWN, m as u64) }
                    // iOS taking nothing for a minute: the connection is dead.
                    Err(_) => { idle += 1; if idle >= 60 { return false } }
                }
            }
        }
        // ipstack sends the FIN only if nothing is in flight when asked and is not woken when
        // that clears, so ask again until it goes (up to 30 s).
        for _ in 0..600 {
            if tokio::time::timeout(Duration::from_millis(50), to_ios.shutdown()).await.is_ok() { break }
        }
        true
    };
    let (complete, was_moved) = {
        tokio::pin!(down);
        tokio::pin!(up);
        tokio::pin!(moved);
        tokio::select! {
            delivered = &mut down => { if delivered { let _ = up.await; } (delivered, false) }
            failed = &mut up => (if failed { false } else { down.await }, false),
            _ = &mut moved => (false, true),
        }
    };
    if was_moved { stats::add(&stats::FLOWS_MOVED, 1) } else if !complete { stats::add(&stats::FLOWS_ABANDONED, 1) }
    if !complete {
        // Dropping the stream tells iOS nothing: the connection is closed toward it (FIN), so
        // the client sees the reply cut short and asks again (by the better route, if moved)
        // instead of waiting on a connection nothing will ever answer.
        for _ in 0..100 {
            if tokio::time::timeout(Duration::from_millis(50), to_ios.shutdown()).await.is_ok() { break }
        }
    }
    complete
}

/// Counters for the extension's periodic log line (what the tunnel carried, and where it lost).
pub mod stats {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    pub static PACKETS_IN: AtomicU64 = AtomicU64::new(0);
    pub static BYTES_IN: AtomicU64 = AtomicU64::new(0);
    pub static PACKETS_OUT: AtomicU64 = AtomicU64::new(0);
    pub static BYTES_OUT: AtomicU64 = AtomicU64::new(0);
    /// Packets from iOS dropped because the stack was behind (TCP resends them).
    pub static DROPPED_IN: AtomicU64 = AtomicU64::new(0);
    /// TCP connections from iOS: opened, open now, given up (abandoned or stalled).
    pub static FLOWS_OPENED: AtomicU64 = AtomicU64::new(0);
    pub static FLOWS_OPEN: AtomicU64 = AtomicU64::new(0);
    pub static FLOWS_ABANDONED: AtomicU64 = AtomicU64::new(0);
    /// Connections carried to Diamond on this network (at home).
    pub static FLOWS_LAN: AtomicU64 = AtomicU64::new(0);
    /// Connections closed on the relay so their next request takes a better route.
    pub static FLOWS_MOVED: AtomicU64 = AtomicU64::new(0);
    /// Connections that found no direct path or relay to carry them.
    pub static FLOWS_UNCARRIED: AtomicU64 = AtomicU64::new(0);
    /// Bytes delivered from Diamond to iOS, and from iOS to Diamond, through streams.
    pub static STREAM_DOWN: AtomicU64 = AtomicU64::new(0);
    pub static STREAM_UP: AtomicU64 = AtomicU64::new(0);
    pub fn add(counter: &AtomicU64, n: u64) { counter.fetch_add(n, Relaxed); }
    pub fn get(counter: &AtomicU64) -> u64 { counter.load(Relaxed) }
}

/// Whether the stack is accepting connections.
pub static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The stack the extension runs, if started.
pub static STACK: Mutex<Option<Stack>> = Mutex::new(None);
