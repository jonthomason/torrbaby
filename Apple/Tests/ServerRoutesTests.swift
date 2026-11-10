import Foundation

@main struct ServerRoutesTests {
    static func main() async throws {
        let primary = "http://192.168.1.44:8765"
        let fallback = ServerRoutes.defaultFallback(for: primary)
        assert(fallback == "http://100.80.147.115:8765")
        assert(ServerRoutes.defaultFallback(for: "http://someone-else:8765").isEmpty)
        var visited: [String] = []
        let (route, value) = try await ServerRoutes.probe(
            addresses: ServerRoutes.candidates(primary: primary, fallback: fallback, current: nil)
        ) { address in
            visited.append(address)
            if address == primary { throw URLError(.cannotConnectToHost) }
            return 42
        }
        assert(route == fallback && value == 42 && visited == [primary, fallback])
        // A working fallback is reused instead of paying the LAN timeout on every refresh.
        assert(ServerRoutes.candidates(primary: primary, fallback: fallback, current: route) == [fallback, primary])
        enum AuthorizationError: Error { case rejected }
        visited = []
        do {
            let _: (String, Int) = try await ServerRoutes.probe(addresses: [primary, fallback]) { address in
                visited.append(address)
                throw AuthorizationError.rejected
            }
            fatalError("Authentication failure was swallowed")
        } catch AuthorizationError.rejected {}
        assert(visited == [primary])
        visited = []
        do {
            let _: (String, Int) = try await ServerRoutes.probe(addresses: [primary, fallback]) { address in
                visited.append(address)
                throw URLError(.timedOut)
            }
            fatalError("Both unreachable routes must fail")
        } catch let error as URLError { assert(error.code == .timedOut) }
        assert(visited == [primary, fallback])
        do {
            _ = try ServerRoutes.endpoint("http://user:secret@example.com", path: "/v1/status")
            fatalError("Embedded credentials accepted")
        } catch {}
        print("Route fallback, reuse, authorization, exhaustion and URL validation passed")
    }
}
