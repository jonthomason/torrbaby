#if os(iOS)
import Foundation

/// One attempt at a direct path to Diamond, shared by the app (DirectTunnel.establish, which
/// shows each step on Settings → Connection) and the packet tunnel extension:
/// - STUN from many sockets through bronze, then the same sockets ask bronze's second STUN
///   port, to see whether this network's NAT gives each destination its own port.
/// - Ask Diamond to start (over the relay or Tailscale), telling it this network's public
///   ports: it sprays exactly those when the NAT keeps one port per socket but hands ports
///   out at random (T-Mobile US); the predicted window when they come in sequence (China
///   Telecom); random ports across the range when each destination gets its own (China Mobile).
/// - Fire from every socket while Diamond sprays, and hand over the socket that got through.
enum DirectPunch {
    struct Failure: LocalizedError {
        let message: String
        /// Diamond's session, when it was asked to start (its view explains a failed punch).
        var session: String? = nil
        /// This network's public address, when a predicted window or exact ports failed from
        /// it: the next attempt from there should spray random ports.
        var windowFailedAt: String? = nil
        var errorDescription: String? { message }
    }
    struct Started: Decodable { let id: String; let server: String; let fingerprint: String; let low: Int; let high: Int; let spread: String? }
    struct Punched { let fd: Int32; let server: String; let fingerprint: String; let publicAddress: String; let summary: String }

    /// What the attempt has found so far, reported as it goes.
    struct Report {
        var step = ""
        var interface: String?
        var publicAddress: String?
        var asked = 0, answered = 0
        var diagnosis: String?
        var spread: String?
        var window: String?
        var server: String?
        var session: String?
        var punchMs: Int?
    }

    /// `stun` is bronze's STUN server ("ip:3478"); `randomFrom` are public addresses a
    /// predicted spray failed from before (random ports from there); `start` sends Diamond the
    /// request (POST /v1/direct) and returns its answer; `report` hears each step.
    static func run(stun: String, randomFrom: Set<String>, start: ([String: Any]) async throws -> Started, report: (Report) -> Void) async throws -> Punched {
        var r = Report()
        func step(_ text: String, _ change: (inout Report) -> Void = { _ in }) { change(&r); r.step = text; report(r) }
        step("opening sockets")
        let probe = PunchProbe(count: 192)
        defer { probe.close() }
        step("opening sockets") { $0.interface = probe.interface }
        guard probe.ready, probe.interface != nil else { throw Failure(message: "No Wi-Fi or cellular network to punch from") }
        step("asking bronze's STUN server (\(stun)) for this network's public ports") { $0.asked = 12 }
        let mappings = await Task.detached { probe.discover(stun: stun, limit: 12) }.value
        guard let ip = mappings.first?.ip else { throw Failure(message: "bronze's STUN server did not answer: UDP to \(stun) may be blocked on this network") }
        let randomSpray = randomFrom.contains(ip)
        let ports = mappings.map(\.port).sorted()
        let span = ports.last! - ports.first!
        let low = max(1, ports.first!), high = min(65535, ports.last! + span + 1024)
        step("comparing ports toward bronze's two STUN ports") { $0.publicAddress = ip; $0.answered = mappings.count }
        let host = stun.split(separator: ":").first.map(String.init) ?? ""
        let second = await Task.detached { probe.discover(stun: "\(host):3479", limit: 6) }.value
        let first = Dictionary(mappings.map { ($0.socket, $0) }, uniquingKeysWith: { a, _ in a })
        let pairs = second.compactMap { other in first[other.socket].map { ($0, other) } }
        let perDestination = pairs.contains { $0.0.port != $0.1.port || $0.0.ip != $0.1.ip }
        let steps = zip(ports, ports.dropFirst()).map { $1 - $0 }.sorted()
        let medianStep = steps.isEmpty ? 0 : steps[steps.count / 2]
        // One port per socket, but handed out far apart: no window can be predicted, and none
        // is needed, since the ports STUN saw are the very ones Diamond will see.
        let exact = !perDestination && !pairs.isEmpty && medianStep > 512 && !randomSpray
        let random = randomSpray || perDestination
        var diagnosis = pairs.isEmpty ? "second STUN port did not answer" :
            (perDestination ? "NAT gives each destination its own port" : "same port for every destination") +
            " (" + pairs.prefix(3).map { "\($0.0.port)/\($0.1.port)" + ($0.0.ip != $0.1.ip ? " \($0.1.ip)" : "") }.joined(separator: ", ") + ")"
        diagnosis += ", median step \(medianStep)"
        if randomSpray && !perDestination { diagnosis += ", window failed here before" }
        let spread = exact ? "exact" : random ? "random" : "window"
        step("asking Diamond to start") {
            $0.diagnosis = diagnosis; $0.spread = spread
            $0.window = exact ? "the \(mappings.count) ports STUN saw (\(ports.first!)–\(ports.last!))"
                : random ? "2048 random ports across the whole range" : "\(low)–\(high) (STUN saw \(ports.first!)–\(ports.last!))"
        }
        var body: [String: Any] = ["ip": ip, "low": low, "high": high, "diagnosis": diagnosis, "ports": mappings.map(\.port)]
        if spread != "window" { body["spread"] = spread }
        let started: Started
        do { started = try await start(body) } catch { throw Failure(message: "Could not ask Diamond to start: \(error.localizedDescription)") }
        let sprays = started.spread == "random" ? "2048 random ports" : started.spread == "exact" ? "the \(mappings.count) ports STUN saw" : "\(started.low)–\(started.high)"
        step("punching: firing from 192 sockets at \(started.server) while Diamond sprays \(sprays)") { $0.server = started.server; $0.session = started.id }
        let began = Date()
        guard let winner = await Task.detached(operation: { probe.punch(at: started.server, id: started.id, word: "TBDIRECT1", seconds: 20) }).value,
              let fd = probe.take(winner) else {
            let why = started.spread == "random"
                ? "No packet got through both NATs in 20 s, even spraying random ports. This network may use a different public address toward Diamond, or block incoming UDP"
                : "No packet got through both NATs in 20 s spraying \(sprays); the next attempt from \(ip) sprays random ports"
            throw Failure(message: why, session: started.id, windowFailedAt: started.spread == "random" ? nil : ip)
        }
        let ms = Int(Date().timeIntervalSince(began) * 1000)
        step("punched; opening QUIC on the winning socket") { $0.punchMs = ms }
        return Punched(fd: fd, server: started.server, fingerprint: started.fingerprint, publicAddress: ip,
                       summary: "punched from \(ip) in \(ms) ms (\(started.spread ?? spread))")
    }
}
#endif
