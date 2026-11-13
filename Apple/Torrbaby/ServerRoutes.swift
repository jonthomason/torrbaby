import Foundation
import Darwin

/// Routing never changes the canonical server string used by caches and profiles.
enum ServerRoutes {
    static func normalized(_ address: String) -> String {
        address.trimmingCharacters(in: .whitespacesAndNewlines).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
    }
    static func isTailscale(_ address: String) -> Bool {
        guard let host = URL(string: address)?.host?.lowercased() else { return false }
        if host.hasSuffix(".ts.net") || host == "diamond-ts" { return true }
        if host.trimmingCharacters(in: CharacterSet(charactersIn: "[]")).hasPrefix("fd7a:115c:a1e0:") { return true }
        let parts = host.split(separator: ".").compactMap { Int($0) }
        return parts.count == 4 && parts[0] == 100 && (64...127).contains(parts[1])
            && parts.allSatisfy { (0...255).contains($0) }
    }
    /// A Tailscale address of this device (100.64.0.0/10 or fd7a:115c:a1e0::/48).
    static func isTailscaleIP(_ ip: String) -> Bool {
        let host = ip.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: "[]"))
        if host.hasPrefix("fd7a:115c:a1e0:") { return true }
        let parts = host.split(separator: ".").compactMap { Int($0) }
        return parts.count == 4 && parts[0] == 100 && (64...127).contains(parts[1])
            && parts.allSatisfy { (0...255).contains($0) }
    }
    /// Whether one of this device's own network interfaces (Wi-Fi or Ethernet, not
    /// a tunnel) is on the same IPv4 subnet as the address's host.
    static func sharesSubnet(_ address: String, interfaces: [(address: UInt32, mask: UInt32)]) -> Bool {
        guard let host = URL(string: address)?.host else { return false }
        let parts = host.split(separator: ".").compactMap { UInt32($0) }
        guard parts.count == 4, parts.allSatisfy({ $0 <= 255 }) else { return false }
        let target = parts.reduce(0) { $0 << 8 | $1 }
        return interfaces.contains { $0.mask != 0 && ($0.address & $0.mask) == (target & $0.mask) }
    }
    /// IPv4 addresses and masks of this device's physical interfaces.
    static func localInterfaces() -> [(address: UInt32, mask: UInt32)] {
        var result: [(address: UInt32, mask: UInt32)] = []
        var list: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&list) == 0, let first = list else { return result }
        defer { freeifaddrs(list) }
        for pointer in sequence(first: first, next: { $0.pointee.ifa_next }) {
            let entry = pointer.pointee
            let name = String(cString: entry.ifa_name)
            guard name.hasPrefix("en"), entry.ifa_flags & UInt32(IFF_UP) != 0,
                  let address = entry.ifa_addr, address.pointee.sa_family == UInt8(AF_INET),
                  let mask = entry.ifa_netmask else { continue }
            let value = address.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { UInt32(bigEndian: $0.pointee.sin_addr.s_addr) }
            let bits = mask.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { UInt32(bigEndian: $0.pointee.sin_addr.s_addr) }
            result.append((value, bits))
        }
        return result
    }
    /// The home address also counts as the fallback when it was reached through a
    /// tunnel: a Tailscale subnet router makes it answer on cellular or away Wi-Fi.
    static func isFallback(primary: String, active: String?, tunnelled: Bool = false) -> Bool {
        guard let active else { return false }
        return isTailscale(active) || normalized(active) != normalized(primary) || tunnelled
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
