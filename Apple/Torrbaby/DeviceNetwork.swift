#if os(iOS)
import Foundation
import Darwin

/// The network this device is on, for Torrbaby's link (Apple/Direct): which interface carries
/// data, and a signature that changes when the network does.
enum DeviceNetwork {
    /// The interface iOS uses for data, from the network monitor (a dual-SIM phone's second
    /// SIM may be pdp_ip1, not pdp_ip0). Set by the Store on every path change.
    nonisolated(unsafe) static var preferredInterface: String?
    /// The network's gateways, from the network monitor: moving between two phone hotspots can
    /// keep the same interface and address range, but not the same gateway.
    nonisolated(unsafe) static var gateways = ""

    /// The real network: the one iOS uses for data when it has an IPv4 address, else Wi-Fi,
    /// else any cellular interface. The link's sockets are bound to it, so no VPN carries them.
    static func physicalInterface() -> String? {
        let addresses = ipv4Addresses()
        if let preferred = preferredInterface, addresses[preferred] != nil { return preferred }
        if addresses["en0"] != nil { return "en0" }
        return addresses.keys.filter { $0.hasPrefix("pdp_ip") }.sorted().first
    }

    /// Which network this is: the interface and its IPv4 address. Changes when Wi-Fi joins or
    /// leaves, or the phone moves between carriers (CT ↔ CM), even on the same interface.
    static func signature() -> String {
        guard let name = physicalInterface() else { return "none" }
        return "\(name) \(ipv4Addresses()[name] ?? "?")" + (gateways.isEmpty ? "" : " via \(gateways)")
    }

    private static func ipv4Addresses() -> [String: String] {
        var list: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&list) == 0, let first = list else { return [:] }
        defer { freeifaddrs(list) }
        var result: [String: String] = [:]
        for entry in sequence(first: first, next: { $0.pointee.ifa_next }) {
            guard let address = entry.pointee.ifa_addr, address.pointee.sa_family == UInt8(AF_INET),
                  entry.pointee.ifa_flags & UInt32(IFF_UP) != 0 else { continue }
            var host = [CChar](repeating: 0, count: Int(NI_MAXHOST))
            getnameinfo(address, socklen_t(address.pointee.sa_len), &host, socklen_t(host.count), nil, 0, NI_NUMERICHOST)
            let name = String(cString: entry.pointee.ifa_name)
            if result[name] == nil { result[name] = String(cString: host) }
        }
        return result
    }
}
#endif
