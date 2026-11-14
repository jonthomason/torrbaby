import Foundation
import Network
import NetworkExtension
import os

// Apple/Direct (Rust), linked statically: the TCP stack and the QUIC paths.
@_silgen_name("tbtunnel_start") private func tbtunnel_start(_ mtu: UInt16, _ write: @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<UInt8>?, UnsafePointer<Int>?, Int) -> Void, _ context: UnsafeMutableRawPointer?) -> Bool
@_silgen_name("tbtunnel_input") private func tbtunnel_input(_ packet: UnsafePointer<UInt8>?, _ length: Int) -> Bool
@_silgen_name("tbtunnel_stop") private func tbtunnel_stop()
@_silgen_name("tbtunnel_alive") private func tbtunnel_alive() -> Bool
@_silgen_name("tbdirect_identity") private func tbdirect_identity(_ fingerprint: UnsafePointer<CChar>)
@_silgen_name("tbdirect_client") private func tbdirect_client(_ id: UnsafePointer<CChar>)
@_silgen_name("tbtunnel_open_flows") private func tbtunnel_open_flows() -> UInt64
@_silgen_name("tbtunnel_waiting_flows") private func tbtunnel_waiting_flows() -> UInt64
@_silgen_name("tbtunnel_home") private func tbtunnel_home(_ address: UnsafePointer<CChar>)
@_silgen_name("tbtunnel_stats") private func tbtunnel_stats(_ buffer: UnsafeMutablePointer<CChar>, _ capacity: Int) -> Int32
@_silgen_name("tbdirect_alive") private func tbdirect_alive() -> Bool
@_silgen_name("tbdirect_stop") private func tbdirect_stop()
@_silgen_name("tbdirect_relay_start_lean") private func tbdirect_relay_start(_ fd: Int32, _ server: UnsafePointer<CChar>, _ fingerprint: UnsafePointer<CChar>) -> UInt16
@_silgen_name("tbdirect_relay_alive") private func tbdirect_relay_alive() -> Bool
@_silgen_name("tbdirect_relay_stop") private func tbdirect_relay_stop()
@_silgen_name("tbdirect_relay_rebind") private func tbdirect_relay_rebind(_ fd: Int32) -> Bool
@_silgen_name("tbdirect_relay_info") private func tbdirect_relay_info(_ buffer: UnsafeMutablePointer<CChar>, _ capacity: Int) -> Int32

/// Torrbaby's packet tunnel: Diamond at 10.233.0.1, reached straight through both NATs.
///
/// iOS keeps this extension running while Torrbaby is suspended, so the direct path it holds
/// stays open and background downloads (fetched by iOS itself) go straight to Diamond. Only
/// 10.233.0.1 is routed here; nothing else on the device is touched. The relay through bronze
/// carries the signalling, and streams only while there is no direct path.
final class PacketTunnelProvider: NEPacketTunnelProvider, @unchecked Sendable {
    static let diamond = "10.233.0.1"
    private let log = Logger(subsystem: "org.torrbaby", category: "tunnel")
    private var relay = "", relayFingerprint = "", stun = "", token = "", home = "", identity = ""
    private var relayPort: UInt16 = 0
    private var monitor: NWPathMonitor?
    private var keeper: Task<Void, Never>?
    private var networkSignature = ""
    /// Public addresses a predicted spray failed from: the next attempt there sprays random ports.
    private var windowFailedFrom: Set<String> = []
    private var lastAttempt = Date.distantPast
    private let started = Date()
    private var attempting = false
    /// Behind the same NAT as Diamond (bronze sees the same public IP for both) and Diamond
    /// answers on this network: the tunnel carries its connections straight to Diamond here
    /// and is otherwise quiet (no relay, no direct path, no keepalives, nothing to bronze)
    /// until the network changes. The tunnel's address works at home too, so a background
    /// download keeps one address whether the phone is at home or away.
    private var atHome = false
    /// Away from home and idle: no paths, no keepalives, until a connection waits.
    private var dormant = false
    private var lastBusy = Date()
    /// Diamond on this network ("ip:port") while at home, and when it was last looked up.
    private var homeAddress = ""
    private var homeLookedUp = Date.distantPast
    private var noHomeNoted = false
    /// Same public address as Diamond, but Diamond does not answer here: relay only.
    private var behindDiamondNAT = false

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

    /// Point the tunnel's connections at Diamond on this network, or (empty) away from it.
    private func setHome(_ address: String) {
        guard address != homeAddress else { return }
        homeAddress = address
        address.withCString { tbtunnel_home($0) }
    }
    /// The public IP the direct path was punched from, and when bronze was last asked.
    private var punchedFrom: String?
    private var lastInfo = Date.distantPast
    /// Bronze saw another address than the direct path's, and the relay was reconnected over
    /// the current network to compare like with like (a relay left on the old carrier after a
    /// data-SIM switch reports the old carrier's address: re-punching on that looped forever).
    private var relayReopenedForMismatch = false
    /// Failed punches on this network, and when the next may start. Bronze is a bridge, not a
    /// way to watch: where punching works (it did on this network, or on this carrier before)
    /// a failure is a bad moment and the next try comes soon (5 s, 15 s, 30 s, then every
    /// minute); where it never has, the tries are expensive and back off (1, 2, 5, then 15 min,
    /// with a gentler spray). Forgotten when the network changes.
    private var punchFailures = 0
    private var nextPunch = Date.distantPast
    private var punchedOnThisNetwork = false
    /// This network's public IP as bronze sees it (for the carrier check below).
    private var publicIP: String?
    /// Carriers where a punch has succeeded, kept across restarts: the kind of connection
    /// (Wi-Fi, which includes phone hotspots, or cellular) and the public IP's first two parts.
    /// The kind matters: China Mobile punches on the phone's own cellular, but a phone hotspot
    /// on China Mobile (a NAT inside the carrier's) may never.
    private static let punchableKey = "punchableCarriers"
    private static func carrier(_ ip: String) -> String {
        let kind = (PunchProbe.physicalInterface() ?? "").hasPrefix("pdp") ? "cellular" : "wifi"
        return kind + " " + ip.split(separator: ".").prefix(2).joined(separator: ".")
    }
    /// Six failures in a row even here: treat it as a network that does not punch.
    private var punchable: Bool {
        guard punchFailures < 6 else { return false }
        return punchedOnThisNetwork || publicIP.map { (UserDefaults.standard.stringArray(forKey: Self.punchableKey) ?? []).contains(Self.carrier($0)) } == true
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
        identity.withCString { tbdirect_identity($0) }
        (shared?.client ?? "").withCString { tbdirect_client($0) }
    }

    /// Bronze's view over the relay: this device's public IP and Diamond's.
    private static func relayInfo() async -> (ip: String, homeIP: String?)? {
        await Task.detached { () -> (String, String?)? in
            var buffer = [CChar](repeating: 0, count: 256)
            guard tbdirect_relay_info(&buffer, buffer.count) > 0 else { return nil }
            var ip: String?, home: String?
            for line in String(cString: buffer).split(separator: "\n") {
                if line.hasPrefix("you ") { ip = line.dropFirst(4).split(separator: ":").dropLast().joined(separator: ":") }
                if line.hasPrefix("homeip ") { home = String(line.dropFirst(7)) }
            }
            return ip.map { ($0, home) }
        }.value
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
        tbtunnel_stop(); tbdirect_stop(); tbdirect_relay_stop()
        completionHandler()
    }

    /// The app asks for the tunnel's state ("status") or a fresh direct path ("punch").
    override func handleAppMessage(_ messageData: Data, completionHandler: ((Data?) -> Void)?) {
        let message = String(data: messageData, encoding: .utf8) ?? ""
        if message == "punch" { lastAttempt = .distantPast; nextPunch = .distantPast }
        if message == "reload" {
            let had = identity
            loadConfiguration()
            note("configuration reloaded from the app" + (identity != had ? (identity.count == 64 ? ": relay now encrypted end to end to Diamond" : ": Diamond's identity forgotten") : ""))
        }
        // One line per part, for Settings → Connection.
        let direct = tbdirect_alive() ? "Direct path: open, \(punched)" : "Direct path: not open" + (status.isEmpty ? "" : " (\(status))")
        let info = Bundle.main.infoDictionary
        let version = "version \(info?["CFBundleShortVersionString"] as? String ?? "?") (\(info?["CFBundleVersion"] as? String ?? "?"))"
        let state = atHome ? version + "\nAt home: connections go straight to Diamond on this network (\(homeAddress))"
            : dormant ? version + "\nDormant: idle, nothing sent until the next connection (it wakes in about a second)"
            : version + "\n" + direct + "\nRelay via bronze: " + (tbdirect_relay_alive() ? "open" : "closed")
                + (identity.count == 64 ? ", encrypted end to end to Diamond" : ", not encrypted end to end yet (Diamond's identity is learned at home or from pairing)")
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
    ///
    /// A change is acted on once it has held for 3 s: Wi-Fi and cellular often swap back and forth
    /// for a moment (Wi-Fi joined, lost, joined again), and a flap that ends where it started
    /// leaves both paths as they were.
    private func watchNetwork() {
        let monitor = NWPathMonitor()
        let queue = DispatchQueue(label: "org.torrbaby.tunnel.path")
        monitor.pathUpdateHandler = { [weak self] path in
            guard let self else { return }
            PunchProbe.preferredInterface = path.availableInterfaces.first { $0.type == .wifi || $0.type == .cellular }?.name
            PunchProbe.gateways = path.gateways.compactMap { if case .hostPort(.ipv4(let address), _) = $0 { "\(address)" } else { nil } }.sorted().joined(separator: ",")
            let signature = PunchProbe.networkSignature()
            if self.networkSignature.isEmpty { self.networkSignature = signature; return }
            self.settling?.cancel()
            guard signature != self.networkSignature else { return }
            let settle = DispatchWorkItem { [weak self] in
                guard let self, PunchProbe.networkSignature() == signature, signature != self.networkSignature else { return }
                self.networkChanged(to: signature)
            }
            self.settling = settle
            queue.asyncAfter(deadline: .now() + 3, execute: settle)
        }
        monitor.start(queue: queue)
        self.monitor = monitor
    }
    private var settling: DispatchWorkItem?

    /// A different network has held for 3 s: the direct path's hole was for the old one, so it
    /// is punched again. The relay moves with QUIC to the new network and keeps bridging
    /// meanwhile (bronze has a public address); only if it cannot move is it opened again.
    private func networkChanged(to signature: String) {
        setHome("")
        tbdirect_stop()
        let probe = PunchProbe(count: 1)
        let moved = probe.ready && tbdirect_relay_alive() && probe.take(0).map { tbdirect_relay_rebind($0) } == true
        probe.close()
        if !moved { tbdirect_relay_stop() }
        note("network changed (\(networkSignature) → \(signature)): direct path dropped, relay " + (moved ? "moved to the new network" : "reopening"))
        networkSignature = signature
        lastAttempt = .distantPast
        atHome = false; punchedFrom = nil; noHomeNoted = false; behindDiamondNAT = false
        punchFailures = 0; nextPunch = .distantPast; punchedOnThisNetwork = false; publicIP = nil
    }

    /// Keep the relay and the direct path open: check every few seconds, open what is closed.
    private func keep() {
        keeper = Task.detached { [weak self] in
            var ticks = 0
            var lastStats = ""
            // Each second (local work only): the paths are looked after every 3 s, and at once
            // when a connection is waiting for one (a dormant tunnel wakes within a second).
            while !Task.isCancelled {
                if ticks % 3 == 0 || tbtunnel_waiting_flows() > 0 { await self?.maintain() }
                ticks += 1
                // Every ~9 s while traffic flows (the counters moved), every 30 s otherwise, and
                // every 5 minutes while dormant.
                var buffer = [CChar](repeating: 0, count: 768)
                let stats = tbtunnel_stats(&buffer, buffer.count) > 0 ? String(cString: buffer) : ""
                let counters = stats.components(separatedBy: "; ").first ?? ""
                let dormant = self?.dormant ?? false
                if (ticks % 9 == 0 && counters != lastStats) || ticks % (dormant ? 300 : 30) == 0 {
                    lastStats = counters
                    self?.note("memory \(String(format: "%.1f", Self.memoryMB())) MB, direct \(tbdirect_alive() ? "open" : "closed"), relay \(tbdirect_relay_alive() ? "open" : "closed")\(dormant ? ", dormant" : ""); \(stats)")
                }
                try? await Task.sleep(for: .seconds(1))
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
        // Battery: away from home, the paths' keepalives (every 5 s) keep the radio awake. With
        // nothing open for 2 minutes the paths are let go; the next connection wakes them (the
        // relay opens in about a second while it waits, then the direct path is punched).
        if !atHome {
            let open = tbtunnel_open_flows(), waiting = tbtunnel_waiting_flows()
            if open > 0 || waiting > 0 { lastBusy = Date() }
            if dormant {
                guard waiting > 0 else { return }
                dormant = false; lastBusy = Date(); lastAttempt = .distantPast
                note("a connection is waiting: waking (opening the relay)")
            } else if Date().timeIntervalSince(lastBusy) > 120 {
                dormant = true
                tbdirect_stop(); tbdirect_relay_stop(); punchedFrom = nil
                status = "dormant: idle, nothing sent until the next connection"
                note("idle for 2 minutes: paths let go, no keepalives, until the next connection")
                return
            }
        }
        if atHome {
            // Diamond's address here is looked up again each minute (it may have changed).
            guard Date().timeIntervalSince(homeLookedUp) > 60 else { return }
            homeLookedUp = Date()
            let home = self.home
            if let address = await Task.detached(operation: { Self.reachHome(home) }).value { setHome(address); return }
            note("at home, but Diamond no longer answers on this network: using the relay again")
            setHome(""); atHome = false
        }
        if !tbdirect_relay_alive() { loadConfiguration(); await openRelay() }
        // Every 6 s, bronze's view: at home, or the public address moved (a carrier switch
        // behind a hotspot, invisible from here) and the direct path's hole is for the old one.
        if tbdirect_relay_alive(), Date().timeIntervalSince(lastInfo) > 6 {
            lastInfo = Date()
            if let info = await Self.relayInfo() {
                publicIP = info.ip
                if let homeIP = info.homeIP, homeIP == info.ip {
                    let home = self.home
                    if !home.isEmpty, let address = await Task.detached(operation: { Self.reachHome(home) }).value {
                        note("at home (same public address as Diamond, \(homeIP)): connections go to Diamond at \(address); relay and direct path closed")
                        // Connections are pointed home first; those on the relay are then cut and
                        // their clients' next requests go home.
                        setHome(address); homeLookedUp = Date()
                        tbdirect_stop(); tbdirect_relay_stop()
                        atHome = true; punchedFrom = nil
                        status = "at home: Diamond is on this network"
                        return
                    }
                    // Behind Diamond's own NAT a punch can only hairpin, which gets through one
                    // way at best: the relay carries everything here.
                    if !noHomeNoted { note("same public address as Diamond, but \(home.isEmpty ? "Diamond's home address is not known" : "\(home) does not answer here"): relay only, no punching"); noHomeNoted = true }
                    tbdirect_stop(); punchedFrom = nil; behindDiamondNAT = true
                    status = "behind Diamond's own network address: relay only"
                    return
                }
                behindDiamondNAT = false
                if let from = punchedFrom, tbdirect_alive(), from != info.ip {
                    if !relayReopenedForMismatch {
                        note("bronze sees \(info.ip), the direct path is from \(from): reconnecting the relay over the current network first")
                        tbdirect_relay_stop(); relayReopenedForMismatch = true
                        return
                    }
                    note("public address changed (\(from) → \(info.ip)): punching again")
                    tbdirect_stop(); punchedFrom = nil; lastAttempt = .distantPast; nextPunch = .distantPast; relayReopenedForMismatch = false
                } else {
                    relayReopenedForMismatch = false
                }
            }
        }
        guard !tbdirect_alive(), !attempting, !behindDiamondNAT, tbdirect_relay_alive() else { return }
        // A failure waits a minute before the next try; a path that dropped is tried at once.
        guard Date() >= nextPunch else { return }
        attempting = true; lastAttempt = Date()
        note("punching a direct path (memory \(String(format: "%.1f", Self.memoryMB())) MB)")
        defer { attempting = false }
        do {
            let punched = try await DirectPunch.run(stun: stun, signal: "http://127.0.0.1:\(relayPort)", token: token, randomFrom: windowFailedFrom, lean: true, gentle: punchFailures > 0 && !punchable) { [weak self] in
                self?.note("direct: \($0.step)")
            }
            status = ""
            self.punched = punched.summary
            punchedFrom = punched.publicAddress
            punchFailures = 0; nextPunch = .distantPast; punchedOnThisNetwork = true
            var carriers = UserDefaults.standard.stringArray(forKey: Self.punchableKey) ?? []
            let carrier = Self.carrier(punched.publicAddress)
            if !carriers.contains(carrier) { carriers.append(carrier); UserDefaults.standard.set(Array(carriers.suffix(64)), forKey: Self.punchableKey) }
            note("direct open: \(punched.summary), QUIC \(punched.quicMs.map { "\($0) ms" } ?? "?") (memory \(String(format: "%.1f", Self.memoryMB())) MB)")
        } catch {
            if let address = (error as? DirectPunch.Failure)?.windowFailedAt { windowFailedFrom.insert(address) }
            punchFailures += 1
            let wait: TimeInterval = punchable ? [5, 15, 30, 60][min(punchFailures, 4) - 1] : [60, 120, 300, 900][min(punchFailures, 4) - 1]
            nextPunch = Date().addingTimeInterval(wait)
            note("next punch in \(wait < 60 ? "\(Int(wait)) s" : "\(Int(wait / 60)) min") (failure \(punchFailures) on this network, " + (punchable ? "which punches: bronze only bridges)" : "which has not punched yet)"))
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

}
