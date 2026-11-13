#if os(iOS)
import Foundation

/// The in-app half of the direct connection (Apple/Direct, Rust), linked statically.
@_silgen_name("tbdirect_start") private func tbdirect_start(_ fd: Int32, _ server: UnsafePointer<CChar>, _ fingerprint: UnsafePointer<CChar>) -> UInt16
@_silgen_name("tbdirect_alive") private func tbdirect_alive() -> Bool
@_silgen_name("tbdirect_stop") private func tbdirect_stop()
@_silgen_name("tbdirect_relay_start") private func tbdirect_relay_start(_ fd: Int32, _ server: UnsafePointer<CChar>, _ fingerprint: UnsafePointer<CChar>) -> UInt16
@_silgen_name("tbdirect_relay_alive") private func tbdirect_relay_alive() -> Bool
@_silgen_name("tbdirect_relay_stop") private func tbdirect_relay_stop()

/// The relay through Torrbaby's rendezvous on bronze, for when this device has no home
/// network, no Tailscale and no direct path yet (bronze's half is rendezvous/, Diamond keeps
/// a standing connection to it). QUIC to bronze, pinned to its certificate; a loopback proxy
/// turns each connection into a stream that bronze splices to Diamond. Requests are ordinary
/// Torrbaby requests with this device's credentials. Signalling (the online check, the
/// direct path's request) rides it, then traffic moves to the direct tunnel when that opens.
/// The socket is bound to the real network, so a Tailscale exit node does not carry it.
final class RelayTunnel: @unchecked Sendable {
    private(set) var port: UInt16 = 0
    private(set) var interface: String?
    var onClose: (@MainActor () -> Void)?
    private var watcher: Task<Void, Never>?

    static func open(address: String, fingerprint: String) async -> RelayTunnel? {
        let probe = PunchProbe(count: 1)
        defer { probe.close() }
        guard probe.ready, let fd = probe.take(0) else { return nil }
        let port = await Task.detached { () -> UInt16 in
            address.withCString { a in fingerprint.withCString { f in tbdirect_relay_start(fd, a, f) } }
        }.value
        guard port != 0 else { return nil }
        let tunnel = RelayTunnel()
        tunnel.port = port
        tunnel.interface = probe.interface
        tunnel.watch()
        return tunnel
    }

    var alive: Bool { tbdirect_relay_alive() }

    private func watch() {
        watcher = Task.detached { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(3))
                if !tbdirect_relay_alive() {
                    let notify = self?.onClose
                    await MainActor.run { notify?() }
                    return
                }
            }
        }
    }

    func close() {
        watcher?.cancel(); watcher = nil
        onClose = nil
        tbdirect_relay_stop()
    }
}

/// Torrbaby's own direct connection to the server, for when this device is away from home
/// and Tailscale can only relay (the server half is server/direct.rs). The device punches a
/// path through both carriers' NAT with plain UDP sockets (as the Direct connection test
/// does), then QUIC runs on the very socket that got through, inside the app (iOS's own QUIC
/// sent nothing when told to reuse that port, and a fresh port fell outside the hole).
/// A loopback proxy turns each connection the app or the player makes into a QUIC stream that
/// the server pipes to its own HTTP port: requests are ordinary Torrbaby requests with this
/// device's credentials, only the route is shorter. The sockets are bound to the real
/// network, so a Tailscale exit node does not carry it abroad. If the path fails, the app
/// falls back to Tailscale and tries again later.
final class DirectTunnel: @unchecked Sendable {
    private(set) var port: UInt16 = 0
    /// How the tunnel came up, for the test screen.
    var how = ""
    /// Called once, on the main actor, when the path is gone.
    var onClose: (@MainActor () -> Void)?
    private var watcher: Task<Void, Never>?

    struct Failure: LocalizedError { let message: String; var errorDescription: String? { message } }
    private struct Started: Decodable { let id: String; let server: String; let fingerprint: String; let low: Int; let high: Int }

    /// Punch, ask the server to listen, and open the tunnel on the socket that got through.
    /// `signal` is the route the request to the server takes (the Tailscale one).
    @MainActor static func establish(store: Store, signal: String) async throws -> DirectTunnel {
        let probe = PunchProbe(count: 192)
        defer { probe.close() }
        guard probe.ready, probe.interface != nil else { throw Failure(message: "No network for a direct path") }
        guard let stun = store.stunServer else { throw Failure(message: "STUN: the rendezvous is not known yet") }
        let mappings = await Task.detached { probe.discover(stun: stun, limit: 12) }.value
        guard let ip = mappings.first?.ip else { throw Failure(message: "STUN: no answer") }
        let ports = mappings.map(\.port).sorted()
        let span = ports.last! - ports.first!
        let low = max(1, ports.first!), high = min(65535, ports.last! + span + 1024)
        let started: Started = try await store.request("POST", "/v1/direct", body: ["ip": ip, "low": low, "high": high], timeout: 30, address: signal)
        guard let winner = await Task.detached(operation: { probe.punch(at: started.server, id: started.id, word: "TBDIRECT1", seconds: 20) }).value,
              let fd = probe.take(winner) else { throw Failure(message: "No direct path") }
        // QUIC on the winning socket itself; the library owns the socket from here.
        let server = started.server, fingerprint = started.fingerprint
        let port = await Task.detached { () -> UInt16 in
            server.withCString { s in fingerprint.withCString { f in tbdirect_start(fd, s, f) } }
        }.value
        guard port != 0 else { throw Failure(message: "QUIC did not open on the punched socket") }
        let tunnel = DirectTunnel()
        tunnel.port = port
        tunnel.how = "QUIC on the punched socket"
        tunnel.watch()
        return tunnel
    }

    /// Notice when the path is gone (the server went quiet, the network changed).
    private func watch() {
        watcher = Task.detached { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(3))
                if !tbdirect_alive() {
                    let notify = self?.onClose
                    await MainActor.run { notify?() }
                    return
                }
            }
        }
    }

    func close() {
        watcher?.cancel(); watcher = nil
        onClose = nil
        tbdirect_stop()
    }
}
#endif
