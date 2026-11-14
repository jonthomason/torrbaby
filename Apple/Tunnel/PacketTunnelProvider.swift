import Foundation
import Network
import NetworkExtension
import os

// Apple/Direct (Rust), linked statically: the TCP stack and the QUIC paths.
@_silgen_name("tbtunnel_start") private func tbtunnel_start(_ mtu: UInt16, _ write: @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<UInt8>?, Int) -> Void, _ context: UnsafeMutableRawPointer?) -> Bool
@_silgen_name("tbtunnel_input") private func tbtunnel_input(_ packet: UnsafePointer<UInt8>?, _ length: Int) -> Bool
@_silgen_name("tbtunnel_stop") private func tbtunnel_stop()
@_silgen_name("tbtunnel_alive") private func tbtunnel_alive() -> Bool
@_silgen_name("tbdirect_start_lean") private func tbdirect_start(_ fd: Int32, _ server: UnsafePointer<CChar>, _ fingerprint: UnsafePointer<CChar>) -> UInt16
@_silgen_name("tbdirect_alive") private func tbdirect_alive() -> Bool
@_silgen_name("tbdirect_stop") private func tbdirect_stop()
@_silgen_name("tbdirect_relay_start_lean") private func tbdirect_relay_start(_ fd: Int32, _ server: UnsafePointer<CChar>, _ fingerprint: UnsafePointer<CChar>) -> UInt16
@_silgen_name("tbdirect_relay_alive") private func tbdirect_relay_alive() -> Bool
@_silgen_name("tbdirect_relay_stop") private func tbdirect_relay_stop()

/// Torrbaby's packet tunnel: Diamond at 10.233.0.1, reached straight through both NATs.
///
/// iOS keeps this extension running while Torrbaby is suspended, so the direct path it holds
/// stays open and background downloads (fetched by iOS itself) go straight to Diamond. Only
/// 10.233.0.1 is routed here; nothing else on the device is touched. The relay through bronze
/// carries the signalling, and streams only while there is no direct path.
final class PacketTunnelProvider: NEPacketTunnelProvider, @unchecked Sendable {
    static let diamond = "10.233.0.1"
    private let log = Logger(subsystem: "org.torrbaby", category: "tunnel")
    private var relay = "", relayFingerprint = "", stun = "", token = ""
    private var relayPort: UInt16 = 0
    private var monitor: NWPathMonitor?
    private var keeper: Task<Void, Never>?
    private var networkSignature = ""
    /// Public addresses a predicted spray failed from: the next attempt there sprays random ports.
    private var windowFailedFrom: Set<String> = []
    private var lastAttempt = Date.distantPast
    private let started = Date()
    private var attempting = false

    /// Logged to the system and to the App Group log the app shows on Settings → Connection.
    private func note(_ line: String) { log.notice("\(line, privacy: .public)"); TunnelLog.write(line) }

    /// Memory in use (iOS ends a packet tunnel extension above 50 MB).
    private static func memoryMB() -> Double {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
        let result = withUnsafeMutablePointer(to: &info) { $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count) } }
        return result == KERN_SUCCESS ? Double(info.phys_footprint) / 1_048_576 : -1
    }

    override func startTunnel(options: [String: NSObject]?, completionHandler: @escaping (Error?) -> Void) {
        note("starting (memory \(String(format: "%.1f", Self.memoryMB())) MB)")
        let configuration = (protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration ?? [:]
        relay = configuration["relay"] as? String ?? ""
        relayFingerprint = configuration["relayFingerprint"] as? String ?? ""
        stun = configuration["stun"] as? String ?? ""
        token = configuration["token"] as? String ?? ""
        guard !relay.isEmpty, relayFingerprint.count == 64, !token.isEmpty else {
            note("cannot start: configuration incomplete (relay \(relay.isEmpty ? "missing" : relay), fingerprint \(relayFingerprint.count) chars, token \(token.isEmpty ? "missing" : "present"))")
            completionHandler(NEVPNError(.configurationInvalid)); return
        }
        note("configuration: relay \(relay), STUN \(stun)")
        let settings = NEPacketTunnelNetworkSettings(tunnelRemoteAddress: String(relay.split(separator: ":").first ?? ""))
        let ipv4 = NEIPv4Settings(addresses: ["10.233.0.2"], subnetMasks: ["255.255.255.255"])
        ipv4.includedRoutes = [NEIPv4Route(destinationAddress: Self.diamond, subnetMask: "255.255.255.255")]
        settings.ipv4Settings = ipv4
        settings.mtu = 1280
        setTunnelNetworkSettings(settings) { [weak self] error in
            guard let self else { return }
            if let error { self.note("network settings refused: \(error.localizedDescription)"); completionHandler(error); return }
            self.note("network settings applied; starting the TCP stack")
            guard self.startStack() else { self.note("TCP stack did not start"); completionHandler(NEVPNError(.configurationUnknown)); return }
            self.readPackets()
            self.watchNetwork()
            self.keep()
            self.note("up: Diamond at \(Self.diamond)")
            completionHandler(nil)
        }
    }

    override func stopTunnel(with reason: NEProviderStopReason, completionHandler: @escaping () -> Void) {
        note("stopping: reason \(reason.rawValue) (\(Self.describe(reason)))")
        keeper?.cancel(); monitor?.cancel()
        tbtunnel_stop(); tbdirect_stop(); tbdirect_relay_stop()
        completionHandler()
    }

    /// The app asks for the tunnel's state ("status") or a fresh direct path ("punch").
    override func handleAppMessage(_ messageData: Data, completionHandler: ((Data?) -> Void)?) {
        let message = String(data: messageData, encoding: .utf8) ?? ""
        if message == "punch" { lastAttempt = .distantPast }
        // One line per part, for Settings → Connection.
        let direct = tbdirect_alive() ? "Direct path: open, \(punched)" : "Direct path: not open" + (status.isEmpty ? "" : " (\(status))")
        let state = direct + "\nRelay via bronze: " + (tbdirect_relay_alive() ? "open" : "closed")
        completionHandler?(state.data(using: .utf8))
    }

    /// Why the last attempt failed, or what it is doing; empty once open.
    private var status = "starting"
    /// How the open direct path was made ("punched from … in … ms (window)").
    private var punched = ""

    private static func describe(_ reason: NEProviderStopReason) -> String {
        switch reason {
        case .userInitiated: "disconnected by the user or the app"
        case .providerFailed: "the extension failed"
        case .noNetworkAvailable: "no network"
        case .unrecoverableNetworkChange: "unrecoverable network change"
        case .providerDisabled: "configuration disabled"
        case .configurationDisabled: "configuration disabled"
        case .configurationRemoved: "configuration removed"
        case .superceded: "another VPN connected"
        case .userLogout: "user logged out"
        case .userSwitch: "user switched"
        case .connectionFailed: "connection failed"
        case .sleep: "device asleep"
        case .appUpdate: "app updated"
        case .idleTimeout: "idle timeout"
        default: "other"
        }
    }

    private func readPackets() {
        packetFlow.readPackets { [weak self] packets, _ in
            for packet in packets { packet.withUnsafeBytes { _ = tbtunnel_input($0.bindMemory(to: UInt8.self).baseAddress, packet.count) } }
            self?.readPackets()
        }
    }

    /// A different network (Wi-Fi joined or left, another carrier): both paths were opened
    /// from the old one, so they are dropped and opened again from here.
    private func watchNetwork() {
        let monitor = NWPathMonitor()
        monitor.pathUpdateHandler = { [weak self] path in
            guard let self else { return }
            PunchProbe.preferredInterface = path.availableInterfaces.first { $0.type == .wifi || $0.type == .cellular }?.name
            let signature = PunchProbe.networkSignature()
            if signature != self.networkSignature {
                if !self.networkSignature.isEmpty {
                    self.note("network changed (\(self.networkSignature) → \(signature)): dropping both paths")
                    tbdirect_stop(); tbdirect_relay_stop()
                    self.lastAttempt = .distantPast
                }
                self.networkSignature = signature
            }
        }
        monitor.start(queue: DispatchQueue(label: "org.torrbaby.tunnel.path"))
        self.monitor = monitor
    }

    /// Keep the relay and the direct path open: check every few seconds, open what is closed.
    private func keep() {
        keeper = Task.detached { [weak self] in
            var ticks = 0
            while !Task.isCancelled {
                await self?.maintain()
                ticks += 1
                if ticks % 10 == 0 { self?.note("memory \(String(format: "%.1f", Self.memoryMB())) MB, direct \(tbdirect_alive() ? "open" : "closed"), relay \(tbdirect_relay_alive() ? "open" : "closed")") }
                try? await Task.sleep(for: .seconds(3))
            }
        }
    }

    /// The TCP stack that turns the tunnel's packets into streams (Rust).
    private func startStack() -> Bool {
        let context = Unmanaged.passUnretained(self).toOpaque()
        return tbtunnel_start(1280, { context, packet, length in
            guard let context, let packet else { return }
            let provider = Unmanaged<PacketTunnelProvider>.fromOpaque(context).takeUnretainedValue()
            provider.packetFlow.writePackets([Data(bytes: packet, count: length)], withProtocols: [NSNumber(value: AF_INET)])
        }, context)
    }

    private func maintain() async {
        // Started a moment ago, the stack's task may not have marked itself running yet.
        if !tbtunnel_alive(), Date().timeIntervalSince(started) > 5 {
            note("TCP stack stopped: starting it again")
            _ = startStack()
        }
        if !tbdirect_relay_alive() { await openRelay() }
        guard !tbdirect_alive(), !attempting, tbdirect_relay_alive() else { return }
        // A failure waits a minute before the next try; a path that dropped is tried at once.
        guard Date().timeIntervalSince(lastAttempt) > 60 else { return }
        attempting = true; lastAttempt = Date()
        note("punching a direct path (memory \(String(format: "%.1f", Self.memoryMB())) MB)")
        defer { attempting = false }
        do {
            let port = relayPort, token = token
            let punched = try await DirectPunch.run(stun: stun, randomFrom: windowFailedFrom, start: { body in
                try await Self.post(port: port, token: token, path: "/v1/direct", body: body)
            }, report: { [weak self] in self?.note("direct: \($0.step)") })
            note("punched: \(punched.summary); opening QUIC")
            let opened = punched.server.withCString { s in punched.fingerprint.withCString { f in tbdirect_start(punched.fd, s, f) } }
            if opened == 0 { throw DirectPunch.Failure(message: "QUIC did not open on the punched socket") }
            status = ""
            self.punched = punched.summary
            note("direct open (memory \(String(format: "%.1f", Self.memoryMB())) MB)")
        } catch {
            if let address = (error as? DirectPunch.Failure)?.windowFailedAt { windowFailedFrom.insert(address) }
            status = "last try failed: \(error.localizedDescription)"
            note("direct failed: \(error.localizedDescription)")
        }
    }

    private func openRelay() async {
        let probe = PunchProbe(count: 1)
        defer { probe.close() }
        guard probe.ready, let fd = probe.take(0) else { note("relay: no socket"); return }
        note("relay: connecting to \(relay) via \(probe.interface ?? "default")")
        let relay = relay, fingerprint = relayFingerprint
        let port = await Task.detached { relay.withCString { a in fingerprint.withCString { f in tbdirect_relay_start(fd, a, f) } } }.value
        relayPort = port
        if port == 0 { status = "relay: could not reach \(relay)"; note("relay: could not reach \(relay)") }
        else { note("relay open (loopback port \(port))") }
    }

    /// A request to Diamond over the relay's loopback port, with the device's credentials.
    private static func post<T: Decodable>(port: UInt16, token: String, path: String, body: [String: Any]) async throws -> T {
        var request = URLRequest(url: URL(string: "http://127.0.0.1:\(port)\(path)")!)
        request.httpMethod = "POST"
        request.timeoutInterval = 30
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, response) = try await URLSession.shared.data(for: request)
        guard (response as? HTTPURLResponse)?.statusCode == 200 else {
            throw DirectPunch.Failure(message: String(data: data, encoding: .utf8) ?? "Diamond refused")
        }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try decoder.decode(T.self, from: data)
    }
}
