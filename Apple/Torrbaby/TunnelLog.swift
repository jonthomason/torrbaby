#if os(iOS)
import Foundation
import Security

/// The packet tunnel extension's log, kept where both it and the app can read it (a keychain
/// item in a shared access group), so Settings → Connection can show what the extension did,
/// including just before it stopped. Its own system log is otherwise visible only with the
/// phone on a Mac. The last ~12 KB.
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
        if text.utf8.count > 12 * 1024 { text = String(text.suffix(8 * 1024)); if let cut = text.firstIndex(of: "\n") { text = String(text[text.index(after: cut)...]) } }
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
