#if os(iOS)
import Foundation

// Apple/Direct (Rust): the whole punch, shared with the packet tunnel and other clients.
@_silgen_name("tbdirect_punch") private func tbdirect_punch(_ request: UnsafePointer<CChar>, _ report: @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<CChar>?) -> Void, _ context: UnsafeMutableRawPointer?) -> UnsafeMutablePointer<CChar>?
@_silgen_name("tbdirect_free") private func tbdirect_free(_ text: UnsafeMutablePointer<CChar>?)

/// One attempt at a direct path to Diamond: the app (DirectTunnel.establish, which shows each
/// step on Settings → Connection) and the packet tunnel extension both call it. The punching
/// itself is in the Rust library (Apple/Direct/src/punch.rs), so Android and Miyu Code share
/// it: STUN through bronze on both its ports, the NAT's behaviour, asking Diamond to start over
/// the route given, firing from every socket while Diamond sprays, and QUIC on the socket that
/// got through. On success the direct tunnel is open and its loopback port returned.
enum DirectPunch {
    struct Failure: LocalizedError {
        let message: String
        /// Diamond's session, when it was asked to start (its view explains a failed punch).
        var session: String? = nil
        /// This network's public address, when a predicted spray failed from it: the next
        /// attempt from there should spray random ports.
        var windowFailedAt: String? = nil
        var errorDescription: String? { message }
    }
    struct Punched { let port: UInt16; let publicAddress: String; let summary: String; let quicMs: Int?; let report: Report }

    /// What the attempt has found so far, reported as it goes.
    struct Report: Decodable {
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

    private final class Listener {
        let report: (Report) -> Void
        init(_ report: @escaping (Report) -> Void) { self.report = report }
    }
    private static let decoder: JSONDecoder = { let d = JSONDecoder(); d.keyDecodingStrategy = .convertFromSnakeCase; return d }()

    /// `stun` is bronze's STUN server ("ip:3478"); `signal` the route to ask Diamond over
    /// ("http://host:port": the relay's loopback port, or Tailscale), with `token`;
    /// `randomFrom` public addresses a predicted spray failed from before; `lean` the packet
    /// tunnel's smaller buffers. `report` hears each step, on the punching thread.
    static func run(stun: String, signal: String, token: String, randomFrom: Set<String>, lean: Bool, report: @escaping (Report) -> Void) async throws -> Punched {
        let request: [String: Any] = ["stun": stun, "signal": signal, "token": token, "interface": PunchProbe.physicalInterface() ?? "",
                                      "random_from": Array(randomFrom), "sockets": 192, "lean": lean]
        guard let json = try? JSONSerialization.data(withJSONObject: request), let text = String(data: json, encoding: .utf8) else {
            throw Failure(message: "Could not prepare the punch")
        }
        let listener = Unmanaged.passRetained(Listener(report))
        let reply: String? = await Task.detached {
            defer { listener.release() }
            let result = text.withCString { tbdirect_punch($0, { context, line in
                guard let context, let line, let data = String(cString: line).data(using: .utf8),
                      let step = try? DirectPunch.decoder.decode(Report.self, from: data) else { return }
                Unmanaged<Listener>.fromOpaque(context).takeUnretainedValue().report(step)
            }, listener.toOpaque()) }
            defer { tbdirect_free(result) }
            return result.map { String(cString: $0) }
        }.value
        struct Reply: Decodable {
            let ok: Bool; let port: UInt16?; let summary: String?; let publicAddress: String?; let quicMs: Int?
            let failure: String?; let session: String?; let windowFailedAt: String?; let report: Report?
        }
        guard let reply, let data = reply.data(using: .utf8), let result = try? decoder.decode(Reply.self, from: data) else {
            throw Failure(message: "The punch gave no answer")
        }
        guard result.ok, let port = result.port else {
            throw Failure(message: result.failure ?? "The punch failed", session: result.session, windowFailedAt: result.windowFailedAt)
        }
        return Punched(port: port, publicAddress: result.publicAddress ?? "", summary: result.summary ?? "", quicMs: result.quicMs, report: result.report ?? Report())
    }
}
#endif
