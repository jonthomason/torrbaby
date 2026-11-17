import Foundation
import Network
import NetworkExtension
import os

// Apple/Direct (Rust), linked statically: the TCP stack (the link is TorrbabyLink.swift).
@_silgen_name("tbtunnel_start") private func tbtunnel_start(_ mtu: UInt16, _ write: @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<UInt8>?, UnsafePointer<Int>?, Int) -> Void, _ context: UnsafeMutableRawPointer?) -> Bool
@_silgen_name("tbtunnel_input") private func tbtunnel_input(_ packet: UnsafePointer<UInt8>?, _ length: Int) -> Bool
@_silgen_name("tbtunnel_stop") private func tbtunnel_stop()
@_silgen_name("tbtunnel_alive") private func tbtunnel_alive() -> Bool
@_silgen_name("tbtunnel_stats") private func tbtunnel_stats(_ buffer: UnsafeMutablePointer<CChar>, _ capacity: Int) -> Int32

/// Torrbaby's packet tunnel: Diamond at 10.233.0.1, for devices without ABT.
///
/// iOS keeps this extension running while Torrbaby is suspended, so its path to Diamond stays
/// open and background downloads (fetched by iOS itself) go straight to Diamond. Only
/// 10.233.0.1 is routed here; nothing else on the device is touched. Each connection goes to
/// Diamond through Torrbaby's link (Apple/Direct proxy.rs, on ABT's shared abt-link): Diamond on
/// this network at home, else the direct path once punched and healthy, else the relay through
/// bronze, end to end to Diamond's identity. The link keeps the relay, punches, follows network
/// changes and goes quiet when idle; the extension tells it the network and when it is at home.
final class PacketTunnelProvider: NEPacketTunnelProvider, @unchecked Sendable {
    static let diamond = "10.233.0.1"
    private let log = Logger(subsystem: "org.torrbaby", category: "tunnel")
    private var relay = "", relayFingerprint = "", stun = "", token = "", home = "", identity = ""
    private var monitor: NWPathMonitor?
    private var keeper: Task<Void, Never>?
    private let started = Date()
    /// Diamond on this network ("ip:port") while at home, and when it was last looked up.
    private var homeAddress = ""
    private var homeLookedUp = Date.distantPast
    private var lastLinkStart = Date.distantPast

    /// Diamond's home name looked up and connected to (1 s), as "ip:port"; nil when it does
    /// not answer here.
    private static func reachHome(_ home: String) -> String? {
        guard let colon = home.lastIndex(of: ":") else { return nil }
        let host = String(home[..<colon]), port = String(home[home.index(after: colon)...])
        var hints = addrinfo(); hints.ai_family = AF_INET; hints.ai_socktype = SOCK_STREAM
        var list: UnsafeMutablePointer<addrinfo>?
        guard getaddrinfo(host, port, &hints, &list) == 0, let first = list else { return nil }
        defer { freeaddrinfo(list) }
        for pointer in sequence(first: first, next: { $0.pointee.ai_next }) {
            let entry = pointer.pointee
            guard let address = entry.ai_addr else { continue }
            let fd = socket(AF_INET, SOCK_STREAM, 0)
            guard fd >= 0 else { continue }
            defer { close(fd) }
            _ = fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK)
            if connect(fd, address, entry.ai_addrlen) != 0 && errno != EINPROGRESS { continue }
            var poll = pollfd(fd: fd, events: Int16(POLLOUT), revents: 0)
            var error: Int32 = 0; var length = socklen_t(MemoryLayout<Int32>.size)
            guard Darwin.poll(&poll, 1, 1000) == 1, getsockopt(fd, SOL_SOCKET, SO_ERROR, &error, &length) == 0, error == 0 else { continue }
            var name = [CChar](repeating: 0, count: Int(NI_MAXHOST))
            getnameinfo(address, entry.ai_addrlen, &name, socklen_t(name.count), nil, 0, NI_NUMERICHOST)
            return String(cString: name) + ":" + port
        }
        return nil
    }

    /// Point the link's connections at Diamond on this network, or (empty) away from it.
    private func setHome(_ address: String) {
        guard address != homeAddress else { return }
        homeAddress = address
        TorrbabyLink.home(address.isEmpty ? nil : address)
        note(address.isEmpty ? "away: connections go by the direct path or the relay" : "at home: connections go straight to Diamond at \(address)")
    }

    /// Bronze, the token and STUN: from the shared keychain the app writes (current), else the
    /// VPN configuration (tunnels set up by earlier builds).
    private func loadConfiguration() {
        let configuration = (protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration ?? [:]
        let shared = TunnelShared.load()
        relay = shared?.relay ?? configuration["relay"] as? String ?? ""
        relayFingerprint = shared?.relayFingerprint ?? configuration["relayFingerprint"] as? String ?? ""
        stun = shared?.stun ?? configuration["stun"] as? String ?? ""
        token = shared?.token ?? configuration["token"] as? String ?? ""
        home = shared?.home ?? ""
        identity = shared?.identity ?? ""
        TorrbabyLink.identity(identity)
        TorrbabyLink.client(shared?.client ?? "")
    }

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
        loadConfiguration()
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
        // Another VPN took over: Torrbaby leaves it alone until it is next launched.
        if reason == .superceded { TunnelYield.yielded = true }
        keeper?.cancel(); monitor?.cancel()
        tbtunnel_stop(); TorrbabyLink.stop()
        completionHandler()
    }

    /// The app asks for the tunnel's state ("status"), a fresh direct path ("punch"), or to
    /// read its configuration again ("reload").
    override func handleAppMessage(_ messageData: Data, completionHandler: ((Data?) -> Void)?) {
        let message = String(data: messageData, encoding: .utf8) ?? ""
        if message == "punch" { TorrbabyLink.resumed() }
        if message == "reload" {
            let had = identity
            loadConfiguration()
            note("configuration reloaded from the app" + (identity != had ? (identity.count == 64 ? ": Diamond's identity pinned" : ": Diamond's identity forgotten") : ""))
            // A new identity takes effect when the link starts again.
            if identity != had { TorrbabyLink.stop() }
        }
        Task {
            let info = Bundle.main.infoDictionary
            let version = "version \(info?["CFBundleShortVersionString"] as? String ?? "?") (\(info?["CFBundleVersion"] as? String ?? "?"))"
            let state = await TorrbabyLink.state()
            // One line per part, for Settings → Connection.
            let text: String
            switch state?.state {
            case "home": text = "At home: connections go straight to Diamond on this network (\(homeAddress))"
            case "direct": text = "Direct path: open" + (state?.lastPunch.map { ", \($0.summary)" } ?? "") + "\nRelay via bronze: \(state?.relay ?? "?")"
            case "relay", "down": text = "Direct path: not open" + (state?.punching.map { " (punching: \($0))" } ?? state?.lastPunch.map { " (last punch: \($0.summary))" } ?? "") + "\nRelay via bronze: \(state?.relay == "dormant" ? "dormant (idle; it wakes in about a second)" : state?.relay ?? "?")"
            default: text = "Link not running"
            }
            completionHandler?((version + "\n" + text).data(using: .utf8))
        }
    }

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

    /// The network, for the link (it moves the relay and punches again once a change has held
    /// for 3 s). Home is looked for again on every change.
    private func watchNetwork() {
        let monitor = NWPathMonitor()
        monitor.pathUpdateHandler = { [weak self] path in
            guard let self else { return }
            DeviceNetwork.preferredInterface = path.availableInterfaces.first { $0.type == .wifi || $0.type == .cellular }?.name
            DeviceNetwork.gateways = path.gateways.compactMap { if case .hostPort(.ipv4(let address), _) = $0 { "\(address)" } else { nil } }.sorted().joined(separator: ",")
            TorrbabyLink.network(DeviceNetwork.signature(), interface: DeviceNetwork.physicalInterface() ?? "")
            self.homeLookedUp = .distantPast
        }
        monitor.start(queue: DispatchQueue(label: "org.torrbaby.tunnel.path"))
        self.monitor = monitor
    }

    /// Every 3 s: the stack is running, the link is started, and home is right.
    private func keep() {
        keeper = Task.detached { [weak self] in
            var ticks = 0
            var lastStats = ""
            while !Task.isCancelled {
                await self?.maintain()
                ticks += 1
                // Every ~9 s while traffic flows (the counters moved), every minute otherwise.
                var buffer = [CChar](repeating: 0, count: 1024)
                let stats = tbtunnel_stats(&buffer, buffer.count) > 0 ? String(cString: buffer) : ""
                let counters = stats.components(separatedBy: "; link").first ?? ""
                if (ticks % 3 == 0 && counters != lastStats) || ticks % 20 == 0 {
                    lastStats = counters
                    self?.note("memory \(String(format: "%.1f", Self.memoryMB())) MB; \(stats)")
                }
                try? await Task.sleep(for: .seconds(3))
            }
        }
    }

    /// The TCP stack that turns the tunnel's packets into streams (Rust).
    private func startStack() -> Bool {
        let context = Unmanaged.passUnretained(self).toOpaque()
        return tbtunnel_start(1280, { context, data, lengths, count in
            guard let context, let data, let lengths, count > 0 else { return }
            // Called on the Rust library's own threads, which have no autorelease pool: without
            // this one, the objects each packet's hand-off to iOS creates were never released
            // (about 40 MB in two minutes of video, and iOS ends the extension at 50 MB).
            autoreleasepool {
                let provider = Unmanaged<PacketTunnelProvider>.fromOpaque(context).takeUnretainedValue()
                var packets: [Data] = []
                packets.reserveCapacity(count)
                var at = 0
                for index in 0..<count {
                    packets.append(Data(bytes: data + at, count: lengths[index]))
                    at += lengths[index]
                }
                provider.packetFlow.writePackets(packets, withProtocols: Array(repeating: NSNumber(value: AF_INET), count: count))
            }
        }, context)
    }

    private func maintain() async {
        // Started a moment ago, the stack's task may not have marked itself running yet.
        if !tbtunnel_alive(), Date().timeIntervalSince(started) > 5 {
            note("TCP stack stopped: starting it again")
            _ = startStack()
        }
        // The link: started (again, a while after its relay could not open).
        if !TorrbabyLink.running {
            guard Date().timeIntervalSince(lastLinkStart) > 15 else { return }
            lastLinkStart = Date()
            loadConfiguration()
            let state = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            if let state { try? FileManager.default.createDirectory(at: state, withIntermediateDirectories: true) }
            if let port = await TorrbabyLink.start(relay: relay, fingerprint: relayFingerprint, stun: stun.isEmpty ? nil : stun, token: token, stateDir: state, lean: true) {
                note("link open (loopback port \(port))")
                homeAddress = ""; homeLookedUp = .distantPast
            } else {
                note("link: bronze's relay at \(relay) did not open; trying again shortly")
                return
            }
        }
        // At home (bronze sees this device at Diamond's public address) and Diamond answers here:
        // connections go straight to it. Looked at again each minute and on every network change.
        guard Date().timeIntervalSince(homeLookedUp) > 60 else { return }
        homeLookedUp = Date()
        guard let state = await TorrbabyLink.state(), let ip = state.publicIp, !ip.isEmpty else { homeLookedUp = .distantPast; return }
        if state.homeIp == ip, !home.isEmpty, let address = await Task.detached(operation: { [home] in Self.reachHome(home) }).value {
            setHome(address)
        } else {
            setHome("")
        }
    }
}
