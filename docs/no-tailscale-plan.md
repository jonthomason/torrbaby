# Running without Tailscale: bronze rendezvous, direct tunnel, relay

Status: plan, written 2026-11-13 at the end of the session that built the direct tunnel.
Owner: the next session. The user wants this ("running without Tailscale is a very desirable
thing"), in Rust, on bronze.

## Where things stand

- **Diamond** (home server, Shenzhen) sits behind **China Telecom carrier NAT**: no public
  IPv4 and no IPv6, and China Telecom refuses both at any price. Its mapping is stable
  (endpoint-independent) and its filtering strict. The UniFi router (UCG Fiber, WAN
  100.75.107.46) cannot forward anything usefully. Diamond's fixed DHCP lease 192.168.1.44
  was corrected in UniFi (the old one was 192.168.50.10 from the previous house).
- **Torrbaby's own direct tunnel works** (iOS Apple 247, server ≥ 2306952), automatically
  away from home:
  - The phone opens 192 UDP sockets bound to its physical interface, so a Tailscale exit node
    does not capture them. It learns their public ports from STUN (12 sockets ask) and asks
    Diamond to start (`POST /v1/direct`, `server/direct.rs`).
  - Diamond sprays the predicted port window from its own socket and echoes punch packets
    until the phone's QUIC arrives, then runs QUIC (quinn) on that same socket and pipes each
    stream to its HTTP port.
  - The phone runs QUIC on the punched socket via the Rust static library `Apple/Direct`
    (`tbdirect_start/alive/stop`; built by `scripts/build_direct.sh`, linked by
    `build_apple.sh ios`) and a loopback proxy. App requests and the player use
    `http://127.0.0.1:<port>`.
  - Measured on the user's Xiaomi hotspot over China Telecom: punch in about 0.3 s, round
    trip 22–48 ms, Tigers 4K at 10+ Mbit/s.
  - Diagnostics: Settings → About → Direct connection test. Server log lines start
    `Direct <id>:`, one per tunnel stream.
- **What still needs Tailscale:** the signalling. `POST /v1/direct` and the once-a-minute
  `/v1/ping` go over Tailscale (`diamond-ts.bengal-locrian.ts.net`) when away. Without
  Tailscale the phone cannot reach Diamond at all, because nothing at home accepts inbound.
- **STUN today:** `stun.chat.bilibili.com:3478` (Google as fallback), chosen ad hoc. It
  throttles bursts. This gets replaced by bronze.

## bronze

- Alibaba Cloud ECS in Shenzhen. Fixed public IP **47.112.18.69**, no DNS name. Using the
  IP is correct here: it is bronze's own fixed address, not Diamond's.
- `ssh bronze` (user ecs-user, key `~/.ssh/cn_miyu_ssh.pem`, sudo works).
- Ubuntu 26.04, x86_64, 2 cores, 3.5 GB RAM.
- About 8 ms from Diamond, with a direct Tailscale path (47.112.18.69:41641).
- Already runs Caddy (TCP 80/443 and UDP 443), Tailscale, and node services on loopback
  (43118, 43127).
- **Firewall:** ufw allows only 22/tcp, 80/tcp, 443/tcp and 41641/udp. Test UDP ports 3478,
  7443 and 40000 got no reply from Diamond or the Mac on 2026-11-13. Open the new ports in
  ufw **and** check the Alibaba security group, which is separate and may need the console;
  ask the user if it does.

## Ports to open on bronze

The user will open whatever is needed. Open these **inbound from anywhere**, in **both** the
Alibaba Cloud security group (the user does this in the console) **and** ufw on bronze
(`sudo ufw allow 3478/udp comment 'Torrbaby STUN'`, `sudo ufw allow 7443/udp comment
'Torrbaby rendezvous/relay'`):

| Port | Protocol | Purpose |
|---|---|---|
| 3478 | UDP | STUN: phones and Diamond learn their public address and port |
| 7443 | UDP | QUIC: Diamond's standing connection, phones' rendezvous and relay |

Nothing else is needed: everything after setup rides 7443, and the direct tunnel goes phone
to Diamond without bronze. Tailscale's 41641/udp stays as it is.

## Design

One Rust service on bronze (`rendezvous/` in this repo, binary `tbrendezvous`, systemd unit),
three jobs:

1. **STUN** (UDP 3478): RFC 5389 Binding responses with XOR-MAPPED-ADDRESS. Phone and
   Diamond use it instead of Bilibili; remove Bilibili and Google from `server/punch.rs`,
   `server/direct.rs` and the app's `PunchProbe.discover`.
2. **Rendezvous and relay over QUIC** (one UDP port, for example 7443; not 443, which Caddy
   uses):
   - **Diamond side:** the Torrbaby server keeps one outbound QUIC connection to bronze
     (ALPN `tbhome`), authenticated with a shared secret made at setup (kept in Diamond's
     config and bronze's config, never in clients). It reconnects with backoff; keepalives
     every ~10 s hold China Telecom's mapping. For each stream bronze opens on that
     connection, Diamond pipes it to its own HTTP port, reusing `direct.rs`'s `pipe`.
   - **Phone side:** the app connects to bronze (ALPN `tbrelay`) with `Apple/Direct`, adding
     a `tbdirect_relay(server, fingerprint)` or generalising `tbdirect_start` to open its own
     socket. It pins bronze's certificate fingerprint (in the app's configuration or the
     pairing code). Each loopback connection becomes a stream; bronze opens a matching stream
     to Diamond and splices bytes.
   - Requests stay ordinary Torrbaby requests carrying the device's own credentials; Diamond
     checks them as today. Bronze relays only for Diamond (single library), keyed by
     Diamond's server id.
3. **Signalling** rides the relay: the phone's `/v1/ping` and `POST /v1/direct` go through
   bronze when it has no LAN or Tailscale route. The direct tunnel is then set up exactly as
   now, and traffic moves off the relay onto the punched path.

**Encryption decision:** start simple. Phone↔bronze and bronze↔Diamond are each TLS/QUIC,
so bronze sees relayed requests, credentials included. It is the user's own server; this was
proposed and not objected to. End-to-end encryption through bronze, with bronze seeing only
ciphertext, is the follow-up, for example phone↔Diamond QUIC carried inside the relay
streams.

## Route preference (app)

1. Home network: `diamond.local` (Bonjour).
2. Direct tunnel (punched QUIC).
3. bronze relay.
4. Tailscale, only on devices that have it, kept as a fallback.

`Store.endpoint()` already prefers `directRoute`; add `relayRoute` (the loopback proxy into
the bronze connection) between direct and Tailscale. The probe race should include the relay
when neither home nor Tailscale answers. Fallback stays automatic. No user-facing switches.

## Pairing without Tailscale

The pairing QR code adds bronze's address (47.112.18.69:7443), bronze's certificate
fingerprint, and Diamond's server id. A new device then needs no Tailscale app. At home,
Bonjour finds Diamond as the TVs already do.

## Connection status page

Settings → Connection status, out of the way: the route in use (home, direct, relay or
Tailscale), round-trip time, the bytes or streams on the current path, and recent events
(opened, dropped, why). It replaces the Direct connection test screen. Keep the
"Try direct now" control there for diagnosis only. No new strings are needed (the user said
translations are unnecessary for this page).

## Steps

1. **bronze service.** `rendezvous/` crate: STUN, plus a QUIC endpoint with the `tbhome` and
   `tbrelay` roles; a self-signed certificate generated once and stored, its fingerprint
   printed; a config file with Diamond's shared secret. A systemd unit; open ufw for UDP
   3478 and 7443; check the Alibaba security group. Test STUN and QUIC reachability from
   Diamond, the Mac and a phone.
2. **Diamond.** An outbound connection to bronze (config: bronze address, fingerprint,
   secret), serving bronze-opened streams through `pipe`; status in `/v1/status`. Switch
   STUN to bronze in `punch.rs` and `direct.rs`.
3. **App.** `Apple/Direct` gains the relay connection; `Store` gains `relayRoute`, signalling
   over it, and STUN on bronze; route preference as above; the connection status page.
4. **Pairing** carries bronze's address and fingerprint.
5. **Test without Tailscale.** Turn Tailscale off on the phone, on the China Telecom
   hotspot: expect relay first, then direct within seconds. Then US roaming (random ports;
   direct should usually punch, otherwise relay) and China Mobile (looked like a different
   public IP per destination; relay at least).
6. **Later:** end-to-end encryption through the relay; Android TVs' Bonjour route is
   unaffected.

## Open items carried over

- The app's check-in: rates and metadata (/v1/metadata is 1.1 MB compressed) now load in
  the background. Confirm the status line shows a real "last check-in" in Apple 247.
- iOS suspends the app in the background, so tunnels drop; the app retries on returning to
  the foreground and right after a drop.
- Xiaomi dual-SIM "acceleration" mixed China Mobile into China Telecom tests; the user turned
  it off.
- Build numbers: next Apple > 247, Android > 101 (`local-state/credits-release-handoff.txt`).
  I build and publish myself now; see memory `torrbaby-release-procedure`.

## Next: Torrbaby's own packet tunnel (decided 2026-11-14)

**Why.** Offline downloads use iOS background transfers, which iOS runs while the app is
suspended. They need an address iOS can reach without the app. Today only Tailscale gives
one: the direct path and the relay live inside the app (loopback proxy) and stop when it is
suspended. Routing downloads through bronze is ruled out: bronze is capped at 50 Mbit/s and
billed for traffic past an allowance. Transfers must go direct, in the background too.

**Decisions (the user's).**
- Both Tailscale and Torrbaby's own tunnel stay supported. A device on Tailscale (the user's,
  often with an exit node) keeps using Tailscale, including for background downloads. iOS
  allows one VPN connection at a time and starting another disconnects Tailscale, so
  Torrbaby never starts its tunnel there.
- Family devices without Tailscale use Torrbaby's tunnel: a Network Extension packet tunnel.
- Transfers nearly always go direct (punched); the relay through bronze is only the fallback.

**Design.**
- `NEPacketTunnelProvider` app extension (iPhone/iPad; later Mac). Split tunnel: Diamond gets
  a private address (10.233.0.1) and only that /32 is routed into the tunnel. No DNS change,
  no other traffic touched.
- The extension runs the existing Rust tunnel (`Apple/Direct`): it punches with bronze's STUN,
  signals Diamond over the relay, and runs QUIC on the punched socket, relay as fallback.
- A userspace TCP stack in Rust (`ipstack`) turns the tunnel's IP packets into TCP streams;
  each becomes a QUIC stream that Diamond already pipes to its HTTP port. **No server change**
  beyond what the direct path has.
- **The goal is the direct path held open while Torrbaby is suspended.** The extension owns
  the punched socket and its QUIC connection; iOS keeps the extension running when the app is
  suspended, so the path stays open (keepalives keep both NATs' mappings) and is punched again
  from the extension on a network change. The relay is only for while there is no direct path.
- Battery: keepalives run while the path is in use (a transfer pending or the app open) and
  for a while after; when idle for long the path may be let go and punched again on the next
  request (2–3 s). Tune once measured.
- Background downloads (and the app and the player) use `http://10.233.0.1:8765` on tunnel
  devices. A player's address no longer changes when the path does.
- The app shares the device's credentials and bronze's details with the extension through an
  App Group. Memory limit for packet tunnels: 50 MB.
- Settings → Connection: "Torrbaby tunnel: Automatic / On / Off". Automatic uses Tailscale
  when it reaches Diamond, otherwise the tunnel (iOS asks once to add the VPN configuration).

**Steps.**
0. Spike: extension + ipstack + QUIC over the relay; prove a background download completes
   while the app is suspended. (Relay first: no punching to move yet.)
1. Punching inside the extension (move the STUN, window/random spray and signalling out of
   `DirectTunnel.establish` into code both targets share), relay fallback, network changes.
2. App integration: route choice, download URLs, Settings (EN + 中文), Tailscale detection.
3. Hardening: memory, battery (keepalives only while busy), metering, the Mac.
