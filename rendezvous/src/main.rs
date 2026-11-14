//! tbrendezvous: Torrbaby's rendezvous on bronze. See lib.rs.
//!
//!   tbrendezvous [--state DIR] [--stun PORT] [--stun2 PORT] [--quic PORT]   run the service
//!   tbrendezvous [--state DIR] info                           print the certificate
//!       fingerprint and the path of Diamond's secret, making both on first use
use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use tbrendezvous::{Hub, Identity, endpoint, load_or_create_secret, serve, stun};

fn main() -> Result<()> {
    let mut state = PathBuf::from("/var/lib/tbrendezvous");
    let (mut stun_port, mut quic_port) = (3478u16, 7443u16);
    // A second STUN port: a device asking both from one socket learns whether its NAT gives
    // each destination its own port (China Mobile does, which defeats predicting the port).
    let mut stun_port_2 = 3479u16;
    let mut info = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--state" => state = args.next().context("--state needs a directory")?.into(),
            "--stun" => stun_port = args.next().context("--stun needs a port")?.parse()?,
            "--stun2" => stun_port_2 = args.next().context("--stun2 needs a port")?.parse()?,
            "--quic" => quic_port = args.next().context("--quic needs a port")?.parse()?,
            "info" => info = true,
            other => bail!("Unknown argument {other}"),
        }
    }
    std::fs::create_dir_all(&state).with_context(|| format!("Creating {}", state.display()))?;
    let identity = Identity::load_or_create(&state)?;
    let secret = load_or_create_secret(&state)?;
    if info {
        println!("fingerprint {}", identity.fingerprint);
        println!("secret {}", state.join("home.secret").display());
        return Ok(());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async move {
        let stun_socket = tokio::net::UdpSocket::bind(("0.0.0.0", stun_port)).await.with_context(|| format!("STUN port {stun_port}"))?;
        let stun_socket_2 = tokio::net::UdpSocket::bind(("0.0.0.0", stun_port_2)).await.with_context(|| format!("STUN port {stun_port_2}"))?;
        let quic_socket = std::net::UdpSocket::bind(("0.0.0.0", quic_port)).with_context(|| format!("QUIC port {quic_port}"))?;
        let endpoint = endpoint(quic_socket, &identity)?;
        tokio::spawn(stun::serve(stun_socket_2));
        eprintln!("tbrendezvous: STUN on UDP {stun_port} and {stun_port_2}, QUIC on UDP {quic_port}, fingerprint {}", identity.fingerprint);
        tokio::spawn(stun::serve(stun_socket));
        let hub = Hub::new(&secret);
        hub.keep_usage(&state);
        let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            _ = serve(endpoint.clone(), hub.clone()) => {}
            _ = terminate.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
        hub.save_usage();
        endpoint.close(0u32.into(), b"shutting down");
        endpoint.wait_idle().await;
        anyhow::Ok(())
    })
}
