#if os(iOS)
import Foundation
import Darwin

/// The phone's half of the test: plain UDP sockets bound to the physical interface.
final class PunchProbe: @unchecked Sendable {
    struct Mapping { let socket: Int; let ip: String; let port: Int }
    struct Hit { let socket: Int; let afterMs: Int; let kind: String }
    struct Result { var hit: Hit?; var firstSpray: Hit?; var roundTrips: [Int] = [] }

    let count: Int
    let interface: String?
    private var sockets: [Int32] = []
    var ready: Bool { !sockets.isEmpty }

    init(count: Int) {
        self.count = count
        interface = Self.physicalInterface()
        let index = interface.map { if_nametoindex($0) } ?? 0
        for _ in 0..<count {
            let fd = socket(AF_INET, SOCK_DGRAM, 0)
            guard fd >= 0 else { continue }
            var address = sockaddr_in(); address.sin_family = sa_family_t(AF_INET); address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
            _ = withUnsafePointer(to: &address) { $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size)) } }
            // On the real network, not the VPN: an exit node would carry it abroad.
            if index != 0 { var bound = index; setsockopt(fd, IPPROTO_IP, IP_BOUND_IF, &bound, socklen_t(MemoryLayout<UInt32>.size)) }
            _ = fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK)
            sockets.append(fd)
        }
    }
    func close() { for fd in sockets { Darwin.close(fd) }; sockets = [] }
    /// Hand one socket over (it is no longer closed here); its descriptor.
    func take(_ index: Int) -> Int32? {
        guard sockets.indices.contains(index) else { return nil }
        let fd = sockets[index]
        sockets.remove(at: index)
        return fd
    }
    /// The local port a socket is bound to.
    func localPort(_ index: Int) -> UInt16? {
        guard sockets.indices.contains(index) else { return nil }
        var address = sockaddr_in(); var length = socklen_t(MemoryLayout<sockaddr_in>.size)
        let ok = withUnsafeMutablePointer(to: &address) { $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { getsockname(sockets[index], $0, &length) } }
        return ok == 0 ? UInt16(bigEndian: address.sin_port) : nil
    }
    /// Fire `word` packets at the server from every socket until one is echoed (both ways
    /// proven); the index of that socket.
    func punch(at server: String, id: String, word: String, seconds: Int) -> Int? {
        guard let target = Self.endpoint(server) else { return nil }
        let deadline = Self.milliseconds() + seconds * 1000
        var nextSend = 0
        while Self.milliseconds() < deadline {
            if Self.milliseconds() >= nextSend {
                for (index, fd) in sockets.enumerated() { send(fd, Array("\(word) \(id) \(index)".utf8), to: target) }
                nextSend = Self.milliseconds() + 200
            }
            for (index, fd) in sockets.enumerated() {
                while let (data, from) = receive(fd) {
                    guard from.sin_addr.s_addr == target.sin_addr.s_addr, let text = String(bytes: data, encoding: .utf8) else { continue }
                    if text.hasPrefix("TBACK") && text.contains("\(word) \(id) ") { return index }
                }
            }
            usleep(5_000)
        }
        return nil
    }

    /// The interface iOS uses for data, from the network monitor (a dual-SIM phone's second
    /// SIM may be pdp_ip1, not pdp_ip0). Set by the Store on every path change.
    nonisolated(unsafe) static var preferredInterface: String?
    /// The network's gateways, from the network monitor: moving between two phone hotspots can
    /// keep the same interface and address range, but not the same gateway.
    nonisolated(unsafe) static var gateways = ""

    /// The network to punch from: the one iOS uses for data when it has an IPv4 address,
    /// else Wi-Fi, else any cellular interface.
    static func physicalInterface() -> String? {
        let addresses = ipv4Addresses()
        if let preferred = preferredInterface, addresses[preferred] != nil { return preferred }
        if addresses["en0"] != nil { return "en0" }
        return addresses.keys.filter { $0.hasPrefix("pdp_ip") }.sorted().first
    }

    /// Which network this is: the interface and its IPv4 address. Changes when Wi-Fi joins or
    /// leaves, or the phone moves between carriers (CT ↔ CM), even on the same interface.
    static func networkSignature() -> String {
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

    private static func resolve(_ host: String, _ port: UInt16) -> sockaddr_in? {
        var hints = addrinfo(); hints.ai_family = AF_INET; hints.ai_socktype = SOCK_DGRAM
        var result: UnsafeMutablePointer<addrinfo>?
        guard getaddrinfo(host, String(port), &hints, &result) == 0, let info = result else { return nil }
        defer { freeaddrinfo(result) }
        return info.pointee.ai_addr.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { $0.pointee }
    }
    private static func endpoint(_ text: String) -> sockaddr_in? {
        let parts = text.split(separator: ":")
        guard parts.count == 2, let port = UInt16(parts[1]) else { return nil }
        var address = sockaddr_in(); address.sin_family = sa_family_t(AF_INET); address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        address.sin_port = port.bigEndian
        guard inet_pton(AF_INET, String(parts[0]), &address.sin_addr) == 1 else { return nil }
        return address
    }
    private func send(_ fd: Int32, _ bytes: [UInt8], to address: sockaddr_in) {
        var target = address
        _ = bytes.withUnsafeBytes { buffer in
            withUnsafePointer(to: &target) { $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                sendto(fd, buffer.baseAddress, bytes.count, 0, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            } }
        }
    }
    private func receive(_ fd: Int32) -> ([UInt8], sockaddr_in)? {
        var buffer = [UInt8](repeating: 0, count: 1500)
        var from = sockaddr_in(); var length = socklen_t(MemoryLayout<sockaddr_in>.size)
        let count = withUnsafeMutablePointer(to: &from) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { recvfrom(fd, &buffer, buffer.count, 0, $0, &length) }
        }
        guard count > 0 else { return nil }
        return (Array(buffer[..<count]), from)
    }
    private static func milliseconds() -> Int { Int(DispatchTime.now().uptimeNanoseconds / 1_000_000) }

    /// Each socket's public address, from Torrbaby's own STUN server on bronze (`stun`,
    /// "ip:port"; no third-party STUN servers). `limit`: ask from only the first few sockets;
    /// the rest of the window is covered by the server's spray.
    func discover(stun: String, limit: Int = .max) -> [Mapping] {
        for target in [stun] {
            guard let server = Self.endpoint(target) else { continue }
            var transactions: [[UInt8]: Int] = [:]
            for (index, fd) in sockets.enumerated() where index < limit {
                var request: [UInt8] = [0, 1, 0, 0, 0x21, 0x12, 0xA4, 0x42]
                let id = (0..<12).map { _ in UInt8.random(in: 0...255) }
                request += id; transactions[id] = index
                send(fd, request, to: server)
            }
            var found: [Int: Mapping] = [:]
            let deadline = Self.milliseconds() + 2500
            while Self.milliseconds() < deadline && found.count < min(limit, sockets.count) {
                for fd in sockets {
                    while let (data, _) = receive(fd) {
                        guard data.count >= 20, let index = transactions[Array(data[8..<20])], let mapped = Self.mapped(data) else { continue }
                        found[index] = Mapping(socket: index, ip: mapped.0, port: mapped.1)
                    }
                }
                usleep(10_000)
            }
            if !found.isEmpty { return found.values.sorted { $0.socket < $1.socket } }
        }
        return []
    }
    private static func mapped(_ data: [UInt8]) -> (String, Int)? {
        var i = 20
        while i + 4 <= data.count {
            let kind = Int(data[i]) << 8 | Int(data[i + 1]), length = Int(data[i + 2]) << 8 | Int(data[i + 3])
            guard i + 4 + length <= data.count else { return nil }
            let value = Array(data[(i + 4)..<(i + 4 + length)])
            if (kind == 0x0020 || kind == 0x8020), value.count >= 8, value[1] == 1 {
                let port = (Int(value[2]) << 8 | Int(value[3])) ^ 0x2112
                let ip = zip(value[4..<8], [0x21, 0x12, 0xA4, 0x42] as [UInt8]).map { String($0 ^ $1) }.joined(separator: ".")
                return (ip, port)
            }
            if kind == 0x0001, value.count >= 8, value[1] == 1 {
                return (value[4..<8].map(String.init).joined(separator: "."), Int(value[2]) << 8 | Int(value[3]))
            }
            i += 4 + length + (4 - length % 4) % 4
        }
        return nil
    }

    /// Fire at the server from every socket until a packet from it arrives, then time pings.
    func fire(at server: String, id: String, seconds: Int) -> Result {
        guard let target = Self.endpoint(server) else { return Result() }
        var result = Result()
        let start = Self.milliseconds(), deadline = start + seconds * 1000
        var nextSend = 0
        while Self.milliseconds() < deadline && result.hit == nil {
            if Self.milliseconds() >= nextSend {
                for (index, fd) in sockets.enumerated() { send(fd, Array("TBPUNCH1 \(id) \(index) \(Self.milliseconds())".utf8), to: target) }
                nextSend = Self.milliseconds() + 200
            }
            for (index, fd) in sockets.enumerated() {
                while let (data, from) = receive(fd) {
                    guard from.sin_addr.s_addr == target.sin_addr.s_addr, let text = String(bytes: data, encoding: .utf8),
                          text.contains("TBPUNCH1 \(id) ") else { continue }
                    // Only an echo proves both directions on this socket: a spray can arrive on
                    // a socket whose outgoing port toward the server differs (random-port NATs).
                    if text.hasPrefix("TBACK") {
                        if result.hit == nil { result.hit = Hit(socket: index, afterMs: Self.milliseconds() - start, kind: "echo") }
                    } else if result.firstSpray == nil {
                        result.firstSpray = Hit(socket: index, afterMs: Self.milliseconds() - start, kind: "Diamond's spray")
                    }
                }
            }
            usleep(5_000)
        }
        guard let hit = result.hit else { return result }
        // Round trips on the socket that got through, to the server's address.
        let fd = sockets[hit.socket]
        for n in 0..<10 {
            let sent = Self.milliseconds()
            send(fd, Array("TBPUNCH1 \(id) \(hit.socket) \(sent) ping\(n)".utf8), to: target)
            let wait = sent + 1500
            var answered = false
            while Self.milliseconds() < wait && !answered {
                while let (data, _) = receive(fd) {
                    if let text = String(bytes: data, encoding: .utf8), text.hasPrefix("TBACK"), text.hasSuffix("ping\(n)") {
                        result.roundTrips.append(Self.milliseconds() - sent); answered = true
                    }
                }
                usleep(2_000)
            }
            usleep(150_000)
        }
        return result
    }
}
#endif
