#if os(iOS)
import Foundation
import Network
import CryptoKit

/// Torrbaby's own direct connection to the server, for when this device is away from home
/// and Tailscale can only relay (the server half is server/direct.rs). The device punches a
/// path through both carriers' NAT (as the Direct connection test does), then speaks QUIC
/// from the winning socket's port so the hole stays the same. A local proxy on 127.0.0.1
/// turns each connection the app or the player makes into a QUIC stream that the server
/// pipes to its own HTTP port: requests are ordinary Torrbaby requests with this device's
/// credentials, only the route is shorter. Sockets are bound to the real network, so a
/// Tailscale exit node does not carry it abroad. If the path fails, the app falls back to
/// Tailscale and tries again later.
final class DirectTunnel: @unchecked Sendable {
    private let queue = DispatchQueue(label: "org.torrbaby.direct")
    private var group: NWConnectionGroup?
    private var listener: NWListener?
    private var closed = false
    private(set) var port: UInt16 = 0
    /// Called once, on the main actor, when the path is gone.
    var onClose: (@MainActor () -> Void)?

    struct Failure: LocalizedError { let message: String; var errorDescription: String? { message } }
    private struct Started: Decodable { let id: String; let server: String; let fingerprint: String; let low: Int; let high: Int }

    /// Punch, ask the server to listen, and open the tunnel. `signal` is the route the request
    /// to the server takes (the Tailscale one).
    @MainActor static func establish(store: Store, signal: String) async throws -> DirectTunnel {
        let probe = PunchProbe(count: 192)
        defer { probe.close() }
        guard probe.ready, let interface = probe.interface else { throw Failure(message: "No network for a direct path") }
        let mappings = await Task.detached { probe.discover() }.value
        guard let ip = mappings.first?.ip else { throw Failure(message: "STUN: no answer") }
        let ports = mappings.map(\.port).sorted()
        let span = ports.last! - ports.first!
        let low = max(1, ports.first!), high = min(65535, ports.last! + span + 1024)
        let started: Started = try await store.request("POST", "/v1/direct", body: ["ip": ip, "low": low, "high": high], timeout: 30, address: signal)
        guard let winner = await Task.detached(operation: { probe.punch(at: started.server, id: started.id, word: "TBDIRECT1", seconds: 20) }).value,
              let localPort = probe.localPort(winner) else { throw Failure(message: "No direct path") }
        // The winning socket's port is taken over by QUIC: close the probe sockets first.
        probe.close()
        let tunnel = DirectTunnel()
        try await tunnel.open(server: started.server, localPort: localPort, wifi: interface == "en0", fingerprint: started.fingerprint)
        return tunnel
    }

    private func open(server: String, localPort: UInt16, wifi: Bool, fingerprint: String) async throws {
        let parts = server.split(separator: ":")
        guard parts.count == 2, let remotePort = NWEndpoint.Port(String(parts[1])), let local = NWEndpoint.Port(rawValue: localPort) else {
            throw Failure(message: "Bad server address")
        }
        let quic = NWProtocolQUIC.Options(alpn: ["tbdirect"])
        quic.idleTimeout = 45_000
        // The server's own certificate, its fingerprint learned over the trusted connection.
        sec_protocol_options_set_verify_block(quic.securityProtocolOptions, { _, trust, complete in
            let secTrust = sec_trust_copy_ref(trust).takeRetainedValue()
            guard let chain = SecTrustCopyCertificateChain(secTrust) as? [SecCertificate], let leaf = chain.first else { complete(false); return }
            let digest = SHA256.hash(data: SecCertificateCopyData(leaf) as Data).map { String(format: "%02x", $0) }.joined()
            complete(digest == fingerprint)
        }, queue)
        let parameters = NWParameters(quic: quic)
        parameters.requiredInterfaceType = wifi ? .wifi : .cellular
        parameters.prohibitedInterfaceTypes = [.other]
        parameters.requiredLocalEndpoint = .hostPort(host: .ipv4(.any), port: local)
        parameters.allowLocalEndpointReuse = true
        let group = NWConnectionGroup(with: NWMultiplexGroup(to: .hostPort(host: NWEndpoint.Host(String(parts[0])), port: remotePort)), using: parameters)
        self.group = group
        try await withThrowingTaskGroup(of: Void.self) { race in
            race.addTask { [queue] in
                try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
                    var resumed = false
                    group.stateUpdateHandler = { [weak self] state in
                        switch state {
                        case .ready:
                            if !resumed { resumed = true; continuation.resume() }
                        case .failed(let error):
                            if !resumed { resumed = true; continuation.resume(throwing: error) } else { self?.close() }
                        case .cancelled:
                            if !resumed { resumed = true; continuation.resume(throwing: CancellationError()) } else { self?.close() }
                        default: break
                        }
                    }
                    group.start(queue: queue)
                }
            }
            race.addTask { try await Task.sleep(for: .seconds(8)); throw Failure(message: "QUIC did not start") }
            try await race.next()
            race.cancelAll()
        }
        // The local side: the app and the player connect here.
        let tcp = NWParameters.tcp
        tcp.requiredLocalEndpoint = .hostPort(host: .ipv4(.loopback), port: .any)
        tcp.acceptLocalOnly = true
        let listener = try NWListener(using: tcp)
        self.listener = listener
        listener.newConnectionHandler = { [weak self] connection in self?.carry(connection) }
        port = try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<UInt16, Error>) in
            var resumed = false
            listener.stateUpdateHandler = { [weak self] state in
                switch state {
                case .ready:
                    if !resumed, let port = listener.port?.rawValue { resumed = true; continuation.resume(returning: port) }
                case .failed(let error):
                    if !resumed { resumed = true; continuation.resume(throwing: error) } else { self?.close() }
                default: break
                }
            }
            listener.start(queue: queue)
        }
    }

    /// One local connection is one QUIC stream, both ways.
    private func carry(_ local: NWConnection) {
        guard !closed, let group, let stream = NWConnection(from: group) else { local.cancel(); return }
        local.start(queue: queue)
        stream.start(queue: queue)
        pump(from: local, to: stream)
        pump(from: stream, to: local)
    }
    private func pump(from: NWConnection, to: NWConnection) {
        from.receive(minimumIncompleteLength: 1, maximumLength: 256 * 1024) { [weak self] data, _, complete, error in
            let finish = complete || error != nil
            to.send(content: data, contentContext: finish ? .finalMessage : .defaultMessage, isComplete: finish, completion: .contentProcessed { sendError in
                if error != nil || sendError != nil { from.cancel(); to.cancel(); return }
                if !finish { self?.pump(from: from, to: to) }
            })
        }
    }

    func close() {
        queue.async { [self] in
            guard !closed else { return }
            closed = true
            listener?.cancel(); group?.cancel()
            let notify = onClose
            Task { @MainActor in notify?() }
        }
    }
}
#endif
