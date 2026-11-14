#if os(iOS)
import Foundation
import Security

/// The packet tunnel extension's log, kept where both it and the app can read it (a keychain
/// item in a shared access group), so Settings → Connection can show what the extension did,
/// including just before it stopped. Its own system log is otherwise visible only with the
/// phone on a Mac. The last ~64 KB (about half an hour of busy logging).
enum TunnelLog {
    private static var group: String? { Bundle.main.object(forInfoDictionaryKey: "TorrbabySharedKeychainGroup") as? String }
    private static let lock = NSLock()
    private static let clock: DateFormatter = { let f = DateFormatter(); f.dateFormat = "HH:mm:ss.SSS"; return f }()
    private static func query(_ group: String) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: "org.torrbaby.tunnel-log",
         kSecAttrAccount as String: "log", kSecAttrAccessGroup as String: group]
    }

    static func write(_ line: String) {
        guard let group else { return }
        lock.lock(); defer { lock.unlock() }
        var text = load(group) + "\(clock.string(from: Date())) \(line)\n"
        if text.utf8.count > 64 * 1024 { text = String(text.suffix(48 * 1024)); if let cut = text.firstIndex(of: "\n") { text = String(text[text.index(after: cut)...]) } }
        let data = Data(text.utf8)
        let status = SecItemUpdate(query(group) as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if status == errSecItemNotFound {
            var add = query(group)
            add[kSecValueData as String] = data
            add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
            SecItemAdd(add as CFDictionary, nil)
        }
    }

    private static func load(_ group: String) -> String {
        var request = query(group)
        request[kSecReturnData as String] = true
        var result: CFTypeRef?
        guard SecItemCopyMatching(request as CFDictionary, &result) == errSecSuccess, let data = result as? Data else { return "" }
        return String(decoding: data, as: UTF8.self)
    }

    /// The last `count` lines, oldest first.
    static func read(_ count: Int = 40) -> [String] {
        guard let group else { return [] }
        return Array(load(group).split(separator: "\n").suffix(count).map(String.init))
    }

    static var available: Bool { group != nil }
}
#endif

#if os(iOS)
/// What the packet tunnel needs to reach Diamond, written by the app and read by the
/// extension, in the shared keychain group: bronze's address and certificate fingerprint,
/// its STUN server, and this device's token (kept out of the VPN configuration). The app
/// writes it whenever it connects the tunnel, so a new token or bronze address reaches the
/// extension at its next reconnect.
struct TunnelShared: Codable, Equatable {
    var relay: String
    var relayFingerprint: String
    var stun: String
    var token: String
    /// Diamond at home ("diamond.local:8765"): at home the tunnel carries connections straight
    /// to it. Looked up by name each time; no address is kept.
    var home: String? = nil
    /// Diamond's identity fingerprint: relayed traffic is encrypted end to end to it.
    var identity: String? = nil

    private static var group: String? { Bundle.main.object(forInfoDictionaryKey: "TorrbabySharedKeychainGroup") as? String }
    private static func query(_ group: String) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: "org.torrbaby.tunnel",
         kSecAttrAccount as String: "configuration", kSecAttrAccessGroup as String: group]
    }

    func save() {
        guard let group = Self.group, let data = try? JSONEncoder().encode(self), Self.load() != self else { return }
        let status = SecItemUpdate(Self.query(group) as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if status == errSecItemNotFound {
            var add = Self.query(group)
            add[kSecValueData as String] = data
            // The extension runs while the phone is locked (after its first unlock since a restart).
            add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            SecItemAdd(add as CFDictionary, nil)
        }
    }

    static func load() -> TunnelShared? {
        guard let group else { return nil }
        var request = query(group)
        request[kSecReturnData as String] = true
        var result: CFTypeRef?
        guard SecItemCopyMatching(request as CFDictionary, &result) == errSecSuccess, let data = result as? Data else { return nil }
        return try? JSONDecoder().decode(TunnelShared.self, from: data)
    }
}
#endif

#if os(iOS)
/// Another VPN took over (iOS runs one at a time). The extension records it as it stops, so the
/// app knows even if it was suspended then; the tunnel then stays off until Torrbaby is next
/// launched, when it takes over again. Kept in the shared keychain group, like the log.
enum TunnelYield {
    private static var group: String? { Bundle.main.object(forInfoDictionaryKey: "TorrbabySharedKeychainGroup") as? String }
    private static func query(_ group: String) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: "org.torrbaby.tunnel-yield",
         kSecAttrAccount as String: "yielded", kSecAttrAccessGroup as String: group]
    }

    static var yielded: Bool {
        get {
            guard let group else { return false }
            var request = query(group)
            request[kSecReturnData as String] = true
            var result: CFTypeRef?
            return SecItemCopyMatching(request as CFDictionary, &result) == errSecSuccess && (result as? Data) == Data([1])
        }
        set {
            guard let group else { return }
            let data = Data([newValue ? 1 : 0])
            if SecItemUpdate(query(group) as CFDictionary, [kSecValueData as String: data] as CFDictionary) == errSecItemNotFound {
                var add = query(group)
                add[kSecValueData as String] = data
                add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
                SecItemAdd(add as CFDictionary, nil)
            }
        }
    }
}
#endif
