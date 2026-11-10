#if os(tvOS)
import Foundation
import Security

/// A small, credential-free catalog shared only by this app and its extension.
struct TopShelfSnapshot: Codable, Equatable {
    struct Item: Codable, Equatable {
        let id: String
        let title: String
        let context: String
        let summary: String
        let image: URL
        let detailURL: URL
        let playURL: URL
    }
    let profile: String
    let items: [Item]
    private static var query: [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: "org.torrbaby.topshelf",
         kSecAttrAccount as String: "snapshot",
         kSecAttrAccessGroup as String: Bundle.main.object(forInfoDictionaryKey: "TopShelfKeychainGroup") as? String ?? ""]
    }
    static func read() -> TopShelfSnapshot? {
        var attributes = query
        attributes[kSecReturnData as String] = true
        var result: CFTypeRef?
        guard SecItemCopyMatching(attributes as CFDictionary, &result) == errSecSuccess,
              let data = result as? Data else { return nil }
        return try? JSONDecoder().decode(Self.self, from: data)
    }
    func save() -> Bool {
        guard let data = try? JSONEncoder().encode(self) else { return false }
        let changes = [kSecValueData as String: data]
        let status = SecItemUpdate(Self.query as CFDictionary, changes as CFDictionary)
        if status == errSecSuccess { return true }
        guard status == errSecItemNotFound else { return false }
        var attributes = Self.query
        attributes[kSecValueData as String] = data
        attributes[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        return SecItemAdd(attributes as CFDictionary, nil) == errSecSuccess
    }
    static func clear() { SecItemDelete(query as CFDictionary) }
}
#endif
