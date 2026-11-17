#if os(iOS)
import Foundation

@_silgen_name("tbproxy_start") private func tbproxy_start(_ config: UnsafePointer<CChar>) -> UInt16
@_silgen_name("tbproxy_running") private func tbproxy_running() -> Bool
@_silgen_name("tbproxy_stop") private func tbproxy_stop()
@_silgen_name("tbproxy_network") private func tbproxy_network(_ signature: UnsafePointer<CChar>, _ interface: UnsafePointer<CChar>)
@_silgen_name("tbproxy_resumed") private func tbproxy_resumed()
@_silgen_name("tbproxy_home") private func tbproxy_home(_ address: UnsafePointer<CChar>)
@_silgen_name("tbproxy_state") private func tbproxy_state(_ buffer: UnsafeMutablePointer<CChar>, _ capacity: Int) -> Int32
@_silgen_name("tbdirect_identity") private func tbdirect_identity(_ fingerprint: UnsafePointer<CChar>)
@_silgen_name("tbdirect_client") private func tbdirect_client(_ id: UnsafePointer<CChar>)

/// Torrbaby's link to Diamond away from home (Apple/Direct proxy.rs, on ABT's shared abt-link):
/// one stable loopback port whose every connection goes to Diamond by the best path that works
/// now (the direct path once punched and healthy, else the relay through bronze, end to end to
/// Diamond's identity). The app and the packet tunnel both use it; ABT, when on, carries
/// everything instead and the link is not started.
enum TorrbabyLink {
    /// What the link says about itself (proxy::state).
    struct State: Decodable, Equatable {
        struct Punch: Decodable, Equatable { let ok: Bool; let summary: String; let at: Double?; let nextTry: Double? }
        struct Bytes: Decodable, Equatable { let sent: Int64; let received: Int64 }
        /// "home", "direct", "relay", "down" or "stopped".
        let state: String
        let rttMs: Int?
        let lastAnsweredS: Int?
        /// "quic", "tcp", "closed" or "dormant".
        let relay: String?
        let relayLine: String?
        let publicIp: String?
        /// Diamond's public address, as bronze sees it: the same as this device's means at home.
        let homeIp: String?
        let behindDiamondsNat: Bool?
        let keeper: String?
        let punching: String?
        let note: String?
        let lastPunch: Punch?
        let bytes: [String: Bytes]?
        let notes: [String]?
    }
    private static let decoder: JSONDecoder = { let d = JSONDecoder(); d.keyDecodingStrategy = .convertFromSnakeCase; return d }()

    /// Start the link (or, already running, take the new token) and return its loopback port;
    /// nil when bronze's relay did not open. Blocks up to about 7 s, off the main thread.
    static func start(relay: String, fingerprint: String, stun: String?, token: String, stateDir: URL?, lean: Bool) async -> UInt16? {
        network(DeviceNetwork.signature(), interface: DeviceNetwork.physicalInterface() ?? "")
        var config: [String: Any] = ["relay": relay, "relay_fingerprint": fingerprint, "token": token, "lean": lean]
        if let stun { config["stun"] = stun }
        if let stateDir { config["state_dir"] = stateDir.path }
        guard let json = try? JSONSerialization.data(withJSONObject: config), let text = String(data: json, encoding: .utf8) else { return nil }
        let port = await Task.detached { text.withCString { tbproxy_start($0) } }.value
        return port == 0 ? nil : port
    }
    static var running: Bool { tbproxy_running() }
    static func stop() { tbproxy_stop() }
    static func network(_ signature: String, interface: String) { signature.withCString { s in interface.withCString { i in tbproxy_network(s, i) } } }
    static func resumed() { tbproxy_resumed() }
    /// Diamond's address on this network while at home ("ip:port"); nil: away.
    static func home(_ address: String?) { (address ?? "").withCString { tbproxy_home($0) } }
    static func identity(_ fingerprint: String) { fingerprint.withCString { tbdirect_identity($0) } }
    static func client(_ id: String) { id.withCString { tbdirect_client($0) } }

    /// The link now; nil while it has never started.
    static func state() async -> State? {
        await Task.detached { () -> State? in
            var buffer = [CChar](repeating: 0, count: 32 * 1024)
            guard tbproxy_state(&buffer, buffer.count) > 0, let data = String(cString: buffer).data(using: .utf8) else { return nil }
            return try? decoder.decode(State.self, from: data)
        }.value
    }
}
#endif
