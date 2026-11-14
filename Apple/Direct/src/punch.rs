//! Punching a direct path to Diamond, shared by every client: the iPhone/iPad app, its packet
//! tunnel, and later the Mac, Android TVs and Miyu Code. The server half is server/direct.rs.
//!
//! 1. Open many UDP sockets on the real network (bound to its interface, so a VPN such as a
//!    Tailscale exit node does not carry them).
//! 2. Ask bronze's STUN server (3478) from a dozen of them for this network's public ports;
//!    the same sockets then ask bronze's second STUN port (3479). A different answer means the
//!    NAT gives each destination its own port (China Mobile), so Diamond sprays random ports.
//!    The same answer with ports handed out far apart (T-Mobile US) means Diamond aims at
//!    exactly the ports STUN saw; ports in sequence (China Telecom) get the predicted window.
//! 3. Ask Diamond to start (POST /v1/direct over the route the client already has: the relay's
//!    loopback port or Tailscale), with the public IP, the window and the known ports.
//! 4. Fire `TBDIRECT1 <id> <socket>` from every socket while Diamond sprays; Diamond's echo
//!    (`TBACK …`) on a socket proves both directions, and that socket carries QUIC.
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

/// What an attempt has found so far, reported at each step (and kept in the result).
#[derive(Default, Clone)]
pub struct Report {
    pub step: String,
    pub interface: Option<String>,
    pub public_address: Option<String>,
    pub asked: usize,
    pub answered: usize,
    pub diagnosis: Option<String>,
    pub spread: Option<String>,
    pub window: Option<String>,
    pub server: Option<String>,
    pub session: Option<String>,
    pub punch_ms: Option<u64>,
}

impl Report {
    pub fn json(&self) -> Value {
        json!({"step": self.step, "interface": self.interface, "public_address": self.public_address, "asked": self.asked,
            "answered": self.answered, "diagnosis": self.diagnosis, "spread": self.spread, "window": self.window,
            "server": self.server, "session": self.session, "punch_ms": self.punch_ms})
    }
}

/// How to punch.
pub struct Plan {
    /// bronze's STUN server, "ip:3478", and its second port (by default 3479 on the same host).
    pub stun: String,
    pub stun2: Option<String>,
    /// The route to ask Diamond over: "http://host:port" (the relay's loopback port, Tailscale).
    pub signal: String,
    /// This device's token for Diamond.
    pub token: String,
    /// The interface to bind the sockets to ("en0", "pdp_ip0"); none binds nothing.
    pub interface: Option<String>,
    /// Public IPs a predicted spray failed from before: random ports from there.
    pub random_from: Vec<String>,
    pub sockets: usize,
}

/// A path that got through: the socket (ready for QUIC) and how it was made.
pub struct Punched {
    pub socket: UdpSocket,
    pub server: SocketAddr,
    pub fingerprint: String,
    pub public_address: String,
    pub summary: String,
}

/// Why an attempt failed, with what Diamond needs to explain it.
pub struct Failure {
    pub message: String,
    /// Diamond's session, when it was asked to start (its GET /v1/direct/<id> view).
    pub session: Option<String>,
    /// This network's public IP, when a predicted spray failed from it.
    pub window_failed_at: Option<String>,
}

fn fail(message: impl Into<String>) -> Failure { Failure { message: message.into(), session: None, window_failed_at: None } }

pub fn punch(plan: &Plan, mut report: impl FnMut(&Report)) -> Result<Punched, Failure> {
    let mut r = Report::default();
    let mut step = |r: &mut Report, text: String| { r.step = text; report(r) };
    step(&mut r, "opening sockets".into());
    let sockets = open_sockets(plan.sockets, plan.interface.as_deref());
    r.interface = plan.interface.clone();
    if sockets.is_empty() { return Err(fail("No Wi-Fi or cellular network to punch from")) }
    let stun: SocketAddr = plan.stun.parse().map_err(|_| fail(format!("bronze's STUN address is not valid: {}", plan.stun)))?;
    r.asked = 12.min(sockets.len());
    step(&mut r, format!("asking bronze's STUN server ({}) for this network's public ports", plan.stun));
    let mappings = discover(&sockets[..r.asked], stun);
    let Some(ip) = mappings.values().next().map(|m| m.ip().to_string()) else {
        return Err(fail(format!("bronze's STUN server did not answer: UDP to {} may be blocked on this network", plan.stun)))
    };
    let mut ports: Vec<u16> = mappings.values().map(|m| m.port()).collect();
    ports.sort_unstable();
    let (first, last) = (ports[0], ports[ports.len() - 1]);
    let span = last - first;
    let low = first.max(1);
    let high = (last as u32 + span as u32 + 1024).min(65535) as u16;
    r.public_address = Some(ip.clone());
    r.answered = mappings.len();
    step(&mut r, "comparing ports toward bronze's two STUN ports".into());
    let second_stun = plan.stun2.as_deref().and_then(|s| s.parse().ok()).unwrap_or(SocketAddr::new(stun.ip(), 3479));
    let six: Vec<usize> = mappings.keys().copied().take(6).collect();
    let second = discover_some(&sockets, &six, second_stun);
    let pairs: Vec<(SocketAddrV4, SocketAddrV4)> = six.iter().filter_map(|i| Some((mappings[i], *second.get(i)?))).collect();
    let per_destination = pairs.iter().any(|(a, b)| a != b);
    let mut steps: Vec<u16> = ports.windows(2).map(|w| w[1] - w[0]).collect();
    steps.sort_unstable();
    let median_step = steps.get(steps.len() / 2).copied().unwrap_or(0);
    let random_spray = plan.random_from.contains(&ip);
    // One port per socket, handed out far apart: no window predicts them, and none is needed.
    let exact = !per_destination && !pairs.is_empty() && median_step > 512 && !random_spray;
    let random = random_spray || per_destination;
    let mut diagnosis = if pairs.is_empty() { "second STUN port did not answer".to_owned() } else {
        format!("{} ({})", if per_destination { "NAT gives each destination its own port" } else { "same port for every destination" },
            pairs.iter().take(3).map(|(a, b)| if a.ip() != b.ip() { format!("{}/{} {}", a.port(), b.port(), b.ip()) } else { format!("{}/{}", a.port(), b.port()) }).collect::<Vec<_>>().join(", "))
    };
    diagnosis.push_str(&format!(", median step {median_step}"));
    if random_spray && !per_destination { diagnosis.push_str(", window failed here before") }
    let spread = if exact { "exact" } else if random { "random" } else { "window" };
    r.diagnosis = Some(diagnosis.clone());
    r.spread = Some(spread.into());
    r.window = Some(match spread {
        "exact" => format!("the {} ports STUN saw ({first}–{last})", mappings.len()),
        "random" => "2048 random ports across the whole range".into(),
        _ => format!("{low}–{high} (STUN saw {first}–{last})"),
    });
    step(&mut r, "asking Diamond to start".into());
    let mut body = json!({"ip": ip, "low": low, "high": high, "diagnosis": diagnosis, "ports": ports});
    if spread != "window" { body["spread"] = json!(spread) }
    let started = post(&plan.signal, &plan.token, "/v1/direct", &body).map_err(|e| fail(format!("Could not ask Diamond to start: {e}")))?;
    let (Some(id), Some(server), Some(fingerprint)) = (started["id"].as_str(), started["server"].as_str(), started["fingerprint"].as_str()) else {
        return Err(fail("Diamond's answer to start was incomplete"))
    };
    let server_address: SocketAddr = server.parse().map_err(|_| fail(format!("Diamond's address is not valid: {server}")))?;
    let started_spread = started["spread"].as_str().unwrap_or("window").to_owned();
    let sprays = match started_spread.as_str() {
        "random" => "2048 random ports".to_owned(),
        "exact" => format!("the {} ports STUN saw", mappings.len()),
        _ => format!("{}–{}", started["low"], started["high"]),
    };
    r.server = Some(server.into());
    r.session = Some(id.into());
    step(&mut r, format!("punching: firing from {} sockets at {server} while Diamond sprays {sprays}", sockets.len()));
    let began = Instant::now();
    let Some(winner) = fire(&sockets, server_address, id, Duration::from_secs(20)) else {
        let message = if started_spread == "random" {
            "No packet got through both NATs in 20 s, even spraying random ports. This network may use a different public address toward Diamond, or block incoming UDP".to_owned()
        } else {
            format!("No packet got through both NATs in 20 s spraying {sprays}; the next attempt from {ip} sprays random ports")
        };
        return Err(Failure { message, session: Some(id.into()), window_failed_at: (started_spread != "random").then(|| ip.clone()) })
    };
    let ms = began.elapsed().as_millis() as u64;
    r.punch_ms = Some(ms);
    step(&mut r, "punched; opening QUIC on the winning socket".into());
    let socket = sockets.into_iter().nth(winner).expect("winner is one of the sockets");
    Ok(Punched { socket, server: server_address, fingerprint: fingerprint.into(), public_address: ip.clone(),
        summary: format!("punched from {ip} in {ms} ms ({started_spread})") })
}

/// UDP sockets on the wildcard address, non-blocking, bound to `interface` when given.
fn open_sockets(count: usize, interface: Option<&str>) -> Vec<UdpSocket> {
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    let index = interface.and_then(|name| std::ffi::CString::new(name).ok()).map(|name| unsafe { libc::if_nametoindex(name.as_ptr()) }).unwrap_or(0);
    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    let _ = interface;
    (0..count).filter_map(|_| {
        let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        if index != 0 {
            use std::os::fd::AsRawFd;
            let value = index as libc::c_uint;
            // On the real network, not the VPN: an exit node would carry it abroad.
            unsafe { libc::setsockopt(socket.as_raw_fd(), libc::IPPROTO_IP, libc::IP_BOUND_IF, &value as *const _ as *const libc::c_void, std::mem::size_of::<libc::c_uint>() as libc::socklen_t) };
        }
        socket.set_nonblocking(true).ok()?;
        Some(socket)
    }).collect()
}

/// Each socket's public address, by index, from `stun` (asked from every socket given).
fn discover(sockets: &[UdpSocket], stun: SocketAddr) -> HashMap<usize, SocketAddrV4> {
    discover_some(sockets, &(0..sockets.len()).collect::<Vec<_>>(), stun)
}

fn discover_some(sockets: &[UdpSocket], which: &[usize], stun: SocketAddr) -> HashMap<usize, SocketAddrV4> {
    let mut transactions: HashMap<[u8; 12], usize> = HashMap::new();
    for &index in which {
        let id: [u8; 12] = std::array::from_fn(|i| (random_u64() >> (i % 8 * 8)) as u8 ^ (index as u8).wrapping_mul(31).wrapping_add(i as u8));
        let mut request = vec![0u8, 1, 0, 0, 0x21, 0x12, 0xA4, 0x42];
        request.extend_from_slice(&id);
        transactions.insert(id, index);
        let _ = sockets[index].send_to(&request, stun);
    }
    let mut found = HashMap::new();
    let deadline = Instant::now() + Duration::from_millis(2500);
    let mut buffer = [0u8; 1500];
    while Instant::now() < deadline && found.len() < which.len() {
        for &index in which {
            while let Ok((n, _)) = sockets[index].recv_from(&mut buffer) {
                if n < 20 { continue }
                let Ok(id): Result<[u8; 12], _> = buffer[8..20].try_into() else { continue };
                if let (Some(&owner), Some(mapped)) = (transactions.get(&id), mapped(&buffer[..n])) { found.insert(owner, mapped); }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    found
}

/// The (XOR-)MAPPED-ADDRESS of a STUN binding response.
fn mapped(data: &[u8]) -> Option<SocketAddrV4> {
    let mut i = 20;
    while i + 4 <= data.len() {
        let kind = u16::from_be_bytes([data[i], data[i + 1]]);
        let length = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        let value = data.get(i + 4..i + 4 + length)?;
        if (kind == 0x0020 || kind == 0x8020) && value.len() >= 8 && value[1] == 1 {
            let port = u16::from_be_bytes([value[2], value[3]]) ^ 0x2112;
            let ip = [value[4] ^ 0x21, value[5] ^ 0x12, value[6] ^ 0xA4, value[7] ^ 0x42];
            return Some(SocketAddrV4::new(ip.into(), port));
        }
        if kind == 0x0001 && value.len() >= 8 && value[1] == 1 {
            return Some(SocketAddrV4::new([value[4], value[5], value[6], value[7]].into(), u16::from_be_bytes([value[2], value[3]])));
        }
        i += 4 + length + (4 - length % 4) % 4;
    }
    None
}

/// Fire from every socket until Diamond echoes one (both directions proven); its index.
fn fire(sockets: &[UdpSocket], server: SocketAddr, id: &str, limit: Duration) -> Option<usize> {
    let deadline = Instant::now() + limit;
    let mut next_send = Instant::now();
    let mut buffer = [0u8; 1500];
    let wanted = format!("TBDIRECT1 {id} ");
    while Instant::now() < deadline {
        if Instant::now() >= next_send {
            for (index, socket) in sockets.iter().enumerate() { let _ = socket.send_to(format!("TBDIRECT1 {id} {index}").as_bytes(), server); }
            next_send = Instant::now() + Duration::from_millis(200);
        }
        for (index, socket) in sockets.iter().enumerate() {
            while let Ok((n, from)) = socket.recv_from(&mut buffer) {
                if from.ip() != server.ip() { continue }
                let Ok(text) = std::str::from_utf8(&buffer[..n]) else { continue };
                if text.starts_with("TBACK") && text.contains(&wanted) { return Some(index) }
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

/// A JSON POST to Diamond over `base` ("http://host:port"), with the device's token. Plain
/// HTTP/1.1 over TCP: the relay's loopback port, or Tailscale.
pub fn post(base: &str, token: &str, path: &str, body: &Value) -> Result<Value, String> {
    let authority = base.strip_prefix("http://").ok_or("only http:// routes")?.trim_end_matches('/');
    let address = std::net::ToSocketAddrs::to_socket_addrs(authority).map_err(|e| format!("{authority}: {e}"))?
        .next().ok_or(format!("{authority} does not resolve"))?;
    let mut tcp = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(10)).map_err(|e| e.to_string())?;
    tcp.set_read_timeout(Some(Duration::from_secs(30))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(10))).ok();
    let payload = body.to_string();
    let request = format!("POST {path} HTTP/1.1\r\nHost: {authority}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}", payload.len());
    tcp.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
    let mut response = Vec::new();
    tcp.read_to_end(&mut response).map_err(|e| e.to_string())?;
    let split = response.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no HTTP answer")?;
    let head = String::from_utf8_lossy(&response[..split]).to_string();
    let mut content = response[split + 4..].to_vec();
    let status: u16 = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or("no HTTP status")?;
    if head.to_ascii_lowercase().contains("transfer-encoding: chunked") { content = dechunk(&content) }
    let value: Value = serde_json::from_slice(&content).map_err(|_| format!("HTTP {status}"))?;
    if !(200..300).contains(&status) { return Err(value["error"].as_str().map(str::to_owned).unwrap_or(format!("HTTP {status}"))) }
    Ok(value)
}

fn dechunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(end) = data.windows(2).position(|w| w == b"\r\n") {
        let size = usize::from_str_radix(std::str::from_utf8(&data[..end]).unwrap_or("0").split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
        if size == 0 { break }
        let start = end + 2;
        let Some(chunk) = data.get(start..start + size) else { break };
        out.extend_from_slice(chunk);
        data = data.get(start + size + 2..).unwrap_or(&[]);
    }
    out
}

fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(Instant::now().elapsed().as_nanos() ^ std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    hasher.finish()
}
