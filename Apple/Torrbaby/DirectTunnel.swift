#if os(iOS)
import Foundation

/// The in-app half of the direct connection (Apple/Direct, Rust), linked statically.
@_silgen_name("tbdirect_start") private func tbdirect_start(_ fd: Int32, _ server: UnsafePointer<CChar>, _ fingerprint: UnsafePointer<CChar>) -> UInt16
@_silgen_name("tbdirect_alive") private func tbdirect_alive() -> Bool
@_silgen_name("tbdirect_stop") private func tbdirect_stop()
@_silgen_name("tbdirect_relay_start") private func tbdirect_relay_start(_ fd: Int32, _ server: UnsafePointer<CChar>, _ fingerprint: UnsafePointer<CChar>) -> UInt16
@_silgen_name("tbdirect_relay_alive") private func tbdirect_relay_alive() -> Bool
@_silgen_name("tbdirect_relay_stop") private func tbdirect_relay_stop()
@_silgen_name("tbdirect_relay_info") private func tbdirect_relay_info(_ buffer: UnsafeMutablePointer<CChar>, _ capacity: Int) -> Int32

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

    /// Bronze's view, over the relay: whether Diamond is connected, and this device's public
    /// address ("ip:port"). Behind a hotspot this is the only way to see the carrier change.
    static func info() async -> (homeOnline: Bool, publicAddress: String)? {
        await Task.detached { () -> (Bool, String)? in
            var buffer = [CChar](repeating: 0, count: 256)
            guard tbdirect_relay_info(&buffer, buffer.count) > 0 else { return nil }
            let text = String(cString: buffer)
            var online = false, address = ""
            for line in text.split(separator: "\n") {
                if line.hasPrefix("home ") { online = line == "home online" }
                if line.hasPrefix("you ") { address = String(line.dropFirst(4)) }
            }
            return address.isEmpty ? nil : (online, address)
        }.value
    }

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
    /// Public addresses a punch has failed from with a predicted window or exact ports: the
    /// next attempt from there sprays random ports instead.
    @MainActor static var windowFailedFrom: Set<String> = []

    /// Punch (DirectPunch, shared with the packet tunnel), with Diamond asked over `signal`
    /// (the relay or Tailscale), and open the tunnel on the socket that got through. Each step
    /// is recorded in `store.directAttempt` for Settings → Connection, and a failure says
    /// which step failed and why.
    @MainActor static func establish(store: Store, signal: String) async throws -> DirectTunnel {
        var attempt = Store.DirectAttempt()
        attempt.signal = store.routeLabel(signal)
        func record(_ change: (inout Store.DirectAttempt) -> Void) { change(&attempt); store.directAttempt = attempt }
        func fail(_ why: String) -> Failure {
            record { $0.failure = why; $0.finished = Date(); $0.ok = false }
            return Failure(message: why)
        }
        guard let stun = store.stunServer else { throw fail("bronze's address is not known yet (it comes with the online check or the pairing code)") }
        let label = attempt.signal
        let punched: DirectPunch.Punched
        do {
            punched = try await DirectPunch.run(stun: stun, randomFrom: windowFailedFrom, start: { body in
                try await store.request("POST", "/v1/direct", body: body, timeout: 30, address: signal)
            }, report: { r in
                record {
                    $0.step = r.step.hasPrefix("asking Diamond") ? "asking Diamond to start (over \(label))" : r.step
                    $0.interface = r.interface; $0.publicAddress = r.publicAddress; $0.asked = r.asked; $0.answered = r.answered
                    $0.diagnosis = r.diagnosis; $0.spread = r.spread; $0.window = r.window; $0.server = r.server; $0.session = r.session; $0.punchMs = r.punchMs
                }
            })
        } catch let failure as DirectPunch.Failure {
            if let session = failure.session {
                let view = await serverView(store: store, signal: signal, id: session)
                record { $0.serverView = view }
            }
            if let address = failure.windowFailedAt { windowFailedFrom.insert(address) }
            throw fail(failure.message)
        }
        // QUIC on the winning socket itself; the library owns the socket from here.
        let server = punched.server, fingerprint = punched.fingerprint, fd = punched.fd
        let quicStart = Date()
        let port = await Task.detached { () -> UInt16 in
            server.withCString { s in fingerprint.withCString { f in tbdirect_start(fd, s, f) } }
        }.value
        guard port != 0 else {
            if let session = attempt.session { let view = await serverView(store: store, signal: signal, id: session); record { $0.serverView = view } }
            throw fail("Punched, but QUIC did not open on the punched socket within 8 s")
        }
        record { $0.quicMs = Int(Date().timeIntervalSince(quicStart) * 1000); $0.step = "open"; $0.ok = true; $0.finished = Date() }
        let tunnel = DirectTunnel()
        tunnel.port = port
        tunnel.how = "QUIC on the punched socket"
        tunnel.watch()
        return tunnel
    }

    /// What Diamond saw of a failed attempt (its state, error and where QUIC came from).
    @MainActor private static func serverView(store: Store, signal: String, id: String) async -> String? {
        struct View: Decodable { let state: String?; let error: String?; let device: String?; let firstQuicFrom: String?; let punchedAfterMs: Int?; let otherSources: [String]? }
        guard let view: View = try? await store.request("GET", "/v1/direct/\(id)", timeout: 8, address: signal) else { return nil }
        var parts = ["state \(view.state ?? "?")"]
        if let device = view.device { parts.append("heard this device from \(device)") }
        if let ms = view.punchedAfterMs { parts.append("after \(ms) ms") }
        if let from = view.firstQuicFrom { parts.append("QUIC came from \(from)") }
        if let others = view.otherSources, !others.isEmpty { parts.append("punch packets from other addresses: \(others.prefix(3).joined(separator: ", "))") }
        if let error = view.error { parts.append(error) }
        return parts.joined(separator: ", ")
    }

    var alive: Bool { tbdirect_alive() }

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
