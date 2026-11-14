#if os(iOS)
import Foundation
import NetworkExtension

/// The app's side of Torrbaby's packet tunnel (the extension is Apple/Tunnel): install it,
/// turn it on or off, and say whether it is up. While it is connected, Diamond is
/// `http://10.233.0.1:8765` from anywhere, straight through both NATs, and background
/// downloads keep going while the app is suspended. For devices without Tailscale: iOS
/// connects one VPN at a time, and turning this on would disconnect Tailscale.
@MainActor final class PacketTunnel: ObservableObject {
    static let shared = PacketTunnel()
    static let route = "http://10.233.0.1:8765"
    static let extensionID = "com.jonthomason.torrbaby.ios.tunnel"

    @Published private(set) var status: NEVPNStatus = .invalid
    @Published private(set) var installed = false
    @Published private(set) var lastError: String?
    /// iOS's own record of why the tunnel last disconnected.
    @Published private(set) var lastDisconnect: String?
    /// Read live from iOS every time: a remembered state can be stale (iOS reports a VPN as
    /// disconnected for a moment after loading, and a change while the app is suspended can
    /// be missed), which left the app routing around a connected tunnel.
    var connected: Bool { manager?.connection.status == .connected }
    /// Called when the tunnel connects or disconnects (the Store looks for a route again).
    var onChange: (() -> Void)?
    private var manager: NETunnelProviderManager?
    private var observer: NSObjectProtocol?

    func load() async {
        let managers = (try? await NETunnelProviderManager.loadAllFromPreferences()) ?? []
        manager = managers.first { ($0.protocolConfiguration as? NETunnelProviderProtocol)?.providerBundleIdentifier == Self.extensionID }
        installed = manager != nil
        watch()
        update()
    }

    /// Install (iOS asks once) or update the configuration, and connect. Not on demand yet:
    /// on a device that also runs Tailscale, iOS reconnecting this would knock Tailscale off.
    /// On demand belongs to devices that use only this tunnel (the coming Automatic setting).
    func turnOn(relay: String, relayFingerprint: String, stun: String, token: String) async {
        lastError = nil
        heldOff = false
        let manager = self.manager ?? NETunnelProviderManager()
        // The token goes to the shared keychain, not the VPN configuration.
        TunnelShared(relay: relay, relayFingerprint: relayFingerprint, stun: stun, token: token).save()
        let configuration = NETunnelProviderProtocol()
        configuration.providerBundleIdentifier = Self.extensionID
        configuration.serverAddress = "Diamond"
        configuration.providerConfiguration = ["relay": relay, "relayFingerprint": relayFingerprint, "stun": stun]
        manager.protocolConfiguration = configuration
        manager.localizedDescription = "Torrbaby"
        manager.isEnabled = true
        // On demand (iOS keeps it connected, after a restart too) only where it cannot fight
        // another VPN: none is connected, or Settings lets this one replace it.
        let onDemand = Self.otherVPN() == nil || UserDefaults.standard.bool(forKey: Self.replacesOtherVPNKey)
        manager.onDemandRules = [NEOnDemandRuleConnect()]
        manager.isOnDemandEnabled = onDemand
        do {
            try await manager.saveToPreferences()
            try await manager.loadFromPreferences()
            self.manager = manager
            installed = true
            watch()
            try manager.connection.startVPNTunnel()
        } catch {
            lastError = error.localizedDescription
        }
        update()
    }

    /// Disconnect, keeping the configuration (another VPN, such as Tailscale, can connect).
    /// It stays off until Connect: Torrbaby does not connect it again by itself.
    func disconnect() {
        heldOff = true
        // On demand would bring it straight back.
        if let manager, manager.isOnDemandEnabled { manager.isOnDemandEnabled = false; Task { try? await manager.saveToPreferences(); manager.connection.stopVPNTunnel() } }
        else { manager?.connection.stopVPNTunnel() }
    }

    /// Whether the user disconnected it on the Connection page (it stays off until Connect).
    var heldOff: Bool {
        get { UserDefaults.standard.bool(forKey: "tunnelHeldOff") }
        set { UserDefaults.standard.set(newValue, forKey: "tunnelHeldOff") }
    }
    /// Settings: connect even while another VPN (such as Tailscale) is connected, which
    /// disconnects it (iOS connects one VPN at a time). Off by default.
    static let replacesOtherVPNKey = "tunnelReplacesOtherVPN"

    /// Another VPN's interface is up: a utun with an IPv4 address other than this tunnel's
    /// (iOS's own utun interfaces carry only IPv6 link-local addresses). Tailscale's is 100.x.
    static func otherVPN() -> String? {
        var list: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&list) == 0, let first = list else { return nil }
        defer { freeifaddrs(list) }
        var found: String?
        for pointer in sequence(first: first, next: { $0.pointee.ifa_next }) {
            let entry = pointer.pointee
            guard let address = entry.ifa_addr, address.pointee.sa_family == UInt8(AF_INET) else { continue }
            let name = String(cString: entry.ifa_name)
            guard name.hasPrefix("utun") || name.hasPrefix("ipsec") || name.hasPrefix("ppp") else { continue }
            var host = [CChar](repeating: 0, count: Int(NI_MAXHOST))
            getnameinfo(address, socklen_t(address.pointee.sa_len), &host, socklen_t(host.count), nil, 0, NI_NUMERICHOST)
            let ip = String(cString: host)
            if ip == "10.233.0.2" || ip.isEmpty { continue }
            found = ip.hasPrefix("100.") ? "Tailscale" : "a VPN (\(ip))"
        }
        return found
    }

    /// Connect automatically (at launch and on returning to the foreground): always when no
    /// other VPN is connected, and over another VPN only when Settings says so. Not after the
    /// user disconnected it, and not before the app knows bronze and has its credentials.
    func autoConnect(relay: String, relayFingerprint: String, stun: String, token: String) async {
        // Keep the extension's copy current (a new token, a new bronze) even while connected.
        if installed, !relay.isEmpty, relayFingerprint.count == 64, !token.isEmpty {
            TunnelShared(relay: relay, relayFingerprint: relayFingerprint, stun: stun, token: token).save()
        }
        // Another VPN turned up and may not be replaced: on demand off, so iOS does not fight it.
        if let manager, manager.isOnDemandEnabled, Self.otherVPN() != nil, !UserDefaults.standard.bool(forKey: Self.replacesOtherVPNKey) {
            manager.isOnDemandEnabled = false
            try? await manager.saveToPreferences()
        }
        guard !connected, status != .connecting, !heldOff, !relay.isEmpty, relayFingerprint.count == 64, !token.isEmpty else { return }
        if Self.otherVPN() != nil && !UserDefaults.standard.bool(forKey: Self.replacesOtherVPNKey) { return }
        await turnOn(relay: relay, relayFingerprint: relayFingerprint, stun: stun, token: token)
    }

    func turnOff() async {
        guard let manager else { return }
        manager.isOnDemandEnabled = false
        try? await manager.saveToPreferences()
        manager.connection.stopVPNTunnel()
        try? await manager.removeFromPreferences()
        self.manager = nil
        installed = false
        update()
    }

    /// Bring `status` up to date with iOS (on returning to the foreground, before choosing a route).
    func refresh() {
        if manager == nil { Task { await load() } } else { update() }
    }

    /// The extension's own account of the paths. Never waits more than 2 s: an extension that
    /// is stopping may not answer, and the Connection page asks every second.
    func ask(_ message: String = "status") async -> String? {
        guard let session = manager?.connection as? NETunnelProviderSession, connected else { return nil }
        let reply = Reply()
        return await withCheckedContinuation { continuation in
            reply.continuation = continuation
            do {
                try session.sendProviderMessage(Data(message.utf8)) { data in
                    reply.finish(data.flatMap { String(data: $0, encoding: .utf8) })
                }
            } catch { reply.finish(nil) }
            DispatchQueue.main.asyncAfter(deadline: .now() + 2) { reply.finish(nil) }
        }
    }
    /// Resumes its continuation once, whichever comes first: the reply or the time limit.
    private final class Reply: @unchecked Sendable {
        var continuation: CheckedContinuation<String?, Never>?
        private let lock = NSLock()
        func finish(_ value: String?) {
            lock.lock(); let waiting = continuation; continuation = nil; lock.unlock()
            waiting?.resume(returning: value)
        }
    }

    private func watch() {
        guard observer == nil else { return }
        observer = NotificationCenter.default.addObserver(forName: .NEVPNStatusDidChange, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.update() }
        }
    }

    private func update() {
        let now = manager?.connection.status ?? .invalid
        guard now != status else { return }
        let wasConnected = status == .connected
        status = now
        if (now == .connected) != wasConnected { onChange?() }
        if now == .disconnected, let connection = manager?.connection {
            connection.fetchLastDisconnectError { [weak self] error in
                let text = error.map { ($0 as NSError).localizedDescription + " (\(($0 as NSError).domain) \(($0 as NSError).code))" }
                Task { @MainActor in self?.lastDisconnect = text }
            }
        }
    }

    var statusText: String {
        switch status {
        case .connected: "VPN connected"
        case .connecting: "connecting"
        case .disconnecting: "disconnecting"
        case .reasserting: "reconnecting"
        case .disconnected: "VPN off" + (heldOff ? ", disconnected here" : "") + (Self.otherVPN().map { "; \($0) is connected" } ?? "")
        case .invalid: installed ? "not configured" : "not installed"
        @unknown default: "unknown"
        }
    }
}
#endif
