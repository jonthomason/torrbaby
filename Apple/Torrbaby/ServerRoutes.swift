import Foundation

/// Routing never changes the canonical server string used by caches and profiles.
enum ServerRoutes {
    static func normalized(_ address: String) -> String {
        address.trimmingCharacters(in: .whitespacesAndNewlines).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
    }
    static func primaryAddress(for identity: String) -> String {
        guard let url = URL(string: identity), url.port == 8765,
              ["100.80.147.115", "diamond-ts", "diamond-ts.bengal-locrian.ts.net"].contains(url.host?.lowercased() ?? "") else { return identity }
        return "http://192.168.1.44:8765"
    }
    static func defaultFallback(for identity: String) -> String {
        let primary = primaryAddress(for: identity)
        guard let url = URL(string: primary), url.port == 8765,
              ["192.168.1.44", "diamond", "diamond.local"].contains(url.host?.lowercased() ?? "") else { return "" }
        return "http://100.80.147.115:8765"
    }
    static func isTailscale(_ address: String) -> Bool {
        guard let host = URL(string: address)?.host?.lowercased() else { return false }
        if host.hasSuffix(".ts.net") || host == "diamond-ts" { return true }
        if host.trimmingCharacters(in: CharacterSet(charactersIn: "[]")).hasPrefix("fd7a:115c:a1e0:") { return true }
        let parts = host.split(separator: ".").compactMap { Int($0) }
        return parts.count == 4 && parts[0] == 100 && (64...127).contains(parts[1])
            && parts.allSatisfy { (0...255).contains($0) }
    }
    static func isFallback(primary: String, active: String?) -> Bool {
        guard let active else { return false }
        return isTailscale(active) || normalized(active) != normalized(primary)
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
