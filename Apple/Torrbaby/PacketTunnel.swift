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
    var connected: Bool { status == .connected }
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

    /// Install (iOS asks once) or update the configuration, and connect. On demand, so iOS
    /// brings it back after a restart or a drop.
    func turnOn(relay: String, relayFingerprint: String, stun: String, token: String) async {
        lastError = nil
        let manager = self.manager ?? NETunnelProviderManager()
        let configuration = NETunnelProviderProtocol()
        configuration.providerBundleIdentifier = Self.extensionID
        configuration.serverAddress = "Diamond"
        configuration.providerConfiguration = ["relay": relay, "relayFingerprint": relayFingerprint, "stun": stun, "token": token]
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

    /// The extension's own account of the paths ("direct open · relay open · …").
    func ask(_ message: String = "status") async -> String? {
        guard let session = manager?.connection as? NETunnelProviderSession, connected else { return nil }
        return await withCheckedContinuation { continuation in
            do {
                try session.sendProviderMessage(Data(message.utf8)) { reply in
                    continuation.resume(returning: reply.flatMap { String(data: $0, encoding: .utf8) })
                }
            } catch { continuation.resume(returning: nil) }
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
        let wasConnected = connected
        status = now
        if connected != wasConnected { onChange?() }
    }

    var statusText: String {
        switch status {
        case .connected: "connected"
        case .connecting: "connecting"
        case .disconnecting: "disconnecting"
        case .reasserting: "reconnecting"
        case .disconnected: "disconnected"
        case .invalid: installed ? "not configured" : "not installed"
        @unknown default: "unknown"
        }
    }
}
#endif
