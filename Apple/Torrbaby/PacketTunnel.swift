#if os(iOS)
import Foundation
import NetworkExtension
import UIKit

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

    /// Install (iOS asks once) or update the configuration, and connect (on demand, so iOS keeps
    /// it connected, after a restart too, until another VPN takes over).
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
        manager.onDemandRules = [NEOnDemandRuleConnect()]
        manager.isOnDemandEnabled = true
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
    /// This launch has been handled (the tunnel taken back from another VPN if need be).
    private var launched = false

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
            // Ours, and iOS's own 192.0.0.x (464XLAT on IPv6-only cellular), are not other VPNs.
            if ip == "10.233.0.2" || ip.isEmpty || ip.hasPrefix("192.0.0.") { continue }
            found = ip.hasPrefix("100.") ? "Tailscale" : "a VPN (\(ip))"
        }
        return found
    }

    /// Connect automatically. At launch it connects even over another VPN (iOS runs one at a
    /// time, so that one is disconnected). Once another VPN has taken over, or was started while
    /// this was off, it stays off, on demand too, until Torrbaby is next launched: returning to
    /// the foreground does not take the connection back. Never after the user disconnected it
    /// on the Connection page, and not before the app knows bronze and has its credentials.
    func autoConnect(relay: String, relayFingerprint: String, stun: String, token: String) async {
        // Keep the extension's copy current (a new token, a new bronze) even while connected.
        if installed, !relay.isEmpty, relayFingerprint.count == 64, !token.isEmpty {
            TunnelShared(relay: relay, relayFingerprint: relayFingerprint, stun: stun, token: token).save()
        }
        let ready = !relay.isEmpty && relayFingerprint.count == 64 && !token.isEmpty
        // The launch: the first call of this process with the credentials known (on a first
        // launch they come after pairing), with the app on screen. iOS also starts the app in
        // the background for downloads; that is not the user launching Torrbaby.
        let launching = !launched && ready && UIApplication.shared.applicationState != .background
        if launching { launched = true; TunnelYield.yielded = false }
        else if TunnelYield.yielded || (!connected && status != .connecting && Self.otherVPN() != nil) {
            await yield()
            return
        }
        guard !connected, status != .connecting, !heldOff, ready else { return }
        await turnOn(relay: relay, relayFingerprint: relayFingerprint, stun: stun, token: token)
    }

    /// Another VPN has the device: stay off until the next launch. On demand goes off, or iOS
    /// would reconnect this and knock the other VPN off.
    private func yield() async {
        TunnelYield.yielded = true
        if let manager, manager.isOnDemandEnabled {
            manager.isOnDemandEnabled = false
            try? await manager.saveToPreferences()
        }
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
        // Only a disconnect seen happening: iOS keeps the last error indefinitely, and reading
        // it on the first check after launch reported an old error as new.
        let wasUp = [.connected, .connecting, .reasserting, .disconnecting].contains(status)
        status = now
        if (now == .connected) != wasConnected { onChange?() }
        // Disconnected with another VPN up: that VPN took over.
        if now == .disconnected, Self.otherVPN() != nil, wasUp || manager?.isOnDemandEnabled == true { Task { await yield() } }
        if now == .disconnected, wasUp, let connection = manager?.connection {
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
        case .disconnected: "VPN off" + (heldOff ? ", disconnected here" : TunnelYield.yielded ? ", another VPN took over (back at the next launch)" : "") + (Self.otherVPN().map { "; \($0) is connected" } ?? "")
        case .invalid: installed ? "not configured" : "not installed"
        @unknown default: "unknown"
        }
    }
}
#endif
