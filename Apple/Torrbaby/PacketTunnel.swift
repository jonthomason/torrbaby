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
        let manager = self.manager ?? NETunnelProviderManager()
        let configuration = NETunnelProviderProtocol()
        configuration.providerBundleIdentifier = Self.extensionID
        configuration.serverAddress = "Diamond"
        configuration.providerConfiguration = ["relay": relay, "relayFingerprint": relayFingerprint, "stun": stun, "token": token]
        manager.protocolConfiguration = configuration
        manager.localizedDescription = "Torrbaby"
        manager.isEnabled = true
        manager.isOnDemandEnabled = false
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
    func disconnect() { manager?.connection.stopVPNTunnel() }

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
        case .disconnected: "VPN off (another VPN, such as Tailscale, may be connected instead)"
        case .invalid: installed ? "not configured" : "not installed"
        @unknown default: "unknown"
        }
    }
}
#endif
