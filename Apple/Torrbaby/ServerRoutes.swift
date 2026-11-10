import Foundation

/// Routing never changes the canonical server string used by caches and profiles.
enum ServerRoutes {
    static func normalized(_ address: String) -> String {
        address.trimmingCharacters(in: .whitespacesAndNewlines).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
    }
    static func defaultFallback(for primary: String) -> String {
        guard let url = URL(string: primary), url.port == 8765,
              ["192.168.1.44", "diamond", "diamond.local"].contains(url.host?.lowercased() ?? "") else { return "" }
        return "http://100.80.147.115:8765"
    }
    static func endpoint(_ address: String, path: String) throws -> URL {
        let address = normalized(address)
        guard let base = URL(string: address),
              ["http", "https"].contains(base.scheme?.lowercased() ?? ""), base.host != nil,
              base.user == nil, base.password == nil, base.query == nil, base.fragment == nil,
              let url = URL(string: address + path) else { throw URLError(.badURL) }
        return url
    }
    static func candidates(primary: String, fallback: String, current: String?) -> [String] {
        var result: [String] = []
        for value in [current ?? "", primary, fallback].map(normalized) where !value.isEmpty {
            if !result.contains(value) { result.append(value) }
        }
        return result
    }
    /// Only read-only probes are retried. Mutations are never replayed on another route.
    static func probe<T>(addresses: [String], fetch: (String) async throws -> T) async throws -> (String, T) {
        var failure: Error = URLError(.cannotConnectToHost)
        for address in addresses {
            try Task.checkCancellation()
            do { return (address, try await fetch(address)) }
            catch {
                guard let network = error as? URLError, network.code != .cancelled else { throw error }
                failure = error
            }
        }
        throw failure
    }
}
