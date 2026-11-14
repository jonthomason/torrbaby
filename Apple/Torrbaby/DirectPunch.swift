#if os(iOS)
import Foundation

/// One attempt at a direct path to Diamond, without the app around it: the packet tunnel
/// extension runs this (the app's own attempt, DirectTunnel.establish, records more for
/// Settings → Connection). The steps are the same: STUN from many sockets through bronze,
/// compare bronze's two STUN ports to see whether the NAT gives each destination its own
/// port, ask Diamond to start (over the relay), fire from every socket while Diamond sprays,
/// and hand over the socket that got through.
enum DirectPunch {
    struct Failure: LocalizedError { let message: String; var errorDescription: String? { message } }
    struct Started: Decodable { let id: String; let server: String; let fingerprint: String; let low: Int; let high: Int; let spread: String? }
    struct Punched { let fd: Int32; let server: String; let fingerprint: String; let publicAddress: String; let summary: String }

    /// `stun` is bronze's STUN server ("ip:3478"); `start` sends Diamond the request (POST
    /// /v1/direct) and returns its answer; `step` hears each step, for the log.
    static func run(stun: String, randomSpray: Bool, start: ([String: Any]) async throws -> Started, step: (String) -> Void) async throws -> Punched {
        let probe = PunchProbe(count: 192)
        defer { probe.close() }
        guard probe.ready, probe.interface != nil else { throw Failure(message: "no Wi-Fi or cellular network to punch from") }
        step("STUN through \(stun) from \(probe.interface ?? "?")")
        let mappings = await Task.detached { probe.discover(stun: stun, limit: 12) }.value
        guard let ip = mappings.first?.ip else { throw Failure(message: "bronze's STUN server did not answer") }
        let ports = mappings.map(\.port).sorted()
        let span = ports.last! - ports.first!
        let low = max(1, ports.first!), high = min(65535, ports.last! + span + 1024)
        let host = stun.split(separator: ":").first.map(String.init) ?? ""
        let second = await Task.detached { probe.discover(stun: "\(host):3479", limit: 6) }.value
        let first = Dictionary(mappings.map { ($0.socket, $0) }, uniquingKeysWith: { a, _ in a })
        let perDestination = second.contains { other in first[other.socket].map { $0.port != other.port || $0.ip != other.ip } ?? false }
        let random = randomSpray || perDestination
        var body: [String: Any] = ["ip": ip, "low": low, "high": high,
                                   "diagnosis": (perDestination ? "NAT gives each destination its own port" : "same port for every destination") + " (packet tunnel)"]
        if random { body["spread"] = "random" }
        step("asking Diamond to start from \(ip), " + (random ? "random spray" : "window \(low)–\(high)"))
        let started: Started
        do { started = try await start(body) } catch { throw Failure(message: "could not ask Diamond to start: \(error.localizedDescription)") }
        step("punching at \(started.server)")
        let began = Date()
        guard let winner = await Task.detached(operation: { probe.punch(at: started.server, id: started.id, word: "TBDIRECT1", seconds: 20) }).value,
              let fd = probe.take(winner) else {
            throw Failure(message: "no packet got through both NATs in 20 s" + (started.spread == "random" ? " (random spray)" : " (window)"))
        }
        let ms = Int(Date().timeIntervalSince(began) * 1000)
        return Punched(fd: fd, server: started.server, fingerprint: started.fingerprint, publicAddress: ip,
                       summary: "punched from \(ip) in \(ms) ms (" + (started.spread == "random" ? "random spray" : "window") + ")")
    }
}
#endif
